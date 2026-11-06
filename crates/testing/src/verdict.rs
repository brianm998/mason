// SPDX-License-Identifier: GPL-3.0-or-later

//! Verdicts proven from output (Part 8).
//!
//! Green requires proof. A run is green only if its parser found at least one
//! executed test, at least the suite's minimum, and no failure, and the
//! command's own exit status agrees. Red is classified before anyone is
//! blamed. The closing verdict of a task ranks worst-first.

use crate::model::Signature;
use crate::results::Results;
use regex::Regex;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    Green,
    /// Nothing failed, and nothing was proven either: fewer tests ran than
    /// required (and never fewer than one). This is red.
    Vacuous {
        executed: usize,
        needed: usize,
    },
    Red {
        failures: Vec<String>,
        why: String,
    },
}

impl Verdict {
    pub fn is_green(&self) -> bool {
        matches!(self, Verdict::Green)
    }
}

/// Judge one run. `exit` is the command's exit status, if it is known (a
/// results file read after the fact has none).
pub fn judge(results: &Results, min_tests: u32, exit: Option<i32>) -> Verdict {
    let needed = (min_tests as usize).max(1);
    let failures = results.failed_names();
    if !failures.is_empty() {
        let why = format!("{} failing", failures.len());
        return Verdict::Red { failures, why };
    }
    if let Some(code) = exit.filter(|c| *c != 0) {
        let why = if results.executed() == 0 {
            format!("exited with status {code} and no test results were found")
        } else {
            format!("exited with status {code} although no test was reported failing")
        };
        return Verdict::Red {
            failures: vec![],
            why,
        };
    }
    if results.executed() < needed {
        return Verdict::Vacuous {
            executed: results.executed(),
            needed,
        };
    }
    Verdict::Green
}

/// Why a red is red. Only `Product` is the task's fault.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RedClass {
    /// Matches a signature of the machine's fault (no device, an expired
    /// certificate). Becomes a wait plus an alert, charged to no task.
    Environmental { signature: String, advice: String },
    /// Every failure passed on an immediate rerun. Logged, and counts toward
    /// quarantine.
    Flaky { tests: Vec<String> },
    /// Every failure is also failing at the merge base. Excused.
    Baseline { tests: Vec<String> },
    /// Every failure is quarantined (with an owner and an unexpired date).
    Quarantined { tests: Vec<String> },
    /// Sent back to development. `excused` and `flaky` name failures that were
    /// set aside and are not the task's problem.
    Product {
        tests: Vec<String>,
        excused: Vec<String>,
        flaky: Vec<String>,
    },
}

pub struct Inputs<'a> {
    /// What failed.
    pub failing: &'a [String],
    /// The run's output, for signatures.
    pub output: &'a str,
    pub signatures: &'a [Signature],
    /// What still failed on an immediate rerun of the failing tests, if one
    /// was done.
    pub rerun_failing: Option<&'a [String]>,
    /// What fails at the merge base, if it was checked.
    pub baseline_failing: Option<&'a [String]>,
    /// Failures excused by an unexpired quarantine.
    pub quarantined: &'a [String],
}

/// Classify a red, in the order that avoids blaming the task for what is not
/// its doing: the machine's fault first, then quarantine, then flakiness, then
/// the baseline, and only then the product.
pub fn classify(i: &Inputs) -> RedClass {
    for s in i.signatures {
        if Regex::new(&s.pattern).is_ok_and(|r| r.is_match(i.output)) {
            return RedClass::Environmental {
                signature: s.name.clone(),
                advice: s.advice.clone(),
            };
        }
    }
    let mut left: Vec<String> = i.failing.to_vec();
    let mut excused: Vec<String> = vec![];
    let mut flaky: Vec<String> = vec![];

    let (q, rest): (Vec<_>, Vec<_>) = left
        .iter()
        .cloned()
        .partition(|t| i.quarantined.contains(t));
    if !q.is_empty() {
        if rest.is_empty() {
            return RedClass::Quarantined { tests: q };
        }
        excused.extend(q);
        left = rest;
    }

    if let Some(again) = i.rerun_failing {
        let (still, passed): (Vec<_>, Vec<_>) =
            left.iter().cloned().partition(|t| again.contains(t));
        if !passed.is_empty() {
            if still.is_empty() {
                return RedClass::Flaky { tests: passed };
            }
            flaky = passed;
            left = still;
        }
    }

    if let Some(base) = i.baseline_failing {
        let (old, new): (Vec<_>, Vec<_>) = left.iter().cloned().partition(|t| base.contains(t));
        if !old.is_empty() {
            if new.is_empty() {
                return RedClass::Baseline { tests: old };
            }
            excused.extend(old);
            left = new;
        }
    }

    RedClass::Product {
        tests: left,
        excused,
        flaky,
    }
}

/// How one suite turned out for one task.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SuiteOutcome {
    /// Green with proof.
    Proven,
    /// Green evidence already existed for this exact tree.
    Reused,
    /// Not selected for this change (the impact map said so).
    NotSelected,
    /// Passed, but only on a rerun.
    Flaky,
    /// Failures excused as baseline or quarantined.
    Excused,
    /// Force-skipped. A debt was recorded.
    Skipped,
    /// Ran but proved nothing.
    Vacuous,
    /// The machine's fault; no verdict was reached.
    Environmental,
    /// Red, and the task's.
    Failed,
}

/// The closing verdict of a task, worst first. `Ord` follows the ranking, so
/// the worst of several is their `min`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Closing {
    /// Something that should have proven the change did not: a suite ran and
    /// proved nothing, or could not run for reasons outside the task.
    Unproven,
    /// Green, but with something set aside: a skipped suite (a debt), an
    /// excused failure, a flaky pass.
    Unverified,
    /// No suite was selected for the change at all.
    Untested,
    /// Every selected suite is green with proof, and nothing was set aside.
    Verified,
}

impl Closing {
    pub fn name(self) -> &'static str {
        match self {
            Closing::Unproven => "unproven",
            Closing::Unverified => "unverified",
            Closing::Untested => "untested",
            Closing::Verified => "verified",
        }
    }

    pub fn from_name(s: &str) -> Option<Closing> {
        [
            Closing::Unproven,
            Closing::Unverified,
            Closing::Untested,
            Closing::Verified,
        ]
        .into_iter()
        .find(|c| c.name() == s)
    }
}

impl SuiteOutcome {
    fn closing(&self) -> Option<Closing> {
        match self {
            SuiteOutcome::Proven | SuiteOutcome::Reused => Some(Closing::Verified),
            SuiteOutcome::NotSelected => None,
            SuiteOutcome::Flaky | SuiteOutcome::Excused | SuiteOutcome::Skipped => {
                Some(Closing::Unverified)
            }
            SuiteOutcome::Vacuous | SuiteOutcome::Environmental | SuiteOutcome::Failed => {
                Some(Closing::Unproven)
            }
        }
    }

    pub fn describe(&self) -> &'static str {
        match self {
            SuiteOutcome::Proven => "green, with proof",
            SuiteOutcome::Reused => "green (evidence for this exact tree)",
            SuiteOutcome::NotSelected => "not selected for this change",
            SuiteOutcome::Flaky => "passed only on a rerun",
            SuiteOutcome::Excused => "failures excused (baseline or quarantine)",
            SuiteOutcome::Skipped => "skipped by force, a debt was recorded",
            SuiteOutcome::Vacuous => "ran but proved nothing",
            SuiteOutcome::Environmental => "could not run: the environment's fault",
            SuiteOutcome::Failed => "red",
        }
    }
}

/// The closing verdict of a set of suite outcomes, and the note that says why.
pub fn closing(outcomes: &[(String, SuiteOutcome)]) -> (Closing, String) {
    let ranked: Vec<Closing> = outcomes.iter().filter_map(|(_, o)| o.closing()).collect();
    let verdict = ranked.into_iter().min().unwrap_or(Closing::Untested);
    let note = if outcomes.is_empty() {
        "no suite is bound to this step".to_string()
    } else {
        outcomes
            .iter()
            .map(|(s, o)| format!("{s}: {}", o.describe()))
            .collect::<Vec<_>>()
            .join("; ")
    };
    (verdict, format!("{}: {note}", verdict.name()))
}

/// A green result remembered per tree hash and suite, so identical code never
/// tests twice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Evidence {
    pub tree: String,
    pub suite: String,
    pub executed: usize,
    pub passed: usize,
    pub at_ms: u64,
    pub task: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::results::Case;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn green_needs_proof() {
        let ran = Results::new(vec![Case::pass("a"), Case::pass("b")]);
        assert_eq!(judge(&ran, 2, Some(0)), Verdict::Green);
        assert_eq!(judge(&ran, 1, None), Verdict::Green);
        // Fewer than the minimum.
        assert_eq!(
            judge(&ran, 3, Some(0)),
            Verdict::Vacuous {
                executed: 2,
                needed: 3
            }
        );
        // Skipped tests prove nothing.
        let skipped = Results::new(vec![Case::skip("a"), Case::skip("b")]);
        assert!(matches!(
            judge(&skipped, 1, Some(0)),
            Verdict::Vacuous { executed: 0, .. }
        ));
        // Nothing at all, even with a minimum of zero.
        assert!(matches!(
            judge(&Results::default(), 0, Some(0)),
            Verdict::Vacuous { needed: 1, .. }
        ));
    }

    #[test]
    fn a_failure_or_a_bad_exit_is_red_whatever_else_is_true() {
        let r = Results::new(vec![Case::pass("a"), Case::fail("b", "x")]);
        assert!(matches!(judge(&r, 1, Some(0)), Verdict::Red { .. }));
        let ok = Results::new(vec![Case::pass("a")]);
        assert!(
            matches!(judge(&ok, 1, Some(2)), Verdict::Red { why, .. } if why.contains("status 2"))
        );
        assert!(matches!(
            judge(&Results::default(), 1, Some(1)),
            Verdict::Red { why, .. } if why.contains("no test results")
        ));
    }

    fn sig() -> Vec<Signature> {
        vec![Signature {
            name: "no-device".into(),
            pattern: "No (devices|simulators) found".into(),
            advice: "plug a device in".into(),
        }]
    }

    fn inputs<'a>(
        failing: &'a [String],
        output: &'a str,
        sigs: &'a [Signature],
        rerun: Option<&'a [String]>,
        base: Option<&'a [String]>,
        q: &'a [String],
    ) -> Inputs<'a> {
        Inputs {
            failing,
            output,
            signatures: sigs,
            rerun_failing: rerun,
            baseline_failing: base,
            quarantined: q,
        }
    }

    #[test]
    fn the_machines_fault_is_never_the_tasks() {
        let f = names(&["t1"]);
        let s = sig();
        let c = classify(&inputs(&f, "boom: No devices found\n", &s, None, None, &[]));
        assert!(matches!(c, RedClass::Environmental { signature, .. } if signature == "no-device"));
        // Even with no failing test named at all.
        let c = classify(&inputs(&[], "No simulators found", &s, None, None, &[]));
        assert!(matches!(c, RedClass::Environmental { .. }));
    }

    #[test]
    fn flaky_baseline_and_product_are_told_apart() {
        let f = names(&["a", "b"]);
        // Both pass on rerun: flaky.
        let none: Vec<String> = vec![];
        assert_eq!(
            classify(&inputs(&f, "", &[], Some(&none), None, &[])),
            RedClass::Flaky { tests: f.clone() }
        );
        // Both still fail, and both fail at the base: baseline.
        assert_eq!(
            classify(&inputs(&f, "", &[], Some(&f), Some(&f), &[])),
            RedClass::Baseline { tests: f.clone() }
        );
        // Both still fail and only one is old: the task owns the other.
        let base = names(&["a"]);
        assert_eq!(
            classify(&inputs(&f, "", &[], Some(&f), Some(&base), &[])),
            RedClass::Product {
                tests: names(&["b"]),
                excused: names(&["a"]),
                flaky: vec![]
            }
        );
        // One flaked, one is real: product, naming only the real one.
        let again = names(&["b"]);
        assert_eq!(
            classify(&inputs(&f, "", &[], Some(&again), None, &[])),
            RedClass::Product {
                tests: names(&["b"]),
                excused: vec![],
                flaky: names(&["a"])
            }
        );
        // No rerun, no baseline: product.
        assert_eq!(
            classify(&inputs(&f, "", &[], None, None, &[])),
            RedClass::Product {
                tests: f.clone(),
                excused: vec![],
                flaky: vec![]
            }
        );
    }

    #[test]
    fn quarantine_excuses_only_what_it_names() {
        let f = names(&["a", "b"]);
        let q = names(&["a", "b", "zzz"]);
        assert_eq!(
            classify(&inputs(&f, "", &[], None, None, &q)),
            RedClass::Quarantined { tests: f.clone() }
        );
        let q = names(&["a"]);
        assert_eq!(
            classify(&inputs(&f, "", &[], None, None, &q)),
            RedClass::Product {
                tests: names(&["b"]),
                excused: names(&["a"]),
                flaky: vec![]
            }
        );
    }

    #[test]
    fn closing_verdicts_rank_worst_first() {
        assert!(Closing::Unproven < Closing::Unverified);
        assert!(Closing::Unverified < Closing::Untested);
        assert!(Closing::Untested < Closing::Verified);
        let o = |v: &[(&str, SuiteOutcome)]| {
            closing(
                &v.iter()
                    .map(|(n, o)| (n.to_string(), o.clone()))
                    .collect::<Vec<_>>(),
            )
            .0
        };
        use SuiteOutcome::*;
        assert_eq!(o(&[("unit", Proven), ("e2e", Reused)]), Closing::Verified);
        assert_eq!(
            o(&[("unit", Proven), ("e2e", Skipped)]),
            Closing::Unverified
        );
        assert_eq!(o(&[("unit", Flaky)]), Closing::Unverified);
        assert_eq!(o(&[("unit", Proven), ("e2e", Vacuous)]), Closing::Unproven);
        assert_eq!(
            o(&[("unit", Skipped), ("e2e", Environmental)]),
            Closing::Unproven
        );
        assert_eq!(o(&[("unit", NotSelected)]), Closing::Untested);
        assert_eq!(o(&[]), Closing::Untested);
        // A suite that was not selected does not dilute the others.
        assert_eq!(
            o(&[("unit", Proven), ("e2e", NotSelected)]),
            Closing::Verified
        );
        let (c, note) = closing(&[("unit".into(), Proven), ("e2e".into(), Skipped)]);
        assert_eq!(c, Closing::Unverified);
        assert_eq!(
            note,
            "unverified: unit: green, with proof; e2e: skipped by force, a debt was recorded"
        );
        for c in [
            Closing::Unproven,
            Closing::Unverified,
            Closing::Untested,
            Closing::Verified,
        ] {
            assert_eq!(Closing::from_name(c.name()), Some(c));
        }
    }
}
