// SPDX-License-Identifier: GPL-3.0-or-later

//! A red landing batch compared with the base branch's own CI (s174, decision 0169).
//!
//! A batch whose CI job fails the same tests that already fail on the base branch did not
//! break them: nobody is blamed and the batch waits. Where the same tests fail with a
//! different assertion text, the owner decides. A test or job that is green on the base is
//! the batch's, as before. No I/O: the logs are parameters.

use std::collections::BTreeMap;

/// How the base branch's latest finished run of the same workflow stood for one job.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Base {
    /// The base could not be read, has no such run or job: nothing is excused.
    Unknown,
    /// The job is green on the base.
    Green,
    /// The job is red on the base, with the end of its log when it could be read.
    Red(Option<String>),
}

/// One failed job of the batch's CI run and what the base says of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Job {
    pub workflow: String,
    pub job: String,
    pub log: Option<String>,
    pub base: Base,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Something fails that is not red on the base: the batch is blamed, as before.
    Blame,
    /// Every failing test also fails on the base with the same text: blame nobody.
    RedOnBase(String),
    /// The same tests fail, but not with the same text: blame nobody, ask the owner.
    Differs(String),
}

/// Drop a GitHub log line's timestamp (`2026-11-11T12:30:00.1234567Z `).
fn strip_stamp(line: &str) -> &str {
    let b = line.as_bytes();
    if b.len() > 21 && b[4] == b'-' && b[10] == b'T' {
        if let Some(i) = line.find("Z ") {
            if i < 32 {
                return &line[i + 2..];
            }
        }
    }
    line
}

/// The tests a log says failed, each with its assertion text (empty when there is none).
/// Reads pytest's short summary (`FAILED path::test - text`) and cargo's
/// (`test name ... FAILED`, text from its `---- name stdout ----` block).
pub fn failing_tests(log: &str) -> BTreeMap<String, String> {
    let lines: Vec<&str> = log.lines().map(|l| strip_stamp(l).trim_end()).collect();
    let mut out = BTreeMap::new();
    for l in &lines {
        let t = l.trim_start();
        if let Some(rest) = t
            .strip_prefix("FAILED ")
            .or_else(|| t.strip_prefix("ERROR "))
        {
            let (id, text) = match rest.split_once(" - ") {
                Some((a, b)) => (a.trim(), b.trim()),
                None => (rest.trim(), ""),
            };
            if !id.is_empty() && !id.contains(' ') {
                out.insert(id.to_string(), text.to_string());
            }
        } else if let Some(name) = t
            .strip_prefix("test ")
            .and_then(|r| r.strip_suffix(" ... FAILED"))
        {
            let head = format!("---- {name} stdout ----");
            let text = lines
                .iter()
                .position(|x| x.trim() == head)
                .map(|at| {
                    lines[at + 1..]
                        .iter()
                        .take_while(|x| !x.trim().is_empty() && !x.starts_with("----"))
                        .map(|x| x.trim())
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default();
            out.insert(name.trim().to_string(), text);
        }
    }
    out
}

/// What to do with a red batch, given its failed jobs and the base's state of each.
pub fn judge(jobs: &[Job]) -> Verdict {
    if jobs.is_empty() {
        return Verdict::Blame;
    }
    let mut differs: Vec<String> = vec![];
    for j in jobs {
        let Base::Red(Some(base_log)) = &j.base else {
            return Verdict::Blame;
        };
        let Some(log) = &j.log else {
            return Verdict::Blame;
        };
        let cand = failing_tests(log);
        let base = failing_tests(base_log);
        if cand.is_empty() {
            return Verdict::Blame;
        }
        for (t, text) in &cand {
            match base.get(t) {
                None => return Verdict::Blame,
                Some(b) if b != text => differs.push(format!(
                    "{}/{}: {t}: batch says {text:?}, the base says {b:?}",
                    j.workflow, j.job
                )),
                Some(_) => {}
            }
        }
    }
    if differs.is_empty() {
        let names: Vec<String> = jobs
            .iter()
            .map(|j| format!("{}/{}", j.workflow, j.job))
            .collect();
        Verdict::RedOnBase(names.join(", "))
    } else {
        Verdict::Differs(differs.join("; "))
    }
}

/// The last `n` lines of a log for a task's feedback, without timestamps.
pub fn feedback_lines(log: &str, n: usize) -> String {
    let all: Vec<&str> = log.lines().map(strip_stamp).collect();
    all[all.len().saturating_sub(n)..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    const PY_BASE: &str = "2026-11-11T10:00:00.1234567Z FAILED pipeline/tests/test_impact.py::test_no_top_level_source_directory_is_unmapped - AssertionError: ['.mason']\n2026-11-11T10:00:01.0000000Z 1 failed, 40 passed";
    const PY_BATCH: &str = "FAILED pipeline/tests/test_impact.py::test_no_top_level_source_directory_is_unmapped - AssertionError: ['.mason', 'scripts']";

    fn job(log: &str, base: Base) -> Job {
        Job {
            workflow: "pr-gate".into(),
            job: "static checks".into(),
            log: Some(log.into()),
            base,
        }
    }

    #[test]
    fn reads_pytest_and_cargo_failures() {
        let m = failing_tests(PY_BASE);
        assert_eq!(m.len(), 1);
        assert!(m.values().next().unwrap().ends_with("['.mason']"));
        let c = failing_tests(
            "test a::b ... FAILED\ntest a::c ... ok\n\n---- a::b stdout ----\nthread 'a::b' panicked at x.rs:1:1:\nboom\n\nfailures:",
        );
        assert_eq!(c["a::b"], "thread 'a::b' panicked at x.rs:1:1:\nboom");
        assert_eq!(c.len(), 1);
    }

    #[test]
    fn same_tests_same_text_is_red_on_base() {
        let v = judge(&[job(PY_BASE, Base::Red(Some(PY_BASE.into())))]);
        assert_eq!(v, Verdict::RedOnBase("pr-gate/static checks".into()));
    }

    #[test]
    fn same_test_other_text_is_held_for_the_owner() {
        let v = judge(&[job(PY_BATCH, Base::Red(Some(PY_BASE.into())))]);
        let Verdict::Differs(t) = v else {
            panic!("{v:?}")
        };
        assert!(
            t.contains("['.mason', 'scripts']") && t.contains("['.mason']"),
            "{t}"
        );
    }

    #[test]
    fn a_test_green_on_base_or_unknown_base_blames() {
        let other = "FAILED t.py::test_new - assert 0";
        assert_eq!(
            judge(&[job(other, Base::Red(Some(PY_BASE.into())))]),
            Verdict::Blame
        );
        assert_eq!(judge(&[job(PY_BASE, Base::Green)]), Verdict::Blame);
        assert_eq!(judge(&[job(PY_BASE, Base::Unknown)]), Verdict::Blame);
        assert_eq!(judge(&[job(PY_BASE, Base::Red(None))]), Verdict::Blame);
        // A failure with no test names is not excused by a red base.
        assert_eq!(
            judge(&[job("error: boom", Base::Red(Some(PY_BASE.into())))]),
            Verdict::Blame
        );
        // One job excused, another not: blamed.
        assert_eq!(
            judge(&[
                job(PY_BASE, Base::Red(Some(PY_BASE.into()))),
                job(PY_BASE, Base::Green)
            ]),
            Verdict::Blame
        );
        assert_eq!(judge(&[]), Verdict::Blame);
    }

    #[test]
    fn feedback_lines_are_the_tail_without_stamps() {
        let t = feedback_lines("2026-11-11T10:00:00.1234567Z a\nb\nc", 2);
        assert_eq!(t, "b\nc");
    }
}
