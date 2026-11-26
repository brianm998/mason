// SPDX-License-Identifier: GPL-3.0-or-later

//! Holding back a queued task while a task in testing, review or landing changes the same
//! files (s398). Two tasks on one file mean one redoes its merge, so the later one starts after
//! the earlier lands. The decision is pure; `Daemon::file_hold` gathers the facts.

use crate::daemon::Daemon;
use mason_core::{Priority, State, Task, TaskId};
use mason_engine::World;
use std::time::{Duration, Instant};

/// The files a task body names: words with a `/` whose last part has an extension, with any
/// trailing `:line` and punctuation dropped.
pub fn paths_in_body(body: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for w in body.split(|c: char| {
        c.is_whitespace()
            || matches!(
                c,
                '`' | '"' | '\'' | '(' | ')' | '[' | ']' | ',' | ';' | '*'
            )
    }) {
        let w = w.trim_end_matches(['.', ':']);
        let w = match w.rsplit_once(':') {
            Some((a, n)) if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) => a,
            _ => w,
        };
        let w = w.trim_start_matches("./");
        let last = w.rsplit('/').next().unwrap_or("");
        let ok = w.contains('/')
            && !w.contains("://")
            && !w.starts_with('/')
            && !w.contains("..")
            && last.rsplit_once('.').is_some_and(|(stem, ext)| {
                !stem.is_empty()
                    && !ext.is_empty()
                    && ext.chars().all(|c| c.is_ascii_alphanumeric())
            });
        if ok && !out.iter().any(|o| o == w) {
            out.push(w.to_string());
        }
    }
    out
}

/// The part of a task body that says what the task changes: without the **Trust** section
/// (up to the next `**…**` heading), which names files only to say they are protected, and
/// without the standard footer and whatever follows it (s1163).
fn changing_text(body: &str) -> String {
    let mut out = String::new();
    let mut in_trust = false;
    for line in body.lines() {
        if line.starts_with("How to work in this repository") {
            break;
        }
        if line.starts_with("**Trust**") {
            in_trust = true;
        } else if line
            .strip_prefix("**")
            .is_some_and(|rest| rest.contains("**"))
        {
            in_trust = false;
        }
        if !in_trust {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// The files a queued task names: its plan's, or, with none and `explored`, those its body
/// says it changes.
fn named_files(plan_json: Option<&str>, body: &str, explored: bool) -> Vec<String> {
    let mine = plan_json.map(plan_files).unwrap_or_default();
    if mine.is_empty() && explored {
        paths_in_body(&changing_text(body))
    } else {
        mine
    }
}

/// The files a plan lists (`files`, strings), as written by the explorer.
pub fn plan_files(plan_json: &str) -> Vec<String> {
    let v: serde_json::Value = serde_json::from_str(plan_json).unwrap_or_default();
    v.get("files")
        .and_then(|f| f.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str())
                .flat_map(paths_in_body)
                .collect()
        })
        .unwrap_or_default()
}

/// The first in-flight task that changes a file `mine` names, and the file. Files matching an
/// `ignore` glob never count. `held_for` is how long this task has been held; at `limit_secs`
/// the hold ends (0 turns holding off).
pub fn decide(
    mine: &[String],
    in_flight: &[(TaskId, Vec<String>)],
    ignore: &[String],
    held_for: Duration,
    limit_secs: u64,
) -> Option<(TaskId, String)> {
    if limit_secs == 0 || held_for >= Duration::from_secs(limit_secs) {
        return None;
    }
    let ignored = |f: &str| ignore.iter().any(|g| crate::protect::glob(g, f));
    for (id, theirs) in in_flight {
        for f in mine {
            if !ignored(f) && theirs.iter().any(|t| t == f) {
                return Some((*id, f.clone()));
            }
        }
    }
    None
}

impl Daemon {
    /// Is this queued task held for a task that changes a file it names? The other task and the
    /// file. Never for an urgent task, and never longer than the project's `hold_back_secs`.
    pub fn file_hold(&self, w: &World, t: &Task) -> Option<(TaskId, String)> {
        if t.state != State::Queued || t.priority == Priority::Urgent {
            return None;
        }
        let p = self.project(t.project)?;
        if p.hold_back_secs == 0 {
            return None;
        }
        let ignored = |f: &str| {
            p.hold_back_ignore
                .iter()
                .any(|g| crate::protect::glob(g, f))
        };
        let plan = std::fs::read_to_string(self.task_dir(t.id).join("plan.json")).ok();
        let mut mine = named_files(plan.as_deref(), &t.body, t.explored_counts());
        mine.retain(|f| !ignored(f));
        if mine.is_empty() {
            return None;
        }
        let in_flight: Vec<(TaskId, Vec<String>)> = w
            .tasks
            .values()
            .filter(|o| {
                o.id != t.id
                    && o.project == t.project
                    && matches!(o.state, State::Testing | State::Review | State::Landing)
            })
            .filter_map(|o| {
                let range = format!("{}...{}", p.base_ref(), p.branch(o.id.0));
                let out =
                    mason_landing::git::run(&p.path, &["diff", "--name-only", &range]).ok()?;
                Some((o.id, out.lines().map(String::from).collect()))
            })
            .collect();
        // Check for an overlap first, so the clock starts only when a hold would apply.
        decide(
            &mine,
            &in_flight,
            &p.hold_back_ignore,
            Duration::ZERO,
            u64::MAX,
        )?;
        let since = *self
            .file_held
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(t.id)
            .or_insert_with(Instant::now);
        decide(
            &mine,
            &in_flight,
            &p.hold_back_ignore,
            since.elapsed(),
            p.hold_back_secs,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn a_body_names_its_files() {
        let b = "Fix `crates/daemon/src/daemon.rs:884` and (gui/starTests/starTests.swift).\nSee https://x.io/a/b.html.";
        assert_eq!(
            paths_in_body(b),
            s(&[
                "crates/daemon/src/daemon.rs",
                "gui/starTests/starTests.swift"
            ])
        );
        assert_eq!(
            plan_files(r#"{"files":["a/b.rs (new)","c/d.rs"]}"#),
            s(&["a/b.rs", "c/d.rs"])
        );
    }

    #[test]
    fn a_task_on_a_file_in_flight_waits_and_one_on_other_files_does_not() {
        let fl = vec![(TaskId(7), s(&["a/x.rs", "b/y.rs"]))];
        let d = |mine: &[&str]| decide(&s(mine), &fl, &[], Duration::ZERO, 7200);
        assert_eq!(d(&["c/z.rs", "b/y.rs"]), Some((TaskId(7), "b/y.rs".into())));
        assert_eq!(d(&["c/z.rs"]), None);
        // Once that task has landed it is no longer in flight.
        assert_eq!(
            decide(&s(&["b/y.rs"]), &[], &[], Duration::ZERO, 7200),
            None
        );
    }

    const FOOTER: &str = "How to work in this repository (the same as every earlier stage):\n\
        - Write a record in docs/decisions/0001-x.md; see docs/self-hosting.md.\n\
        - `bash scripts/run-exit-scripts.sh` and scripts/check-pure.sh must pass.\n";

    #[test]
    fn the_footer_names_no_files() {
        let b = format!("**Change**\nEdit `crates/a/x.rs`.\n\n{FOOTER}");
        assert_eq!(named_files(None, &b, true), s(&["crates/a/x.rs"]));
    }

    #[test]
    fn the_trust_section_names_no_files() {
        let b = "**Cause**\n`crates/a/cause.rs:3` is wrong.\n\n\
                 **Trust**\n`crates/daemon/src/protect.rs:46` lists it.\n\n\
                 Nor `crates/daemon/src/other.rs`.\n\n\
                 **Change**\nEdit crates/a/change.rs.\n";
        assert_eq!(
            named_files(None, b, true),
            s(&["crates/a/cause.rs", "crates/a/change.rs"])
        );
        // The heading and its text may share a line, and a heading may carry a note.
        let b = "**Trust** crates/daemon/src/protect.rs is untouched.\n\
                 **After landing (owner)**\nEdit docs/a.md.\n";
        assert_eq!(named_files(None, b, true), s(&["docs/a.md"]));
    }

    #[test]
    fn a_plan_is_read_whole_and_an_unexplored_body_is_not_read() {
        let plan = r#"{"files":["docs/self-hosting.md"]}"#;
        assert_eq!(
            named_files(Some(plan), "Edit a/b.rs.", true),
            s(&["docs/self-hosting.md"])
        );
        assert!(named_files(None, "Edit a/b.rs.", false).is_empty());
    }

    #[test]
    fn a_footer_overlap_does_not_hold_and_a_changed_file_still_does() {
        let fl = vec![(
            TaskId(7),
            s(&[
                "docs/self-hosting.md",
                "scripts/run-exit-scripts.sh",
                "crates/a/x.rs",
            ]),
        )];
        let d = |change: &str| {
            let b = format!("**Change**\nEdit {change}.\n\n{FOOTER}");
            decide(&named_files(None, &b, true), &fl, &[], Duration::ZERO, 7200)
        };
        assert_eq!(d("crates/b/y.rs"), None);
        assert_eq!(
            d("crates/a/x.rs"),
            Some((TaskId(7), "crates/a/x.rs".into()))
        );
    }

    #[test]
    fn the_hold_ends_after_the_limit_and_ignored_files_never_hold() {
        let fl = vec![(TaskId(7), s(&["a/x.rs"]))];
        let mine = s(&["a/x.rs"]);
        assert!(decide(&mine, &fl, &[], Duration::from_secs(7199), 7200).is_some());
        assert!(decide(&mine, &fl, &[], Duration::from_secs(7200), 7200).is_none());
        assert!(decide(&mine, &fl, &[], Duration::ZERO, 0).is_none());
        assert!(decide(&mine, &fl, &s(&["a/*.rs"]), Duration::ZERO, 7200).is_none());
    }
}
