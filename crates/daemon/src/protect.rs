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
    (
        "scripts/gen-*.sh",
        "gates: scripts the daemon runs when changes land",
    ),
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
    /// The line in the new version of the file (the added line, or the first
    /// changed line for findings about removals).
    pub line: Option<usize>,
}

impl Finding {
    fn new(file: &str, kind: &'static str, detail: impl Into<String>, line: usize) -> Finding {
        Finding {
            file: file.into(),
            kind,
            detail: detail.into(),
            line: Some(line).filter(|l| *l > 0),
        }
    }
}

/// Commands whose failure is ordinary at cleanup or in a probe: `kill $pid || true`
/// is not a test that stopped mattering. A `|| true` after one of these hides no check.
const CLEANUP_WORDS: &[&str] = &[
    "kill", "pkill", "killall", "rm", "rmdir", "unlink", "wait", "trap", "command", "which",
    "type", "pgrep",
];
// `test`, `[` and `[[` are deliberately not here: `[ "$n" -gt 0 ] || true` turns an assertion
// into a no-op. A `[ ... ] && kill ... || true` chain is still fine, its last command is `kill`.

/// A Markdown file: its lines are read, never run, so they cannot broaden a skip.
fn is_prose(file: &str) -> bool {
    let f = file.to_ascii_lowercase();
    f.ends_with(".md") || f.ends_with(".markdown")
}

/// The last simple command of a shell fragment, its first word.
fn last_command_word(fragment: &str) -> String {
    let mut seg = fragment;
    for sep in ["&&", ";", "|"] {
        if let Some(i) = seg.rfind(sep) {
            seg = &seg[i + sep.len()..];
        }
    }
    let mut words = seg.split_whitespace().peekable();
    while let Some(w) = words.peek() {
        if matches!(*w, "do" | "then" | "else" | "{" | "(" | "!") {
            words.next();
        } else {
            break;
        }
    }
    words.next().unwrap_or("").to_string()
}

/// Is every `|| true` on this (lowercased) line one that hides nothing: it follows a
/// cleanup or probe command, or the same line then exits non-zero?
fn or_true_is_harmless(lower: &str) -> bool {
    let ops: Vec<&str> = lower.split("||").collect();
    let mut seen = false;
    for k in 1..ops.len() {
        let rest = ops[k].trim_start();
        let Some(after) = rest.strip_prefix("true") else {
            continue;
        };
        if after
            .chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric() || c == '_')
        {
            continue;
        }
        seen = true;
        // `... || true; exit 1`: the script still fails.
        let tail = after.split("exit").nth(1).map(str::trim_start);
        if let Some(n) = tail {
            let n: String = n.chars().take_while(|c| c.is_ascii_digit()).collect();
            if !n.is_empty() && n.chars().any(|c| c != '0') {
                continue;
            }
        }
        if !CLEANUP_WORDS.contains(&last_command_word(ops[k - 1]).as_str()) {
            return false;
        }
    }
    seen
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
    scan_full(diff).0
}

/// The findings, and for each file the first changed line (new side).
fn scan_full(diff: &str) -> (Vec<Finding>, BTreeMap<String, usize>) {
    #[derive(Default)]
    struct F {
        removed: Vec<String>,
        added: Vec<String>,
        /// The new-side line of each added line.
        at: Vec<usize>,
        deleted: bool,
        /// Added whole: its old side is /dev/null, so nothing was weakened by exiting early.
        new: bool,
        first: usize,
    }
    let mut files: BTreeMap<String, F> = BTreeMap::new();
    let mut cur: Option<String> = None;
    let mut old_name: Option<String> = None;
    let mut next = 0usize;
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
                let f = files.entry(n.clone()).or_default();
                f.new = old_name.as_deref() == Some("/dev/null");
                cur = Some(n);
            }
        } else if let Some(h) = l.strip_prefix("@@ ") {
            // `@@ -a,b +c,d @@`: c is where the new side starts.
            let c = h
                .split_whitespace()
                .find_map(|w| w.strip_prefix('+'))
                .and_then(|w| w.split(',').next())
                .and_then(|w| w.parse::<usize>().ok())
                .unwrap_or(0);
            next = c;
            if let Some(f) = cur.as_ref().and_then(|c| files.get_mut(c)) {
                if f.first == 0 {
                    f.first = c.max(1);
                }
            }
        } else if let Some(f) = cur.as_ref().and_then(|c| files.get_mut(c)) {
            if let Some(a) = l.strip_prefix('+') {
                f.added.push(a.to_string());
                f.at.push(next);
                next += 1;
            } else if l.starts_with(' ') {
                next += 1;
            } else if let Some(r) = l.strip_prefix('-') {
                f.removed.push(r.to_string());
            }
        }
    }
    let mut out = vec![];
    let firsts: BTreeMap<String, usize> = files
        .iter()
        .map(|(k, f)| (k.clone(), f.first))
        .filter(|(_, l)| *l > 0)
        .collect();
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
                f.first,
            ));
            continue;
        }
        let (ra, aa) = (count_in(&f.removed, ASSERTS), count_in(&f.added, ASSERTS));
        if ra > aa {
            out.push(Finding::new(
                file,
                "removed-assertion",
                format!("{} assertion(s) removed, {} added", ra, aa),
                f.first,
            ));
        }
        let (rt, at) = (count_in(&f.removed, TESTS), count_in(&f.added, TESTS));
        if rt > at {
            out.push(Finding::new(
                file,
                "removed-test",
                format!("{} test(s) removed, {} added", rt, at),
                f.first,
            ));
        }
        for (i, l) in f.added.iter().enumerate() {
            let line = f.at[i];
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
                    line,
                ));
            }
            let lower = t.to_lowercase();
            // Markdown is prose: a note that quotes `|| true` runs nothing (decision 0122).
            let broadened = !is_prose(file)
                && ((lower.contains("|| true") && !or_true_is_harmless(&lower))
                    || lower.contains("continue-on-error: true")
                    || lower.replace(' ', "").contains("skip=true")
                    || lower.replace(' ', "").contains("skip:true")
                    || (file.ends_with(".sh")
                        && !f.new
                        && (lower == "exit 0" || lower.starts_with("exit 0 "))));
            if broadened {
                out.push(Finding::new(
                    file,
                    "broadened-skip",
                    t.chars().take(100).collect::<String>(),
                    line,
                ));
            }
        }
        let removed: Vec<(String, f64)> = f
            .removed
            .iter()
            .filter_map(|l| name_and_number(l))
            .collect();
        for (i, l) in f.added.iter().enumerate() {
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
                    f.at[i],
                ));
            } else if FLOOR_WORDS.iter().any(|w| name.contains(w)) && new < *old {
                out.push(Finding::new(
                    file,
                    "lowered-floor",
                    format!("{name}: {old} -> {new}"),
                    f.at[i],
                ));
            }
        }
    }
    (out, firsts)
}

/// A path a change touches, and whether the change added it (it did not exist before).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Changed {
    pub path: String,
    pub added: bool,
}

/// Protected paths where *adding* a file is not held by its path. A new exit script
/// has no earlier version to weaken, and it only runs once the runner lists it, which
/// is a change to `scripts/run-exit-scripts.sh` and is held. Editing an existing
/// gate, and every other protected path, stays held (decision 0019). The content of
/// a new file is still scanned for weakening.
const ADDED_OK: &[&str] = &["scripts/exit/**"];

/// What the check found in a change.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Inspection {
    /// `(path, why)` of each protected file the change touches.
    pub protected: Vec<(String, String)>,
    pub weakening: Vec<Finding>,
    /// The first changed line of each file in the diff.
    pub first_lines: BTreeMap<String, usize>,
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

    /// The short form the owner reads: each finding with file and line, what is asked,
    /// and the diff to look at. `mason task show` and `mason self` print this text.
    pub fn short(&self, diff_cmd: &str) -> String {
        let at = |file: &str, line: Option<usize>| match line {
            Some(l) => format!("{file}:{l}"),
            None => file.to_string(),
        };
        let clip = |s: &str| -> String { s.replace(';', ",").chars().take(60).collect() };
        let mut parts = vec![];
        for (p, why) in &self.protected {
            parts.push(format!(
                "touches {} ({})",
                at(p, self.first_lines.get(p).copied()),
                why
            ));
        }
        for f in &self.weakening {
            parts.push(format!(
                "{} at {} ({})",
                f.kind.replace('-', " "),
                at(&f.file, f.line),
                clip(&f.detail)
            ));
        }
        format!(
            "{}. Decide: approve only if this change is intended. Diff: {diff_cmd}",
            parts.join("; ")
        )
    }
}

/// Paths that exist already (all treated as edits).
pub fn inspect(paths: &[String], diff: &str, extra: &[String]) -> Inspection {
    let changed: Vec<Changed> = paths
        .iter()
        .map(|p| Changed {
            path: p.clone(),
            added: false,
        })
        .collect();
    inspect_changed(&changed, diff, extra)
}

pub fn inspect_changed(files: &[Changed], diff: &str, extra: &[String]) -> Inspection {
    let (weakening, first_lines) = scan_full(diff);
    Inspection {
        protected: files
            .iter()
            .filter(|c| {
                !(c.added
                    && ADDED_OK
                        .iter()
                        .any(|g| glob(g, c.path.trim_start_matches("./"))))
            })
            .filter_map(|c| protected_because(&c.path, extra).map(|w| (c.path.clone(), w)))
            .collect(),
        weakening,
        first_lines,
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
            "scripts/gen-status.sh",
            "scripts/gen-other.sh",
        ] {
            assert!(protected_because(p, &[]).is_some(), "{p}");
        }
        for p in [
            "docs/stages/s17.md",
            "crates/engine/src/reducer.rs",
            "clients/swift/Sources/MonitorUI/Pages.swift",
            "README.md",
            "scripts/genesis.sh",
            "scripts/gen.sh",
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
    fn a_quoted_or_true_in_markdown_is_not_a_broadened_skip() {
        let note = "Counters use `find` instead of `ls | grep || true`.";
        assert!(kinds(&diff("docs/stages/s98.md", &[], &[note])).is_empty());
        assert!(kinds(&diff("README.MD", &[], &["cargo test || true"])).is_empty());
        // The same line in a script, or a workflow, is still flagged.
        assert_eq!(kinds(&diff("run.sh", &[], &[note])), ["broadened-skip"]);
        assert_eq!(
            kinds(&diff("ci.yml", &[], &["  run: cargo test || true"])),
            ["broadened-skip"]
        );
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

    // ---- Stage 41: what a task really weakens ----------------------------------------------

    /// A cleanup line, plus one real weakening in the same file: exactly the weakening is found.
    fn only_the_real_one(file: &str, cleanup: &str, real: &str) {
        let d = diff(file, &[], &[cleanup, real]);
        let f = scan_diff(&d);
        assert_eq!(f.len(), 1, "{cleanup:?} must not be flagged: {f:?}");
        assert_eq!(f[0].kind, "broadened-skip");
        assert_eq!(f[0].detail, real);
        assert!(
            scan_diff(&diff(file, &[], &[cleanup])).is_empty(),
            "{cleanup:?} alone"
        );
    }

    #[test]
    fn a_kill_cleanup_is_not_a_broadened_skip() {
        only_the_real_one(
            "scripts/exit/s9.sh",
            "kill \"$PID\" 2>/dev/null || true",
            "cargo test || true",
        );
    }

    #[test]
    fn an_rm_cleanup_is_not_a_broadened_skip() {
        only_the_real_one(
            "scripts/exit/s9.sh",
            "rm -rf \"$TMP\" || true",
            "grep -c x out || true",
        );
    }

    #[test]
    fn a_wait_cleanup_is_not_a_broadened_skip() {
        only_the_real_one(
            "scripts/exit/s9.sh",
            "wait \"$PID\" 2>/dev/null || true",
            "cargo test || true",
        );
    }

    #[test]
    fn a_trap_is_not_a_broadened_skip() {
        only_the_real_one(
            "scripts/exit/s9.sh",
            "trap 'kill $PID 2>/dev/null || true; rm -rf \"$T\"' EXIT",
            "cargo test || true",
        );
        // `trap` as the command itself.
        only_the_real_one(
            "scripts/exit/s9.sh",
            "trap - EXIT || true",
            "cargo test || true",
        );
    }

    #[test]
    fn a_command_probe_is_not_a_broadened_skip() {
        only_the_real_one(
            "scripts/exit/s9.sh",
            "command -v jq >/dev/null 2>&1 || true",
            "grep -c x out || true",
        );
    }

    #[test]
    fn a_kill_chain_is_not_a_broadened_skip() {
        only_the_real_one(
            "scripts/exit/s9.sh",
            "[ -n \"$PID\" ] && kill \"$PID\" || true",
            "cargo test || true",
        );
        only_the_real_one(
            "scripts/exit/s9.sh",
            "for p in $PIDS; do kill $p || true; done",
            "cargo test || true",
        );
    }

    #[test]
    fn an_assertion_followed_by_or_true_is_still_a_broadened_skip() {
        for line in [
            "[ \"$n\" -gt 0 ] || true",
            "[[ -s out ]] || true",
            "test -s out || true",
            "test \"$a\" = \"$b\" || true",
        ] {
            let f = scan_diff(&diff("scripts/exit/s9.sh", &[], &[line]));
            assert_eq!(f.len(), 1, "{line:?} turns a check into a no-op: {f:?}");
            assert_eq!(f[0].kind, "broadened-skip");
        }
    }

    #[test]
    fn a_diagnostic_then_a_failing_exit_is_not_a_broadened_skip() {
        only_the_real_one(
            "scripts/exit/s9.sh",
            "tail -5 log || true; exit 1",
            "cargo test || true",
        );
        // `exit 0` after it does not excuse it.
        let f = scan_diff(&diff(
            "scripts/exit/s9.sh",
            &[],
            &["tail -5 log || true; exit 0"],
        ));
        assert_eq!(f.len(), 1, "{f:?}");
    }

    #[test]
    fn a_bare_exit_zero_is_flagged_in_an_old_file_and_not_in_a_new_one() {
        let new = "--- /dev/null\n+++ b/scripts/exit/s9.sh\n@@ -0,0 +1,2 @@\n+echo hi\n+exit 0\n";
        assert!(scan_diff(new).is_empty());
        // The same line in an existing file is the case the rule is for.
        let f = scan_diff(&diff("scripts/exit/s9.sh", &[], &["exit 0"]));
        assert_eq!(f.len(), 1);
        // A new file with a real `|| true` is still flagged.
        let new = "--- /dev/null\n+++ b/scripts/exit/s9.sh\n@@ -0,0 +1,2 @@\n+cargo test || true\n+exit 0\n";
        let f = scan_diff(new);
        assert_eq!(f.len(), 1, "{f:?}");
        assert_eq!(
            (f[0].detail.as_str(), f[0].line),
            ("cargo test || true", Some(1))
        );
    }

    #[test]
    fn findings_carry_the_line_of_the_added_line() {
        let d = "--- a/scripts/exit/s9.sh\n+++ b/scripts/exit/s9.sh\n@@ -3,0 +4,2 @@\n+echo a\n+cargo test || true\n@@ -20 +22 @@\n-x\n+exit 0\n";
        let f = scan_diff(d);
        let got: Vec<_> = f.iter().map(|f| (f.detail.as_str(), f.line)).collect();
        assert_eq!(got, [("cargo test || true", Some(5)), ("exit 0", Some(22))]);
        // Removals carry the first hunk.
        let f = scan_diff("--- a/src/a.rs\n+++ b/src/a.rs\n@@ -7 +6,0 @@\n-    assert!(x);\n");
        assert_eq!((f[0].kind, f[0].line), ("removed-assertion", Some(6)));
    }

    #[test]
    fn a_generator_the_daemon_runs_is_held_whether_added_or_edited() {
        let ch = |p: &str, added: bool| Changed {
            path: p.into(),
            added,
        };
        let d = "--- /dev/null\n+++ b/scripts/gen-x.sh\n@@ -0,0 +1 @@\n+echo ok\n";
        let added = inspect_changed(&[ch("scripts/gen-x.sh", true)], d, &[]);
        assert!(!added.clean());
        assert!(added.protected[0].1.contains("runs when changes land"));
        assert!(!inspect_changed(&[ch("scripts/gen-status.sh", false)], d, &[]).clean());
        // A new exit script is not held by its path; the generator is not exempt.
        assert!(inspect_changed(&[ch("scripts/exit/s99.sh", true)], d, &[]).clean());
    }

    #[test]
    fn a_new_exit_script_is_not_held_by_its_path_but_edits_and_the_runner_are() {
        let add = |p: &str| Changed {
            path: p.into(),
            added: true,
        };
        let edit = |p: &str| Changed {
            path: p.into(),
            added: false,
        };
        let d = "--- /dev/null\n+++ b/scripts/exit/s99.sh\n@@ -0,0 +1 @@\n+echo ok\n";
        assert!(inspect_changed(&[add("scripts/exit/s99.sh")], d, &[]).clean());
        // An existing gate edited, the runner (even if "added"), and other gates stay held.
        assert!(!inspect_changed(&[edit("scripts/exit/s17.sh")], d, &[]).clean());
        assert!(!inspect_changed(&[add("scripts/run-exit-scripts.sh")], d, &[]).clean());
        assert!(!inspect_changed(&[add("scripts/check-new.sh")], d, &[]).clean());
        assert!(!inspect_changed(&[add(".github/workflows/x.yml")], d, &[]).clean());
        // `inspect` treats every path as an existing one.
        assert!(!inspect(&["scripts/exit/s99.sh".into()], d, &[]).clean());
    }

    #[test]
    fn the_hold_text_is_short_with_file_line_and_a_decision() {
        let d = "--- a/scripts/exit/s17.sh\n+++ b/scripts/exit/s17.sh\n@@ -3,0 +4 @@\n+cargo test || true\n";
        let i = inspect(&["scripts/exit/s17.sh".into()], d, &[]);
        let t = i.short("git -C /r diff origin/develop...b");
        assert!(
            t.contains("touches scripts/exit/s17.sh:4 (gates: the exit tests)")
                && t.contains("broadened skip at scripts/exit/s17.sh:4 (cargo test || true)")
                && t.contains("Decide: approve only if this change is intended")
                && t.ends_with("Diff: git -C /r diff origin/develop...b"),
            "{t}"
        );
        assert!(!t.contains('\n'));
    }
}
