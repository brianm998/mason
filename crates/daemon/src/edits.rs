// SPDX-License-Identifier: GPL-3.0-or-later

//! The agent's file edits, step run by step run (task 1130): each `Edit`, `MultiEdit` and
//! `Write` the stream reported, appended in order to `edits-<run>.jsonl` in the task's log
//! directory (not the store). `mason task show --edits` prints them as compact diffs; the
//! reviewer and the diagnosis agent get only the per-file summary lines.

use mason_adapter_api::AgentEvent;
use serde_json::{json, Value};
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// The most bytes one run's file keeps; later edits of that run are not recorded.
const MAX_FILE: u64 = 4 << 20;
/// The most diff lines shown per side of one edit.
const SHOW_LINES: usize = 40;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub turn: u32,
    pub tool: String,
    pub path: String,
    pub old: Option<String>,
    pub new: String,
    pub truncated: bool,
}

impl Edit {
    pub fn to_json(&self) -> Value {
        json!({"turn": self.turn, "tool": self.tool, "path": self.path, "old": self.old,
               "new": self.new, "truncated": self.truncated})
    }

    pub fn from_json(v: &Value) -> Option<Edit> {
        Some(Edit {
            turn: v["turn"].as_u64().unwrap_or(0) as u32,
            tool: v["tool"].as_str()?.to_string(),
            path: v["path"].as_str()?.to_string(),
            old: v["old"].as_str().map(str::to_string),
            new: v["new"].as_str().unwrap_or("").to_string(),
            truncated: v["truncated"].as_bool().unwrap_or(false),
        })
    }
}

fn file(task_dir: &Path, run: u64) -> PathBuf {
    task_dir.join(format!("edits-{run}.jsonl"))
}

/// Append `ev` to the run's file if it is a file edit.
pub fn record(task_dir: &Path, run: u64, ev: &AgentEvent) {
    let AgentEvent::FileEdit {
        tool,
        path,
        old,
        new,
        truncated,
        turn,
    } = ev
    else {
        return;
    };
    let p = file(task_dir, run);
    if std::fs::metadata(&p).is_ok_and(|m| m.len() >= MAX_FILE) {
        return;
    }
    let e = Edit {
        turn: *turn,
        tool: tool.clone(),
        path: path.clone(),
        old: old.clone(),
        new: new.clone(),
        truncated: *truncated,
    };
    let line = format!("{}\n", e.to_json());
    let r = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&p)
        .and_then(|mut f| f.write_all(line.as_bytes()));
    if let Err(e) = r {
        tracing::warn!(run, "a file edit could not be recorded: {e}");
    }
}

/// The run's edits in the order they were made.
pub fn read(task_dir: &Path, run: u64) -> Vec<Edit> {
    std::fs::read_to_string(file(task_dir, run))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter_map(|v| Edit::from_json(&v))
        .collect()
}

/// Every run of the task that has edits, oldest first.
pub fn runs(task_dir: &Path) -> Vec<u64> {
    let mut v: Vec<u64> = std::fs::read_dir(task_dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let n = e.file_name().into_string().ok()?;
            n.strip_prefix("edits-")?
                .strip_suffix(".jsonl")?
                .parse()
                .ok()
        })
        .collect();
    v.sort_unstable();
    v
}

/// The endpoint's body: the edits of `run`, or of every run that has some.
pub fn report(task_dir: &Path, run: Option<u64>) -> Value {
    let list = run.map_or_else(|| runs(task_dir), |r| vec![r]);
    json!({"runs": list.iter().map(|r| json!({
        "run": r,
        "edits": read(task_dir, *r).iter().map(Edit::to_json).collect::<Vec<_>>(),
    })).collect::<Vec<_>>()})
}

/// Whether `later` puts a file back as an earlier edit of it had it: an `Edit` that swaps the
/// text of an earlier one back, or a `Write` of text an earlier `Write` had already put there.
fn undoes(later: &Edit, earlier: &Edit, between_writes: bool) -> bool {
    match (&later.old, &earlier.old) {
        (Some(lo), Some(eo)) => *lo == earlier.new && later.new == *eo,
        (None, None) => between_writes && later.new == earlier.new,
        _ => false,
    }
}

/// How many times each file was edited, in order of first edit, and whether a later edit
/// undid an earlier one.
pub fn per_file(edits: &[Edit]) -> Vec<(String, usize, bool)> {
    let mut out: Vec<(String, usize, bool)> = vec![];
    for (i, e) in edits.iter().enumerate() {
        let reverted = edits[..i].iter().enumerate().any(|(j, x)| {
            x.path == e.path && {
                let between = edits[j + 1..i]
                    .iter()
                    .any(|m| m.path == e.path && m.old.is_none() && m.new != e.new);
                undoes(e, x, between)
            }
        });
        match out.iter_mut().find(|o| o.0 == e.path) {
            Some(o) => {
                o.1 += 1;
                o.2 |= reverted;
            }
            None => out.push((e.path.clone(), 1, reverted)),
        }
    }
    out
}

/// Agent text safe to print: control characters (escape sequences too) become spaces.
fn plain(s: &str) -> String {
    s.replace(|c: char| c.is_control(), " ")
}

/// One line per file: `path: edited 3×, reverted`.
pub fn summary_lines(edits: &[Edit]) -> Vec<String> {
    per_file(edits)
        .into_iter()
        .map(|(p, n, rev)| {
            format!(
                "{}: edited {n}×{}",
                plain(&p),
                if rev { ", reverted" } else { "" }
            )
        })
        .collect()
}

/// The edit as a compact diff: the lines both texts share at the start and end are left out.
fn diff_lines(e: &Edit) -> Vec<String> {
    let old: Vec<&str> = e.old.as_deref().map_or(vec![], |o| o.lines().collect());
    let new: Vec<&str> = e.new.lines().collect();
    let pre = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
    let suf = old[pre..]
        .iter()
        .rev()
        .zip(new[pre..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let (old, new) = (&old[pre..old.len() - suf], &new[pre..new.len() - suf]);
    let mut out = vec![];
    for (sign, lines) in [('-', old), ('+', new)] {
        for l in lines.iter().take(SHOW_LINES) {
            out.push(format!("{sign}{}", plain(l)));
        }
        if lines.len() > SHOW_LINES {
            out.push(format!("{sign}… {} more line(s)", lines.len() - SHOW_LINES));
        }
    }
    out
}

/// The timeline of one run: a header and compact diff per edit, then a line per file.
pub fn timeline(run: u64, edits: &[Edit]) -> Vec<String> {
    let mut out = vec![format!("run {run}: {} edit(s)", edits.len())];
    for (i, e) in edits.iter().enumerate() {
        out.push(format!(
            "@@ edit {} of {} · turn {} · {} {}{} @@",
            i + 1,
            edits.len(),
            e.turn,
            e.tool,
            plain(&e.path),
            if e.truncated { " (text cut)" } else { "" }
        ));
        out.extend(diff_lines(e));
    }
    out.extend(summary_lines(edits).into_iter().map(|l| format!("= {l}")));
    out
}

/// What `mason task show --edits` prints, from the endpoint's body.
pub fn report_lines(v: &Value) -> Vec<String> {
    let mut out = vec![];
    for r in v["runs"].as_array().into_iter().flatten() {
        let edits: Vec<Edit> = r["edits"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Edit::from_json)
            .collect();
        if !edits.is_empty() {
            out.extend(timeline(r["run"].as_u64().unwrap_or(0), &edits));
        }
    }
    if out.is_empty() {
        out.push("no file edits recorded".into());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(tool: &str, path: &str, old: Option<&str>, new: &str, turn: u32) -> AgentEvent {
        AgentEvent::FileEdit {
            tool: tool.into(),
            path: path.into(),
            old: old.map(str::to_string),
            new: new.into(),
            truncated: false,
            turn,
        }
    }

    #[test]
    fn a_reverted_edit_is_summarised_and_the_timeline_has_a_step_per_edit() {
        let t = tempfile::tempdir().unwrap();
        for e in [
            ev("Edit", "a.rs", Some("x = 1"), "x = 2", 1),
            ev("Write", "b.rs", None, "fn b() {}\n", 2),
            ev("Edit", "a.rs", Some("x = 2"), "x = 1", 3),
            ev("Edit", "a.rs", Some("y"), "z", 4),
        ] {
            record(t.path(), 7, &e);
        }
        let edits = read(t.path(), 7);
        assert_eq!(edits.len(), 4);
        assert_eq!(
            summary_lines(&edits),
            ["a.rs: edited 3×, reverted", "b.rs: edited 1×"]
        );
        let lines = report_lines(&report(t.path(), None));
        assert_eq!(lines.iter().filter(|l| l.starts_with("@@ edit")).count(), 4);
        assert!(lines.contains(&"-x = 1".to_string()), "{lines:?}");
        assert!(lines.contains(&"= a.rs: edited 3×, reverted".to_string()));
        assert_eq!(runs(t.path()), [7]);
    }

    #[test]
    fn a_write_of_earlier_text_is_a_revert_and_terminal_escapes_are_not_printed() {
        let w = |s: &str| {
            Edit::from_json(&json!({"tool":"Write","path":"f\u{1b}[2J","new":s})).unwrap()
        };
        let edits = [w("one"), w("two"), w("one")];
        assert_eq!(summary_lines(&edits), ["f [2J: edited 3×, reverted"]);
        assert!(!per_file(&[w("one"), w("one")])[0].2);
    }

    #[test]
    fn no_edits_say_so() {
        assert_eq!(
            report_lines(&json!({"runs": []})),
            ["no file edits recorded"]
        );
    }
}
