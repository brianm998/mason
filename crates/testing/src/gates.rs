// SPDX-License-Identifier: GPL-3.0-or-later

//! The optional gates from the library (Part 8): negative control,
//! assertion-weakening scan, acceptance table, and quarantine, which always
//! has an owner and an expiry.

use crate::glob;
use crate::results::{Results, Status};
use std::collections::BTreeMap;

// ---- negative control ------------------------------------------------------

/// Tests that exist in the change's results but not in the base's.
pub fn new_tests(head: &Results, base_names: &[String]) -> Vec<String> {
    head.names()
        .into_iter()
        .filter(|n| !base_names.contains(n))
        .collect()
}

/// New tests must fail against the old code. Returns the ones that do not:
/// a test that passes without the change tests nothing about the change.
/// `at_base` is what the suite reported when the new tests were run against
/// the old code; a new test missing from it did not run, which also proves
/// nothing, so it is reported too.
pub fn negative_control(new: &[String], at_base: &Results) -> Vec<String> {
    new.iter()
        .filter(|t| {
            !at_base
                .cases
                .iter()
                .any(|c| &c.name == *t && matches!(c.status, Status::Fail(_)))
        })
        .cloned()
        .collect()
}

// ---- assertion-weakening scan ----------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Weakening {
    pub file: String,
    pub what: String,
}

const ASSERTION: &[&str] = &[
    "assert",
    "expect(",
    "#expect",
    "XCTAssert",
    "XCTFail",
    "should.",
    ".should",
    "require(",
    "verify(",
    "toEqual",
    "toBe(",
    "toThrow",
];

const DISABLING: &[&str] = &[
    "#[ignore",
    "@Ignore",
    "@Disabled",
    "@pytest.mark.skip",
    "pytest.skip(",
    "pytest.mark.xfail",
    ".skip(",
    "xit(",
    "xdescribe(",
    "t.Skip(",
    "XCTSkip",
    "assert!(true)",
    "assertTrue(true)",
    "assert True",
    "expect(true)",
];

fn is_assertion(l: &str) -> bool {
    ASSERTION.iter().any(|a| l.contains(a))
}

/// Whether a path looks like a test file: one of the suite's `test_paths`
/// globs, or a path with `test` or `spec` in it.
pub fn is_test_path(path: &str, test_paths: &[String]) -> bool {
    if !test_paths.is_empty() {
        return test_paths.iter().any(|g| glob::matches(g, path));
    }
    let p = path.to_lowercase();
    p.contains("test") || p.contains("spec")
}

/// Look at a unified diff for assertions removed or tests disabled in test
/// files. A removed assertion is only a finding when the file gains fewer
/// assertions than it loses.
pub fn weakening(diff: &str, test_paths: &[String]) -> Vec<Weakening> {
    let mut out = vec![];
    let mut file = String::new();
    let (mut removed, mut added) = (0usize, 0usize);
    let flush = |file: &str, removed: usize, added: usize, out: &mut Vec<Weakening>| {
        if !file.is_empty() && removed > added {
            out.push(Weakening {
                file: file.to_string(),
                what: format!("removes {} assertion(s) and adds {added}", removed),
            });
        }
    };
    for l in diff.lines() {
        if let Some(p) = l.strip_prefix("+++ b/") {
            flush(&file, removed, added, &mut out);
            file = p.to_string();
            (removed, added) = (0, 0);
            continue;
        }
        if l.starts_with("+++ ") || l.starts_with("--- ") {
            continue;
        }
        if file.is_empty() || !is_test_path(&file, test_paths) {
            continue;
        }
        if let Some(body) = l.strip_prefix('-') {
            if is_assertion(body) {
                removed += 1;
            }
        } else if let Some(body) = l.strip_prefix('+') {
            if is_assertion(body) {
                added += 1;
            }
            if let Some(d) = DISABLING.iter().find(|d| body.contains(**d)) {
                out.push(Weakening {
                    file: file.clone(),
                    what: format!("adds `{d}`"),
                });
            }
        }
    }
    flush(&file, removed, added, &mut out);
    out
}

/// A change that weakens assertions must say so: its explanation has to
/// mention assertions and be more than a few words.
pub fn explains_weakening(explanation: &str) -> bool {
    explanation.to_lowercase().contains("assert") && explanation.split_whitespace().count() >= 6
}

// ---- acceptance table ------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Gap {
    /// No test is named for this criterion.
    Unmapped(String),
    /// A named test did not run.
    Missing { criterion: String, test: String },
    /// A named test ran and did not pass.
    NotPassing { criterion: String, test: String },
}

/// Read `{"criterion": ["test name", ...]}`.
pub fn parse_acceptance(text: &str) -> Result<BTreeMap<String, Vec<String>>, String> {
    let v: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("not a JSON object: {e}"))?;
    let obj = v.as_object().ok_or("expected a JSON object")?;
    let mut out = BTreeMap::new();
    for (k, v) in obj {
        let tests = v
            .as_array()
            .ok_or_else(|| format!("`{k}` must list test names"))?
            .iter()
            .map(|t| {
                t.as_str()
                    .map(String::from)
                    .ok_or_else(|| format!("`{k}` must list test names"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        out.insert(k.clone(), tests);
    }
    Ok(out)
}

/// Each criterion in the plan needs a test that ran and passed.
pub fn acceptance_table(
    criteria: &[String],
    mapping: &BTreeMap<String, Vec<String>>,
    results: &Results,
) -> Vec<Gap> {
    let mut gaps = vec![];
    for c in criteria {
        let tests = mapping.get(c).filter(|t| !t.is_empty());
        let Some(tests) = tests else {
            gaps.push(Gap::Unmapped(c.clone()));
            continue;
        };
        for t in tests {
            match results.cases.iter().find(|r| &r.name == t) {
                None => gaps.push(Gap::Missing {
                    criterion: c.clone(),
                    test: t.clone(),
                }),
                Some(r) if r.status != Status::Pass => gaps.push(Gap::NotPassing {
                    criterion: c.clone(),
                    test: t.clone(),
                }),
                Some(_) => {}
            }
        }
    }
    gaps
}

// ---- quarantine ------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Quarantine {
    pub suite: String,
    pub test: String,
    pub owner: String,
    pub expires_at_ms: u64,
    pub reason: String,
}

/// Quarantine is never open-ended: it needs an owner and a future expiry no
/// more than `max_days` away.
pub fn check_quarantine(q: &Quarantine, now_ms: u64, max_days: u64) -> Result<(), String> {
    if q.owner.trim().is_empty() {
        return Err("a quarantine needs an owner".into());
    }
    if q.expires_at_ms <= now_ms {
        return Err("a quarantine needs an expiry in the future".into());
    }
    if q.expires_at_ms > now_ms + max_days * 86_400_000 {
        return Err(format!("a quarantine may last at most {max_days} days"));
    }
    Ok(())
}

pub fn active(entries: &[Quarantine], suite: &str, now_ms: u64) -> Vec<String> {
    entries
        .iter()
        .filter(|q| q.suite == suite && q.expires_at_ms > now_ms)
        .map(|q| q.test.clone())
        .collect()
}

/// Tests that flaked at least `threshold` times within `window_ms`: candidates
/// for quarantine. `log` is `(suite, test, at_ms)`.
pub fn quarantine_candidates(
    log: &[(String, String, u64)],
    now_ms: u64,
    window_ms: u64,
    threshold: usize,
) -> Vec<(String, String)> {
    let mut counts: BTreeMap<(String, String), usize> = BTreeMap::new();
    for (s, t, at) in log {
        if now_ms.saturating_sub(*at) <= window_ms {
            *counts.entry((s.clone(), t.clone())).or_insert(0) += 1;
        }
    }
    counts
        .into_iter()
        .filter(|(_, n)| *n >= threshold)
        .map(|(k, _)| k)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::results::Case;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn a_new_test_that_passes_without_the_change_is_caught() {
        let head = Results::new(vec![
            Case::pass("old"),
            Case::pass("new_a"),
            Case::pass("new_b"),
        ]);
        let new = new_tests(&head, &s(&["old"]));
        assert_eq!(new, s(&["new_a", "new_b"]));
        let at_base = Results::new(vec![
            Case::fail("new_a", "no such function"),
            Case::pass("new_b"),
        ]);
        assert_eq!(negative_control(&new, &at_base), s(&["new_b"]));
        // Missing at base did not run: also not proof.
        assert_eq!(negative_control(&s(&["ghost"]), &at_base), s(&["ghost"]));
        assert!(negative_control(&s(&["new_a"]), &at_base).is_empty());
    }

    const DIFF: &str = "\
diff --git a/src/lib.rs b/src/lib.rs
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,3 +1,3 @@
-assert_eq!(1, 1);
+assert_eq!(2, 2);
diff --git a/tests/calc_test.py b/tests/calc_test.py
--- a/tests/calc_test.py
+++ b/tests/calc_test.py
@@ -1,6 +1,4 @@
-    assert add(1, 2) == 3
-    assert add(2, 2) == 4
-    assert sub(2, 1) == 1
+    assert add(1, 2) == 3
+@pytest.mark.skip
";

    #[test]
    fn removed_assertions_and_disabled_tests_are_found_in_test_files_only() {
        let f = weakening(DIFF, &[]);
        assert_eq!(f.len(), 2, "{f:?}");
        assert!(f.iter().all(|w| w.file == "tests/calc_test.py"));
        assert!(f
            .iter()
            .any(|w| w.what.contains("removes 3 assertion(s) and adds 1")));
        assert!(f.iter().any(|w| w.what.contains("@pytest.mark.skip")));
        // `src/lib.rs` changed an assertion but is not a test file here.
        let only_src = weakening(DIFF, &s(&["src/**"]));
        assert_eq!(
            only_src.len(),
            0,
            "{only_src:?}: the declared test paths win"
        );
        // Replacing an assertion with an assertion is fine.
        let same = "+++ b/t/a_test.rs\n-assert!(a);\n+assert!(b);\n";
        assert!(weakening(same, &[]).is_empty());
    }

    #[test]
    fn a_weakening_must_explain_itself() {
        assert!(!explains_weakening("fixed it"));
        assert!(!explains_weakening(
            "this change makes the code faster and nicer overall"
        ));
        assert!(explains_weakening(
            "Loosened the assertion on the timestamp because the clock is not exact"
        ));
    }

    #[test]
    fn the_acceptance_table_wants_evidence_for_every_criterion() {
        let r = Results::new(vec![
            Case::pass("t1"),
            Case::fail("t2", "x"),
            Case::skip("t3"),
        ]);
        let m =
            parse_acceptance(r#"{"a": ["t1"], "b": ["t2"], "c": ["t3"], "d": ["nope"], "e": []}"#)
                .unwrap();
        let crit = s(&["a", "b", "c", "d", "e", "f"]);
        let gaps = acceptance_table(&crit, &m, &r);
        assert_eq!(
            gaps,
            vec![
                Gap::NotPassing {
                    criterion: "b".into(),
                    test: "t2".into()
                },
                Gap::NotPassing {
                    criterion: "c".into(),
                    test: "t3".into()
                },
                Gap::Missing {
                    criterion: "d".into(),
                    test: "nope".into()
                },
                Gap::Unmapped("e".into()),
                Gap::Unmapped("f".into()),
            ]
        );
        assert!(parse_acceptance("[1]").is_err());
        assert!(parse_acceptance(r#"{"a": "t1"}"#).is_err());
    }

    #[test]
    fn quarantine_always_has_an_owner_and_an_expiry() {
        let day = 86_400_000;
        let q = |owner: &str, exp: u64| Quarantine {
            suite: "unit".into(),
            test: "t".into(),
            owner: owner.into(),
            expires_at_ms: exp,
            reason: "flaky".into(),
        };
        assert!(check_quarantine(&q("brian", 10 * day), 0, 30).is_ok());
        assert!(check_quarantine(&q("", 10 * day), 0, 30).is_err());
        assert!(check_quarantine(&q("brian", 0), 0, 30).is_err());
        assert!(check_quarantine(&q("brian", 31 * day), 0, 30).is_err());
        let list = [q("b", 5), q("b", 50)];
        assert_eq!(active(&list, "unit", 10), s(&["t"]));
        assert!(active(&list, "unit", 60).is_empty());
        assert!(active(&list, "other", 10).is_empty());
    }

    #[test]
    fn repeated_flakes_become_quarantine_candidates() {
        let log: Vec<(String, String, u64)> = vec![
            ("u".into(), "a".into(), 100),
            ("u".into(), "a".into(), 200),
            ("u".into(), "a".into(), 300),
            ("u".into(), "b".into(), 300),
            ("u".into(), "a".into(), 1),
        ];
        assert_eq!(
            quarantine_candidates(&log, 400, 350, 3),
            vec![("u".to_string(), "a".to_string())]
        );
        assert!(quarantine_candidates(&log, 400, 150, 3).is_empty());
    }
}
