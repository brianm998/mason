// SPDX-License-Identifier: GPL-3.0-or-later

//! Protection against weakening its own gates (Part 13). A change to MASON's own
//! repository that touches gates, guards, budgets, self-change rules, verdict
//! logic or trust stops for the owner; everything else lands freely. A scan
//! also reads the diff for *semantic* weakening anywhere: a removed assertion
//! or test, a raised limit, a lowered floor, an ignored test, a broadened skip.
//!
//! The rules are compiled into the running daemon. A change cannot switch off
//! the check on itself: its own copy of these rules is not consulted until it
//! is the running version, and it cannot take effect without passing this one
//! (this file is on the list).

use mason_adapter_api::webhook::{hex, sha256};
use std::collections::BTreeMap;

/// Paths whose change always stops for the owner, and why.
pub const PROTECTED: &[(&str, &str)] = &[
    // gates: what must pass before anything lands
    (
        ".mason/**",
        "gates: the pipeline definition and landing rules",
    ),
    ("scripts/exit/**", "gates: the exit tests"),
    ("scripts/check-*.sh", "gates: the repository checks"),
    ("scripts/run-exit-scripts.sh", "gates: the exit-test runner"),
    (".github/workflows/**", "gates: CI"),
    ("deny.toml", "gates: dependency policy"),
    ("clients/features.toml", "gates: client parity"),
    (
        "crates/daemon/tests/fixtures/**",
        "gates: golden event logs",
    ),
    // verdict logic: what counts as green
    (
        "crates/testing/**",
        "verdict logic: parsers, proof, classification, gates",
    ),
    (
        "crates/daemon/src/suites.rs",
        "verdict logic: running suites",
    ),
    (
        "crates/daemon/src/selftest.rs",
        "gates: what a new build proves before it takes over",
    ),
    // guards: what keeps work safe
    (
        "crates/adapter-api/src/netguard.rs",
        "guards: the test-mode network guard",
    ),
    (
        "crates/adapter-api/src/contract.rs",
        "guards: the step contract's validation",
    ),
    (
        "crates/adapter-api/src/webhook.rs",
        "guards: webhook verification",
    ),
    (
        "crates/landing/**",
        "guards: how work is merged and promoted",
    ),
    (
        "crates/daemon/src/landing.rs",
        "guards: batching, the red ladder, the freeze",
    ),
    (
        "crates/daemon/src/update.rs",
        "guards: updates, rollback, no downgrades",
    ),
    // budgets: what limits spending and concurrency
    ("crates/resources/**", "budgets: shared resources"),
    ("crates/scheduler/**", "budgets: the scheduler"),
    ("crates/daemon/src/sched.rs", "budgets: leases and limits"),
    // trust: whose words count
    (
        "crates/daemon/src/prompts.rs",
        "trust: fencing of outside text",
    ),
    ("crates/daemon/src/attention.rs", "trust: who may answer"),
    ("crates/core/src/task.rs", "trust: task trust sources"),
    (
        "crates/definition/src/layers.rs",
        "trust: configuration layers",
    ),
    (
        "crates/definition/src/validate.rs",
        "trust: definition validation",
    ),
    (
        "crates/definition/src/filing.rs",
        "trust: what a filing may claim",
    ),
    // self-change rules: this check, and the limits
    (
        "crates/daemon/src/protect.rs",
        "self-change rules: this check",
    ),
    (
        "crates/daemon/src/selfguard.rs",
        "self-change rules: holds and limits",
    ),
];

/// Does `path` match a glob? `**` is any number of path segments (even none),
/// `*` any run of characters inside a segment, `?` one character.
pub fn glob(pattern: &str, path: &str) -> bool {
    fn seg(p: &[u8], s: &[u8]) -> bool {
        match (p.first(), s.first()) {
            (None, None) => true,
            (Some(b'*'), _) => seg(&p[1..], s) || (!s.is_empty() && seg(p, &s[1..])),
            (Some(b'?'), Some(_)) => seg(&p[1..], &s[1..]),
            (Some(a), Some(b)) if a == b => seg(&p[1..], &s[1..]),
            _ => false,
        }
    }
    fn go(p: &[&str], s: &[&str]) -> bool {
        match p.first() {
            None => s.is_empty(),
            Some(&"**") => (0..=s.len()).any(|i| go(&p[1..], &s[i..])),
            Some(seg_p) => {
                !s.is_empty() && seg(seg_p.as_bytes(), s[0].as_bytes()) && go(&p[1..], &s[1..])
            }
        }
    }
    let p: Vec<&str> = pattern.split('/').collect();
    let s: Vec<&str> = path.split('/').collect();
    go(&p, &s)
}

/// Why `path` is protected, if it is (the compiled list, plus extra globs the
/// owner added in the project file; the owner can add, a change cannot remove).
pub fn protected_because(path: &str, extra: &[String]) -> Option<String> {
    let path = path.trim_start_matches("./");
    for (g, why) in PROTECTED {
        if glob(g, path) {
            return Some((*why).to_string());
        }
    }
    extra
        .iter()
        .find(|g| glob(g, path))
        .map(|g| format!("protected by the owner: {g}"))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub file: String,
    pub kind: &'static str,
    pub detail: String,
}

impl Finding {
    fn new(file: &str, kind: &'static str, detail: impl Into<String>) -> Finding {
        Finding {
            file: file.into(),
            kind,
            detail: detail.into(),
        }
    }
}

const ASSERTS: &[&str] = &[
    "assert!(",
    "assert_eq!(",
    "assert_ne!(",
    "debug_assert",
    "XCTAssert",
    "assertEquals(",
    "assertTrue(",
    "assertFalse(",
    "assertNull(",
    "assertNotNull(",
    "assertThat(",
];
const TESTS: &[&str] = &["#[test]", "@Test", "func test"];
const LIMIT_WORDS: &[&str] = &[
    "limit",
    "max",
    "timeout",
    "budget",
    "cap",
    "threshold",
    "quota",
    "retries",
    "attempts",
    "rounds",
    "ceiling",
];
const FLOOR_WORDS: &[&str] = &["min", "floor", "required", "coverage", "at_least"];

fn count_in(lines: &[String], pats: &[&str]) -> usize {
    lines
        .iter()
        .filter(|l| pats.iter().any(|p| l.contains(p)))
        .count()
}

/// `name = 30`, `const NAME: u32 = 30;`, `name: 30,`, `--name 30`: the name and the number.
fn name_and_number(line: &str) -> Option<(String, f64)> {
    let t = line.trim();
    // `name = 30` and `const NAME: u32 = 30;` have an `=`; `name: 30,` has only a colon.
    let (left, right) = match t.find('=') {
        Some(i) if i > 0 => (&t[..i], &t[i + 1..]),
        _ => {
            let i = t.find(':').filter(|i| *i > 0)?;
            (&t[..i], &t[i + 1..])
        }
    };
    let left = left.split(':').next().unwrap_or(left);
    let name = left
        .rsplit(|c: char| !(c.is_alphanumeric() || c == '_'))
        .find(|w| !w.is_empty())?
        .to_lowercase();
    let num: String = right
        .trim_start_matches(['"', '\'', ' ', '='])
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '_' || *c == '.')
        .filter(|c| *c != '_')
        .collect();
    if num.is_empty() {
        return None;
    }
    Some((name, num.parse().ok()?))
}

/// Unified diff in, findings out. Per file: net removed assertions and tests; every
/// added ignore or skip; limits whose number went up and floors whose went down
/// (matched by name between a removed and an added line).
pub fn scan_diff(diff: &str) -> Vec<Finding> {
    #[derive(Default)]
    struct F {
        removed: Vec<String>,
        added: Vec<String>,
        deleted: bool,
    }
    let mut files: BTreeMap<String, F> = BTreeMap::new();
    let mut cur: Option<String> = None;
    let mut old_name: Option<String> = None;
    for l in diff.lines() {
        if let Some(p) = l.strip_prefix("--- ") {
            old_name = Some(p.trim_start_matches("a/").to_string());
        } else if let Some(p) = l.strip_prefix("+++ ") {
            if p == "/dev/null" {
                let n = old_name.clone().unwrap_or_default();
                files.entry(n.clone()).or_default().deleted = true;
                cur = Some(n);
            } else {
                let n = p.trim_start_matches("b/").to_string();
                files.entry(n.clone()).or_default();
                cur = Some(n);
            }
        } else if let Some(f) = cur.as_ref().and_then(|c| files.get_mut(c)) {
            if let Some(a) = l.strip_prefix('+') {
                f.added.push(a.to_string());
            } else if let Some(r) = l.strip_prefix('-') {
                f.removed.push(r.to_string());
            }
        }
    }
    let mut out = vec![];
    for (file, f) in &files {
        let is_test_path = file.contains("/tests/")
            || file.contains("/test/")
            || file.contains("Test")
            || file.ends_with("_test.rs")
            || file.starts_with("tests/");
        if f.deleted && is_test_path {
            out.push(Finding::new(
                file,
                "removed-test-file",
                "a test file was deleted",
            ));
            continue;
        }
        let (ra, aa) = (count_in(&f.removed, ASSERTS), count_in(&f.added, ASSERTS));
        if ra > aa {
            out.push(Finding::new(
                file,
                "removed-assertion",
                format!("{} assertion(s) removed, {} added", ra, aa),
            ));
        }
        let (rt, at) = (count_in(&f.removed, TESTS), count_in(&f.added, TESTS));
        if rt > at {
            out.push(Finding::new(
                file,
                "removed-test",
                format!("{} test(s) removed, {} added", rt, at),
            ));
        }
        for l in &f.added {
            let t = l.trim();
            let ignored = t.contains("#[ignore")
                || t.contains("@Ignore")
                || t.contains("@Disabled")
                || t.contains(".skip(")
                || t.starts_with("xit(")
                || t.starts_with("xdescribe(");
            if ignored {
                out.push(Finding::new(
                    file,
                    "ignored-test",
                    t.chars().take(100).collect::<String>(),
                ));
            }
            let lower = t.to_lowercase();
            let broadened = lower.contains("|| true")
                || lower.contains("continue-on-error: true")
                || lower.replace(' ', "").contains("skip=true")
                || lower.replace(' ', "").contains("skip:true")
                || (file.ends_with(".sh") && (lower == "exit 0" || lower.starts_with("exit 0 ")));
            if broadened {
                out.push(Finding::new(
                    file,
                    "broadened-skip",
                    t.chars().take(100).collect::<String>(),
                ));
            }
        }
        let removed: Vec<(String, f64)> = f
            .removed
            .iter()
            .filter_map(|l| name_and_number(l))
            .collect();
        for l in &f.added {
            let Some((name, new)) = name_and_number(l) else {
                continue;
            };
            let Some((_, old)) = removed.iter().find(|(n, _)| *n == name) else {
                continue;
            };
            if LIMIT_WORDS.iter().any(|w| name.contains(w)) && new > *old {
                out.push(Finding::new(
                    file,
                    "raised-limit",
                    format!("{name}: {old} -> {new}"),
                ));
            } else if FLOOR_WORDS.iter().any(|w| name.contains(w)) && new < *old {
                out.push(Finding::new(
                    file,
                    "lowered-floor",
                    format!("{name}: {old} -> {new}"),
                ));
            }
        }
    }
    out
}

/// What the check found in a change.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Inspection {
    /// `(path, why)` of each protected file the change touches.
    pub protected: Vec<(String, String)>,
    pub weakening: Vec<Finding>,
}

impl Inspection {
    pub fn clean(&self) -> bool {
        self.protected.is_empty() && self.weakening.is_empty()
    }

    /// One paragraph for the owner: what the change touches and what it weakens.
    pub fn explain(&self) -> String {
        let mut parts = vec![];
        if !self.protected.is_empty() {
            let mut by_why: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
            for (p, w) in &self.protected {
                by_why.entry(w).or_default().push(p);
            }
            for (w, ps) in by_why {
                parts.push(format!("touches {} ({})", ps.join(", "), w));
            }
        }
        for f in &self.weakening {
            parts.push(format!(
                "{} in {}: {}",
                f.kind.replace('-', " "),
                f.file,
                f.detail
            ));
        }
        parts.join("; ")
    }
}

pub fn inspect(paths: &[String], diff: &str, extra: &[String]) -> Inspection {
    Inspection {
        protected: paths
            .iter()
            .filter_map(|p| protected_because(p, extra).map(|w| (p.clone(), w)))
            .collect(),
        weakening: scan_diff(diff),
    }
}

/// The identity of an exact change: an approval is for this diff and no other.
pub fn diff_hash(diff: &str) -> String {
    hex(&sha256(diff.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs() {
        assert!(glob(".mason/**", ".mason/mason.toml"));
        assert!(glob(".mason/**", ".mason/a/b/c"));
        assert!(!glob(".mason/**", "src/.mason/x"));
        assert!(glob("crates/testing/**", "crates/testing/src/verdict.rs"));
        assert!(glob("scripts/check-*.sh", "scripts/check-spdx.sh"));
        assert!(!glob("scripts/check-*.sh", "scripts/check/x.sh"));
        assert!(glob("deny.toml", "deny.toml") && !glob("deny.toml", "x/deny.toml"));
        assert!(
            glob("**/x.rs", "x.rs") && glob("**/x.rs", "a/b/x.rs") && !glob("**/x.rs", "a/b/y.rs")
        );
        assert!(glob("a/?.rs", "a/b.rs") && !glob("a/?.rs", "a/bc.rs"));
    }

    #[test]
    fn the_listed_files_are_protected_and_the_rest_are_not() {
        for p in [
            ".mason/landing.toml",
            "scripts/exit/s05.sh",
            "crates/testing/src/verdict.rs",
            "crates/landing/src/plan.rs",
            "crates/daemon/src/protect.rs",
            "crates/daemon/src/selfguard.rs",
            ".github/workflows/ci.yml",
            "crates/adapter-api/src/netguard.rs",
        ] {
            assert!(protected_because(p, &[]).is_some(), "{p}");
        }
        for p in [
            "docs/stages/s17.md",
            "crates/engine/src/reducer.rs",
            "clients/swift/Sources/MonitorUI/Pages.swift",
            "README.md",
        ] {
            assert!(protected_because(p, &[]).is_none(), "{p}");
        }
        // The owner can add, with a reason that names the glob.
        assert_eq!(
            protected_because("docs/secret.md", &["docs/secret*".into()]).unwrap(),
            "protected by the owner: docs/secret*"
        );
    }

    #[test]
    fn this_check_protects_itself() {
        // The two files that decide are on the list, so a change to them cannot go unseen.
        assert!(protected_because("crates/daemon/src/protect.rs", &[]).is_some());
        assert!(protected_because("crates/daemon/src/selfguard.rs", &[]).is_some());
    }

    fn diff(file: &str, removed: &[&str], added: &[&str]) -> String {
        let mut d = format!("--- a/{file}\n+++ b/{file}\n@@ -1 +1 @@\n");
        for r in removed {
            d.push_str(&format!("-{r}\n"));
        }
        for a in added {
            d.push_str(&format!("+{a}\n"));
        }
        d
    }

    fn kinds(d: &str) -> Vec<&'static str> {
        scan_diff(d).into_iter().map(|f| f.kind).collect()
    }

    #[test]
    fn a_removed_assertion_is_flagged_but_a_changed_one_is_not() {
        assert_eq!(
            kinds(&diff("src/a.rs", &["    assert_eq!(x, 1);"], &[])),
            ["removed-assertion"]
        );
        assert_eq!(
            kinds(&diff(
                "src/a.rs",
                &["    assert_eq!(x, 1);"],
                &["    assert_eq!(x, 2);"]
            )),
            Vec::<&str>::new()
        );
        assert_eq!(
            kinds(&diff("T.kt", &["assertEquals(1, x)"], &[])),
            ["removed-assertion"]
        );
        // Adding assertions is never a finding.
        assert!(kinds(&diff("src/a.rs", &[], &["assert!(ok);"])).is_empty());
    }

    #[test]
    fn removed_and_ignored_tests_are_flagged() {
        assert_eq!(
            kinds(&diff("src/a.rs", &["#[test]", "fn it_works() {}"], &[])),
            ["removed-test"]
        );
        assert_eq!(
            kinds(&diff(
                "src/a.rs",
                &[],
                &["#[test]", "#[ignore]", "fn flaky() {}"]
            )),
            ["ignored-test"]
        );
        assert!(
            kinds(&diff("T.kt", &[], &["@Ignore", "@Test fun x() {}"])).contains(&"ignored-test")
        );
        let deleted = "--- a/crates/x/tests/it.rs\n+++ /dev/null\n@@ -1 +0,0 @@\n-fn a() {}\n";
        assert_eq!(kinds(deleted), ["removed-test-file"]);
    }

    #[test]
    fn a_raised_limit_or_lowered_floor_is_flagged_by_name() {
        let f = scan_diff(&diff(
            "src/a.rs",
            &["pub const MAX_ATTEMPTS: u32 = 3;"],
            &["pub const MAX_ATTEMPTS: u32 = 30;"],
        ));
        assert_eq!(f.len(), 1);
        assert_eq!(
            (f[0].kind, f[0].detail.as_str()),
            ("raised-limit", "max_attempts: 3 -> 30")
        );
        assert_eq!(
            kinds(&diff(
                ".x.toml",
                &["test_timeout_s = 60"],
                &["test_timeout_s = 6000"]
            )),
            ["raised-limit"]
        );
        assert_eq!(
            kinds(&diff("a.toml", &["min_tests = 5"], &["min_tests = 1"])),
            ["lowered-floor"]
        );
        // Tightened, or unrelated names, are fine.
        assert!(kinds(&diff(
            "src/a.rs",
            &["const MAX_ATTEMPTS: u32 = 3;"],
            &["const MAX_ATTEMPTS: u32 = 2;"]
        ))
        .is_empty());
        assert!(kinds(&diff("src/a.rs", &["let width = 3;"], &["let width = 30;"])).is_empty());
        assert!(kinds(&diff(
            "src/a.rs",
            &["const MAX_X: u32 = 3;"],
            &["const OTHER_MAX: u32 = 30;"]
        ))
        .is_empty());
    }

    #[test]
    fn a_broadened_skip_is_flagged() {
        assert_eq!(
            kinds(&diff("run.sh", &[], &["cargo test || true"])),
            ["broadened-skip"]
        );
        assert_eq!(kinds(&diff("run.sh", &[], &["exit 0"])), ["broadened-skip"]);
        assert_eq!(
            kinds(&diff("ci.yml", &[], &["    continue-on-error: true"])),
            ["broadened-skip"]
        );
        assert_eq!(
            kinds(&diff("d.toml", &[], &["skip = true"])),
            ["broadened-skip"]
        );
        assert!(kinds(&diff("src/a.rs", &[], &["// we must not skip anything"])).is_empty());
    }

    #[test]
    fn the_explanation_names_files_and_reasons() {
        let i = inspect(
            &["crates/testing/src/verdict.rs".into(), "docs/x.md".into()],
            &diff("crates/engine/tests/m.rs", &["assert!(a);"], &[]),
            &[],
        );
        assert!(!i.clean());
        let e = i.explain();
        assert!(
            e.contains("crates/testing/src/verdict.rs")
                && e.contains("verdict logic")
                && e.contains("removed assertion in crates/engine/tests/m.rs"),
            "{e}"
        );
        assert!(!e.contains("docs/x.md"));
        assert!(inspect(
            &["docs/x.md".into()],
            &diff("docs/x.md", &["a"], &["b"]),
            &[]
        )
        .clean());
    }

    #[test]
    fn an_approval_is_for_one_exact_diff() {
        let a = diff("src/a.rs", &["x"], &["y"]);
        let b = diff("src/a.rs", &["x"], &["z"]);
        assert_eq!(diff_hash(&a), diff_hash(&a));
        assert_ne!(diff_hash(&a), diff_hash(&b));
        assert_eq!(diff_hash(&a).len(), 64);
    }
}
