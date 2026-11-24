// SPDX-License-Identifier: GPL-3.0-or-later

//! Lifting known-red ledger entries (source `base`) that the base has since proven green.
//!
//! An entry is lifted when a run on the base executes the test and it passes (`lift_proven`),
//! but that never happens for a pseudo-test that stands for a broken build or suite
//! (`** BUILD FAILED **`, `build`), and it can be slow for a named test when nothing runs that
//! suite again. The baselines table records what each suite did at each base commit, so the
//! sweep reads it: a baseline recorded after the entry was last seen red, at a commit that
//! descends from the one where the entry began, is the proof. No proof, no lift.
//!
//! A `<suite>~ledger` entry is made by the post-landing re-run of a suite's ledger tests
//! (`suites::ledger_run`); it follows its base suite, so both names' baselines are read.
//!
//! Base entries are not in the set `knownred::known_for` skips or excuses (that set is the
//! owner's and the automatic flaky quarantines), so named tests keep running in the base runs
//! that call `lift_proven` and no separate probe is needed.

use crate::daemon::Daemon;
use mason_landing::git;
use mason_store::{BaseProfile, LedgerRow, Store};
use std::sync::Arc;

/// The suffix of the suite name the post-landing ledger run uses.
const LEDGER_SUFFIX: &str = "~ledger";

/// The suite a `~ledger` name stands for; any other name is itself.
pub fn base_suite(suite: &str) -> &str {
    suite.strip_suffix(LEDGER_SUFFIX).unwrap_or(suite)
}

/// A name that stands for "the build or suite failed as a whole" rather than for one test: the
/// fixed `build`, a captured banner such as `** BUILD FAILED **`, or a compiler location
/// (`path:line`). None of these ever appears as a passing test case.
pub fn is_pseudo(test: &str) -> bool {
    if test == "build" {
        return true;
    }
    if test.starts_with("** ") && test.ends_with(" **") {
        return true;
    }
    !test.chars().any(char::is_whitespace)
        && test.rsplit_once(':').is_some_and(|(head, line)| {
            !head.is_empty()
                && !head.ends_with(':')
                && !line.is_empty()
                && line.bytes().all(|b| b.is_ascii_digit())
        })
}

/// What made a lift lawful: the baseline's suite and base commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proof {
    pub suite: String,
    pub base: String,
}

/// A recorded baseline: `(suite, base, at_ms, profile, complete)`. A row that is not complete
/// has only `failing`, `names` and `executed`; its other fields are defaults.
pub type Recorded = (String, String, u64, BaseProfile, bool);

/// The baseline that proves `row` green now, if any. `descends(first_base, base)` says whether
/// `base` is `first_base` or a descendant of it. `baselines` is in any order.
pub fn proof_for(
    row: &LedgerRow,
    baselines: &[Recorded],
    descends: &dyn Fn(&str, &str) -> bool,
) -> Option<Proof> {
    if row.source != "base" {
        return None;
    }
    let pseudo = is_pseudo(&row.test);
    let mut newest_first: Vec<&Recorded> = baselines.iter().collect();
    newest_first.sort_by_key(|b| std::cmp::Reverse(b.2));
    newest_first
        .into_iter()
        .filter(|(_, _, at, _, _)| *at >= row.last_seen_ms)
        .find(|(_, base, _, p, complete)| {
            // A run that built nothing or ran nothing proves nothing. The extra fields are
            // only meaningful on a complete row.
            let sound = p.executed > 0 && (!complete || (p.build_error.is_none() && !p.vacuous));
            let green = if pseudo {
                p.failing.is_empty() && (!complete || !p.red)
            } else {
                p.names.contains(&row.test) && !p.failing.contains(&row.test)
            };
            sound && green && (row.first_base.is_empty() || descends(&row.first_base, base))
        })
        .map(|(suite, base, _, _, _)| Proof {
            suite: suite.clone(),
            base: base.clone(),
        })
}

/// Lift every base entry of the ledger that the recorded baselines prove green. Returns
/// `(suite, test, proof)` for each one lifted.
pub fn lift_proven_by_baselines(
    store: &Store,
    descends: &dyn Fn(&str, &str) -> bool,
) -> Vec<(String, String, Proof)> {
    let mut lifted = vec![];
    for row in store.ledger().unwrap_or_default() {
        if row.source != "base" {
            continue;
        }
        let mut names = vec![row.suite.clone()];
        let base = base_suite(&row.suite).to_string();
        if base != row.suite {
            names.push(base);
        }
        let mut recorded: Vec<Recorded> = vec![];
        for n in &names {
            for (b, at, p, c) in store.baseline_profiles(n).unwrap_or_default() {
                recorded.push((n.clone(), b, at, p, c));
            }
        }
        let Some(proof) = proof_for(&row, &recorded, descends) else {
            continue;
        };
        if store
            .ledger_lift(&row.suite, std::slice::from_ref(&row.test))
            .is_ok_and(|l| !l.is_empty())
        {
            lifted.push((row.suite, row.test, proof));
        }
    }
    lifted
}

/// Called from the sweep: lift the base entries the baselines prove green.
pub fn lift_by_baselines(d: &Arc<Daemon>) {
    let repos: Vec<std::path::PathBuf> = d.projects().values().map(|p| p.path.clone()).collect();
    let descends = |first: &str, base: &str| {
        first == base || repos.iter().any(|r| git::is_ancestor(r, first, base))
    };
    for (suite, test, p) in lift_proven_by_baselines(&d.store, &descends) {
        tracing::info!(
            %suite, %test, base = %p.base, baseline_suite = %p.suite,
            "known-red entry lifted: the baseline of {} at {} is green",
            p.suite, p.base
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, Store) {
        let d = tempfile::tempdir().unwrap();
        let s = Store::open(d.path().join("s.db")).unwrap();
        (d, s)
    }

    fn green() -> BaseProfile {
        BaseProfile {
            executed: 10,
            names: vec!["a::ok".into(), "a::was_red".into()],
            ..Default::default()
        }
    }

    /// Commits are numbers; a base descends from `first` when it is not smaller.
    fn by_number(first: &str, base: &str) -> bool {
        first.parse::<u32>().unwrap() <= base.parse::<u32>().unwrap()
    }

    fn seen(s: &Store, suite: &str, test: &str, base: &str, at: u64) {
        s.ledger_seen_red(suite, test, "base", base, at).unwrap();
    }

    #[test]
    fn a_build_pseudo_entry_is_lifted_by_a_green_baseline_at_a_later_base_only() {
        let (_d, s) = store();
        seen(&s, "build-gui", "** BUILD FAILED **", "5", 1_000);
        // The only green baseline is at an older base: it proves nothing about the entry.
        s.put_baseline_profile("3", "build-gui", &green(), 2_000)
            .unwrap();
        assert!(lift_proven_by_baselines(&s, &by_number).is_empty());
        assert_eq!(s.ledger().unwrap().len(), 1);
        // One recorded before the entry was last seen red proves nothing either.
        s.put_baseline_profile("9", "build-gui", &green(), 500)
            .unwrap();
        assert!(lift_proven_by_baselines(&s, &by_number).is_empty());
        // A red baseline at a later base does not lift it.
        let red = BaseProfile {
            red: true,
            build_error: Some("boom".into()),
            ..green()
        };
        s.put_baseline_profile("8", "build-gui", &red, 3_000)
            .unwrap();
        assert!(lift_proven_by_baselines(&s, &by_number).is_empty());
        // A vacuous green does not either.
        let vacuous = BaseProfile {
            vacuous: true,
            ..green()
        };
        s.put_baseline_profile("7", "build-gui", &vacuous, 3_500)
            .unwrap();
        assert!(lift_proven_by_baselines(&s, &by_number).is_empty());
        // A green one at a later base does.
        s.put_baseline_profile("6", "build-gui", &green(), 4_000)
            .unwrap();
        let l = lift_proven_by_baselines(&s, &by_number);
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].2.base, "6");
        assert!(s.ledger().unwrap().is_empty());
    }

    #[test]
    fn a_named_test_is_lifted_only_when_a_later_baseline_executed_and_passed_it() {
        let (_d, s) = store();
        seen(&s, "cargo", "a::was_red", "5", 1_000);
        seen(&s, "cargo", "a::never_ran", "5", 1_000);
        seen(&s, "cargo", "a::still_red", "5", 1_000);
        let p = BaseProfile {
            failing: vec!["a::still_red".into()],
            names: vec!["a::ok".into(), "a::was_red".into(), "a::still_red".into()],
            red: true,
            ..green()
        };
        s.put_baseline_profile("6", "cargo", &p, 2_000).unwrap();
        let l = lift_proven_by_baselines(&s, &by_number);
        assert_eq!(l.len(), 1, "{l:?}");
        assert_eq!(l[0].1, "a::was_red");
        let left: Vec<String> = s.ledger().unwrap().into_iter().map(|r| r.test).collect();
        assert_eq!(left, ["a::never_ran", "a::still_red"]);
    }

    fn put_plain(s: &Store, base: &str, suite: &str, p: &BaseProfile, at: u64) {
        s.put_baseline(base, suite, &p.failing, &p.names, p.executed, at)
            .unwrap();
    }

    #[test]
    fn a_non_complete_baseline_is_judged_from_its_failing_names_and_executed() {
        let (_d, s) = store();
        seen(&s, "build-gui", "** BUILD FAILED **", "5", 1_000);
        seen(&s, "cargo", "a::was_red", "5", 1_000);
        seen(&s, "cargo", "a::never_ran", "5", 1_000);
        seen(&s, "other", "build", "5", 1_000);
        // Failures listed: the build is not proven green.
        let bad = BaseProfile {
            failing: vec!["x".into()],
            ..green()
        };
        put_plain(&s, "6", "other", &bad, 2_000);
        assert!(lift_proven_by_baselines(&s, &by_number).is_empty());
        // Nothing executed proves nothing.
        let none = BaseProfile {
            executed: 0,
            ..green()
        };
        put_plain(&s, "6", "build-gui", &none, 2_000);
        assert!(lift_proven_by_baselines(&s, &by_number).is_empty());
        // Green rows lift the pseudo entry and the named test that ran.
        let ok = BaseProfile {
            executed: 1,
            names: vec!["BUILD-build-gui-OK".into(), "a::was_red".into()],
            ..Default::default()
        };
        put_plain(&s, "7", "build-gui", &ok, 3_000);
        put_plain(&s, "7", "cargo", &ok, 3_000);
        let l = lift_proven_by_baselines(&s, &by_number);
        assert_eq!(l.len(), 2, "{l:?}");
        let left: Vec<String> = s.ledger().unwrap().into_iter().map(|r| r.test).collect();
        assert_eq!(left, ["a::never_ran", "build"]);
    }

    #[test]
    fn a_named_test_is_kept_when_the_passing_baseline_is_at_an_older_base() {
        let (_d, s) = store();
        seen(&s, "cargo", "a::was_red", "5", 1_000);
        s.put_baseline_profile("4", "cargo", &green(), 2_000)
            .unwrap();
        assert!(lift_proven_by_baselines(&s, &by_number).is_empty());
    }

    #[test]
    fn a_ledger_suite_entry_follows_its_base_suite() {
        let (_d, s) = store();
        seen(&s, "swift-clients~ledger", "-[T testIt]", "5", 1_000);
        seen(&s, "build-gui~ledger", "** BUILD FAILED **", "5", 1_000);
        let mut p = green();
        p.names.push("-[T testIt]".into());
        s.put_baseline_profile("6", "swift-clients", &p, 2_000)
            .unwrap();
        s.put_baseline_profile("6", "build-gui", &green(), 2_000)
            .unwrap();
        let l = lift_proven_by_baselines(&s, &by_number);
        assert_eq!(l.len(), 2, "{l:?}");
        assert!(s.ledger().unwrap().is_empty());
    }

    #[test]
    fn owner_entries_are_never_lifted_here() {
        let (_d, s) = store();
        s.put_quarantine("cargo", "a::was_red", "brian", i64::MAX as u64, "mine", 1)
            .unwrap();
        s.put_baseline_profile("6", "cargo", &green(), 2_000)
            .unwrap();
        assert!(lift_proven_by_baselines(&s, &by_number).is_empty());
        assert_eq!(s.ledger().unwrap().len(), 1);
    }

    #[test]
    fn pseudo_names_are_told_from_test_names() {
        for n in ["build", "** BUILD FAILED **", "src/View.swift:42"] {
            assert!(is_pseudo(n), "{n}");
        }
        for n in [
            "a::b::c",
            "-[T testIt]",
            "masons_own_impact_map",
            "mod::t:2x",
        ] {
            assert!(!is_pseudo(n), "{n}");
        }
        assert_eq!(base_suite("build-gui~ledger"), "build-gui");
        assert_eq!(base_suite("build-gui"), "build-gui");
    }
}
