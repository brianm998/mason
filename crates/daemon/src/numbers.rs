// SPDX-License-Identifier: GPL-3.0-or-later

//! The numbers MASON hands out: stage notes (`docs/stages/sNN.md`), exit scripts
//! (`scripts/exit/sNN.sh`, `sNN-real.sh`, `sNN-manual.sh`) and decision records
//! (`docs/decisions/NNNN-*.md`). A task never looks for "the next number": it is told its
//! numbers, taken from a counter that lives in the store, so two tasks cut from the same
//! base can never pick the same name.
//!
//! The first half of this file is plain logic over strings; the second half reads the base
//! branch and the store.

use crate::config::ProjectCfg;
use crate::daemon::{now_ms, Daemon};
use mason_core::{Task, TaskId};
use mason_landing::git;
use mason_store::ReservationRow;
use std::path::Path;

/// Exit scripts and stage notes have two or three digits: `scripts/run-exit-scripts.sh` runs
/// `s[0-9][0-9].sh` and `s[0-9][0-9][0-9].sh`. Widening those globs is a gate change, so the
/// counter stops here.
pub const EXIT_CEILING: u32 = 999;
/// Decisions have four digits.
pub const DECISION_CEILING: u32 = 9999;
/// Where ad-hoc tasks start, above the planned stages (setting `adhoc_exit_start`).
pub const DEFAULT_ADHOC_START: u32 = 40;

pub const KIND_DECISION: &str = "decision";
pub const KIND_EXIT: &str = "exit";

/// A numbered file MASON hands out names for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Numbered {
    /// A stage note or an exit script: they share one number of two or three digits.
    Stage(u32),
    Decision(u32),
}

/// The number a path carries, if it is one of the numbered names.
pub fn parse_path(path: &str) -> Option<Numbered> {
    let digits = |s: &str, n: usize| -> Option<u32> {
        (s.len() == n && s.bytes().all(|b| b.is_ascii_digit())).then(|| s.parse().ok())?
    };
    let stage = |s: &str| digits(s, 2).or_else(|| digits(s, 3)).map(Numbered::Stage);
    if let Some(f) = path.strip_prefix("docs/stages/") {
        let stem = f.strip_suffix(".md")?;
        return stage(stem.strip_prefix('s')?);
    }
    if let Some(f) = path.strip_prefix("docs/decisions/") {
        if f.contains('/') || !f.ends_with(".md") {
            return None;
        }
        let (n, _) = f.split_once('-')?;
        return digits(n, 4).map(Numbered::Decision);
    }
    if let Some(f) = path.strip_prefix("scripts/exit/") {
        let stem = f.strip_suffix(".sh")?;
        let stem = stem
            .strip_suffix("-real")
            .or_else(|| stem.strip_suffix("-manual"))
            .unwrap_or(stem);
        return stage(stem.strip_prefix('s')?);
    }
    None
}

/// The stage a task says it is: `Stage 21` in its title, or at the start of its body.
/// A body that only mentions other stages in passing does not count.
pub fn stage_of_task(title: &str, body: &str) -> Option<u32> {
    fn after_stage(s: &str) -> Option<u32> {
        let lower = s.to_ascii_lowercase();
        let mut from = 0;
        while let Some(i) = lower[from..].find("stage ") {
            let at = from + i;
            let word_start = at == 0 || !lower.as_bytes()[at - 1].is_ascii_alphanumeric();
            let rest = &lower[at + 6..];
            let d: String = rest.chars().take_while(char::is_ascii_digit).collect();
            let after = rest[d.len()..].chars().next();
            if word_start
                && (1..=2).contains(&d.len())
                && !after.is_some_and(|c| c.is_alphanumeric())
            {
                return d.parse().ok();
            }
            from = at + 6;
        }
        None
    }
    after_stage(title).or_else(|| {
        let first = body.lines().map(str::trim).find(|l| !l.is_empty())?;
        first
            .to_ascii_lowercase()
            .starts_with("stage ")
            .then(|| after_stage(first))?
    })
}

/// The highest numbers among paths: (stage/exit, decision). Zero when there are none.
pub fn highest<'a>(paths: impl IntoIterator<Item = &'a str>) -> (u32, u32) {
    let (mut s, mut d) = (0, 0);
    for p in paths {
        match parse_path(p) {
            Some(Numbered::Stage(n)) => s = s.max(n),
            Some(Numbered::Decision(n)) => d = d.max(n),
            None => {}
        }
    }
    (s, d)
}

/// What a task has been given.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Mine {
    /// Its stage note and exit script number: its stage, or a reserved one.
    pub exit: Option<u32>,
    pub decision: Option<u32>,
    /// The exit number came from the task naming its stage, not from a reservation.
    pub from_stage: bool,
}

impl Mine {
    pub fn stage_note(&self) -> Option<String> {
        self.exit.map(|n| format!("docs/stages/s{n:02}.md"))
    }
    pub fn exit_script(&self) -> Option<String> {
        self.exit.map(|n| format!("scripts/exit/s{n:02}.sh"))
    }
    pub fn decision_record(&self) -> Option<String> {
        self.decision
            .map(|n| format!("docs/decisions/{n:04}-<slug>.md"))
    }
}

/// The ceiling for each kind's counter.
pub fn ceiling(kind: &str) -> u32 {
    if kind == KIND_EXIT {
        EXIT_CEILING
    } else {
        DECISION_CEILING
    }
}

/// The lines put in every agent prompt.
pub fn prompt_block(m: &Mine) -> String {
    let (Some(note), Some(script)) = (m.stage_note(), m.exit_script()) else {
        return String::new();
    };
    let dec = m
        .decision_record()
        .unwrap_or_else(|| "docs/decisions/NNNN-<slug>.md".into());
    format!(
        "Your numbers were handed out by MASON: your stage note is {note}, your decision record is {dec}, \
         your exit script is {script} (and {script_real}, {script_manual} if you need them). Do not look for \
         the next number yourself; do not take another number. Do not edit the `Status:` block in README.md: \
         put your one-sentence summary in your stage note as a line starting `Summary: ` instead, and \
         MASON builds the status block from the stage notes when your change lands.",
        script_real = script.replace(".sh", "-real.sh"),
        script_manual = script.replace(".sh", "-manual.sh"),
    )
}

/// A file a task added under a number that is not its own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Violation {
    pub path: String,
    pub number: u32,
    /// What it should be called instead, when the task has a number of that kind.
    pub should_use: Option<String>,
    pub why: String,
}

impl std::fmt::Display for Violation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path, self.why)
    }
}

/// Check the files a task added against its numbers and against what the base already
/// has. `base` is every path on the base branch tip.
pub fn check_added(added: &[String], base: &[String], mine: &Mine) -> Vec<Violation> {
    let base_nums: Vec<Numbered> = base.iter().filter_map(|p| parse_path(p)).collect();
    let mut out = vec![];
    for path in added {
        let Some(n) = parse_path(path) else { continue };
        let (number, own, family) = match n {
            Numbered::Stage(x) => (x, mine.exit, "stage note / exit script"),
            Numbered::Decision(x) => (x, mine.decision, "decision record"),
        };
        let on_base = base_nums.contains(&n);
        if own == Some(number) && !on_base {
            continue;
        }
        let should = match n {
            Numbered::Stage(_) => {
                let ext = path.rsplit('/').next().unwrap_or("");
                let suffix = if ext.ends_with("-real.sh") {
                    "-real.sh"
                } else if ext.ends_with("-manual.sh") {
                    "-manual.sh"
                } else if path.starts_with("docs/") {
                    ".md"
                } else {
                    ".sh"
                };
                let dir = if path.starts_with("docs/") {
                    "docs/stages"
                } else {
                    "scripts/exit"
                };
                own.map(|o| format!("{dir}/s{o:02}{suffix}"))
            }
            Numbered::Decision(_) => own.map(|o| {
                let slug = path
                    .rsplit('/')
                    .next()
                    .and_then(|f| f.split_once('-'))
                    .map_or("slug.md", |x| x.1);
                format!("docs/decisions/{o:04}-{slug}")
            }),
        };
        let why = match (&should, on_base) {
            (Some(s), true) => format!(
                "number {number} is already used on the base branch; your {family} number is {}, so name it {s}",
                own.unwrap_or_default()
            ),
            (Some(s), false) => format!(
                "number {number} is not yours; MASON reserved {} for your {family}, so name it {s}",
                own.unwrap_or_default()
            ),
            (None, _) => format!("number {number} is not yours and MASON reserved no {family} number for this task"),
        };
        out.push(Violation {
            path: path.clone(),
            number,
            should_use: should,
            why,
        });
    }
    out
}

/// The text sent back to the agent.
pub fn feedback(v: &[Violation]) -> String {
    let mut s = String::from(
        "Your change adds numbered files that are not under your reserved numbers. MASON hands out these numbers; \
         do not take another. Rename them and commit:\n",
    );
    for x in v {
        s.push_str(&format!("- {x}\n"));
    }
    s
}

// ---- reading the base branch and the store -------------------------------------------

/// Every path on the base tip under the numbered directories. A failed git call is an error:
/// an empty list would let a number be handed out or accepted a second time.
pub fn base_paths(p: &ProjectCfg) -> Result<Vec<String>, git::GitError> {
    git::run(
        &p.path,
        &[
            "ls-tree",
            "-r",
            "--name-only",
            &p.base_ref(),
            "--",
            "docs/stages",
            "docs/decisions",
            "scripts/exit",
        ],
    )
    .map(|s| s.lines().map(String::from).collect())
}

/// Files a branch adds against its base (since the merge base).
pub fn added_files(repo: &Path, base_ref: &str, branch: &str) -> Result<Vec<String>, git::GitError> {
    git::run(
        repo,
        &[
            "diff",
            "--name-only",
            "--diff-filter=A",
            &format!("{base_ref}...{branch}"),
        ],
    )
    .map(|s| s.lines().map(String::from).collect())
}

fn mine_from(task: &Task, rows: &[ReservationRow]) -> Mine {
    let get = |k: &str| rows.iter().find(|r| r.kind == k).map(|r| r.number);
    let stage = stage_of_task(&task.title, &task.body);
    Mine {
        exit: stage.or_else(|| get(KIND_EXIT)),
        decision: get(KIND_DECISION),
        from_stage: stage.is_some(),
    }
}

impl Daemon {
    /// The next exit number a new ad-hoc task would get, or `None` when the range is used
    /// up. Does not reserve anything.
    pub fn adhoc_exit_room(&self, p: &ProjectCfg) -> bool {
        let rows = self.store.all_reservations().unwrap_or_default();
        let top = rows
            .iter()
            .filter(|r| r.kind == KIND_EXIT)
            .map(|r| r.number)
            .max()
            .unwrap_or(0);
        // Advisory (the reservation itself is checked again): if the base cannot be read
        // now, say there is room rather than turn a new task away on a git blip.
        let Ok(paths) = base_paths(p) else {
            return true;
        };
        let (base_top, _) = highest(paths.iter().map(String::as_str));
        let floor = self.cfg.adhoc_exit_start.saturating_sub(1);
        floor.max(top).max(base_top) < EXIT_CEILING
    }

    /// The task's numbers, reserving any it does not have yet (once: a task that comes
    /// back keeps them). A task naming its stage keeps that number and gets only a
    /// decision number.
    pub fn ensure_numbers(&self, task: TaskId) -> Result<Mine, String> {
        let w = self.store.world().map_err(|e| e.to_string())?;
        let t = w.tasks.get(&task).ok_or("no such task")?;
        let p = self.project(t.project).ok_or("no such project")?;
        let have = self.store.reservations(task.0).map_err(|e| e.to_string())?;
        let stage = stage_of_task(&t.title, &t.body);
        let need_exit = stage.is_none() && !have.iter().any(|r| r.kind == KIND_EXIT);
        let need_dec = !have.iter().any(|r| r.kind == KIND_DECISION);
        if need_exit || need_dec {
            let paths = base_paths(p).map_err(|e| format!("cannot read the base's numbers: {e}"))?;
            let (base_exit, base_dec) = highest(paths.iter().map(String::as_str));
            if need_dec {
                self.store
                    .reserve_number(task.0, KIND_DECISION, base_dec, DECISION_CEILING, now_ms())
                    .map_err(|e| e.to_string())?
                    .ok_or("the decision numbers are used up")?;
            }
            if need_exit {
                let floor = base_exit.max(self.cfg.adhoc_exit_start.saturating_sub(1));
                self.store
                    .reserve_number(task.0, KIND_EXIT, floor, EXIT_CEILING, now_ms())
                    .map_err(|e| e.to_string())?
                    .ok_or("the ad-hoc range of stage and exit-script numbers is used up")?;
            }
        }
        let rows = self.store.reservations(task.0).map_err(|e| e.to_string())?;
        Ok(mine_from(t, &rows))
    }

    /// The task's numbers as stored, without reserving.
    pub fn numbers_of(&self, t: &Task) -> Mine {
        let rows = self.store.reservations(t.id.0).unwrap_or_default();
        mine_from(t, &rows)
    }
}

/// One reserved number as shown to the owner (`mason task show`, both monitors).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub kind: String,
    pub number: u32,
    pub released: bool,
    pub path: String,
    /// `reserved`, or `stage` when the task kept the stage its title names.
    pub source: String,
}

impl Daemon {
    /// The numbers to show for a task: what it has, with a cancelled task's marked released.
    pub fn number_lines(&self, t: &Task) -> Vec<Line> {
        let rows = self.store.reservations(t.id.0).unwrap_or_default();
        let mut out = vec![];
        let released = |k: &str| rows.iter().any(|r| r.kind == k && r.released);
        if let Some(n) = stage_of_task(&t.title, &t.body) {
            out.push(Line {
                kind: KIND_EXIT.into(),
                number: n,
                released: t.state == mason_core::State::Canceled,
                path: format!("docs/stages/s{n:02}.md, scripts/exit/s{n:02}.sh"),
                source: "stage".into(),
            });
        } else if let Some(r) = rows.iter().find(|r| r.kind == KIND_EXIT) {
            out.push(Line {
                kind: KIND_EXIT.into(),
                number: r.number,
                released: released(KIND_EXIT),
                path: format!("docs/stages/s{0:02}.md, scripts/exit/s{0:02}.sh", r.number),
                source: "reserved".into(),
            });
        }
        if let Some(r) = rows.iter().find(|r| r.kind == KIND_DECISION) {
            out.push(Line {
                kind: KIND_DECISION.into(),
                number: r.number,
                released: r.released,
                path: format!("docs/decisions/{:04}-<slug>.md", r.number),
                source: "reserved".into(),
            });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(p: &[&str]) -> Vec<String> {
        p.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn names_are_read_by_their_family() {
        assert_eq!(parse_path("docs/stages/s20.md"), Some(Numbered::Stage(20)));
        assert_eq!(
            parse_path("scripts/exit/s40-real.sh"),
            Some(Numbered::Stage(40))
        );
        assert_eq!(
            parse_path("scripts/exit/s06-manual.sh"),
            Some(Numbered::Stage(6))
        );
        assert_eq!(
            parse_path("docs/decisions/0017-x-y.md"),
            Some(Numbered::Decision(17))
        );
        assert_eq!(parse_path("docs/stages/s12-screenshots/a.png"), None);
        assert_eq!(parse_path("scripts/exit/fixtures/s42/x.json"), None);
        assert_eq!(
            parse_path("docs/stages/s123.md"),
            Some(Numbered::Stage(123))
        );
        assert_eq!(
            parse_path("scripts/exit/s100-real.sh"),
            Some(Numbered::Stage(100))
        );
        assert_eq!(parse_path("docs/stages/s1234.md"), None);
        assert_eq!(parse_path("docs/stages/s7.md"), None);
        assert_eq!(parse_path("docs/decisions/17-x.md"), None);
        assert_eq!(
            highest([
                "docs/stages/s42.md",
                "docs/decisions/0020-a.md",
                "README.md"
            ]),
            (42, 20)
        );
    }

    #[test]
    fn a_stage_is_taken_from_the_title_not_from_passing_mentions() {
        assert_eq!(stage_of_task("Stage 21: more", ""), Some(21));
        assert_eq!(stage_of_task("do the thing (stage 7)", ""), Some(7));
        assert_eq!(stage_of_task("Backstage 3", ""), None);
        assert_eq!(stage_of_task("Stage 100", ""), None);
        assert_eq!(
            stage_of_task("Monitor answers", "Stage 21 adds more.\nx"),
            Some(21)
        );
        assert_eq!(
            stage_of_task(
                "MASON hands out numbers",
                "Task #2 was cut when it held Stage 19"
            ),
            None
        );
    }

    #[test]
    fn a_wrong_number_is_named_with_the_right_one() {
        let mine = Mine {
            exit: Some(40),
            decision: Some(18),
            from_stage: false,
        };
        let base = v(&[
            "docs/stages/s20.md",
            "scripts/exit/s20.sh",
            "docs/decisions/0017-a.md",
        ]);
        let ok = v(&[
            "docs/stages/s40.md",
            "scripts/exit/s40.sh",
            "scripts/exit/s40-real.sh",
            "docs/decisions/0018-x.md",
            "src/a.rs",
        ]);
        assert!(check_added(&ok, &base, &mine).is_empty());
        let bad = check_added(
            &v(&[
                "docs/stages/s20.md",
                "docs/decisions/0017-x.md",
                "scripts/exit/s41-real.sh",
            ]),
            &base,
            &mine,
        );
        assert_eq!(bad.len(), 3);
        assert_eq!(bad[0].should_use.as_deref(), Some("docs/stages/s40.md"));
        assert_eq!(
            bad[1].should_use.as_deref(),
            Some("docs/decisions/0018-x.md")
        );
        assert_eq!(
            bad[2].should_use.as_deref(),
            Some("scripts/exit/s40-real.sh")
        );
        assert!(
            bad[0].why.contains("already used on the base") && bad[2].why.contains("not yours")
        );
    }

    #[test]
    fn a_stage_numbered_task_keeps_its_stage() {
        let mine = Mine {
            exit: Some(21),
            decision: Some(18),
            from_stage: true,
        };
        assert!(check_added(
            &v(&["docs/stages/s21.md", "scripts/exit/s21.sh"]),
            &[],
            &mine
        )
        .is_empty());
        let bad = check_added(&v(&["docs/stages/s22.md"]), &[], &mine);
        assert_eq!(bad[0].should_use.as_deref(), Some("docs/stages/s21.md"));
    }

    #[test]
    fn the_prompt_states_the_numbers_and_forbids_taking_another() {
        let b = prompt_block(&Mine {
            exit: Some(40),
            decision: Some(18),
            from_stage: false,
        });
        assert!(b.contains("docs/stages/s40.md") && b.contains("docs/decisions/0018-<slug>.md"));
        assert!(b.contains("scripts/exit/s40.sh") && b.contains("do not take another number"));
    }
}
