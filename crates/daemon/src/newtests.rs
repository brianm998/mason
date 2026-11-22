// SPDX-License-Identifier: GPL-3.0-or-later
//! The tests a change adds, read from its diff (#812).
//!
//! A fast-tracked task (`mason task skip N --all`) has its testing skipped, but a change
//! that adds a failing test, or code that does not build, must not pass for that. What the
//! fast track still runs is the build and these tests, by name: each new `#[test]` fn and
//! each new or edited exit script. The same reading serves `run_check` (the worktree) and
//! the landing batch (the merged range).

use std::path::Path;

use mason_landing::git;

/// The tests a diff adds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Added {
    /// `(file, fn name)` of each new `#[test]` fn.
    pub tests: Vec<(String, String)>,
    /// Exit scripts the change adds or edits (`scripts/exit/*.sh`).
    pub exits: Vec<String>,
}

impl Added {
    pub fn is_empty(&self) -> bool {
        self.tests.is_empty() && self.exits.is_empty()
    }

    /// The names of the new test fns, once each.
    pub fn names(&self) -> Vec<String> {
        let mut v: Vec<String> = vec![];
        for (_, n) in &self.tests {
            if !v.contains(n) {
                v.push(n.clone());
            }
        }
        v
    }

    /// Every file that adds a test or script.
    pub fn files(&self) -> Vec<String> {
        let mut v: Vec<String> = vec![];
        for f in self.tests.iter().map(|(f, _)| f).chain(self.exits.iter()) {
            if !v.contains(f) {
                v.push(f.clone());
            }
        }
        v
    }

    /// Only what files in `only` add.
    pub fn restricted(&self, only: &[String]) -> Added {
        let ok = |f: &str| only.iter().any(|o| o == f);
        Added {
            tests: self.tests.iter().filter(|(f, _)| ok(f)).cloned().collect(),
            exits: self.exits.iter().filter(|f| ok(f)).cloned().collect(),
        }
    }

    /// In words, for a "Not run" text: `test a, test b, exit script scripts/exit/s1.sh`.
    pub fn describe(&self) -> String {
        let mut v: Vec<String> = self.names().iter().map(|n| format!("test {n}")).collect();
        v.extend(self.exits.iter().map(|e| format!("exit script {e}")));
        v.join(", ")
    }
}

/// An exit script path.
pub fn is_exit_script(path: &str) -> bool {
    path.starts_with("scripts/exit/") && path.ends_with(".sh") && path.matches('/').count() == 2
}

/// The new `#[test]` fns of a unified diff (`-U0` or any context): `(file, name)`.
pub fn parse_diff(diff: &str) -> Vec<(String, String)> {
    let mut out = vec![];
    let mut file = String::new();
    let mut pending = false;
    for line in diff.lines() {
        if let Some(f) = line.strip_prefix("+++ ") {
            file = f.strip_prefix("b/").unwrap_or(f).to_string();
            pending = false;
        } else if line.starts_with("@@") {
            pending = false;
        } else if let Some(added) = line.strip_prefix('+') {
            if file.ends_with(".rs") {
                scan_line(&file, added, &mut pending, &mut out);
            }
        }
    }
    out
}

/// One added line of Rust: an attribute makes the next `fn` a test.
fn scan_line(file: &str, line: &str, pending: &mut bool, out: &mut Vec<(String, String)>) {
    let t = line.trim();
    if t.starts_with("#[test]") || t.starts_with("#[tokio::test") || t.starts_with("#[test ") {
        *pending = true;
        if let Some(name) = fn_name(t) {
            out.push((file.to_string(), name));
            *pending = false;
        }
    } else if *pending {
        if let Some(name) = fn_name(t) {
            out.push((file.to_string(), name));
            *pending = false;
        } else if !(t.starts_with("#[") || t.starts_with("//") || t.is_empty()) {
            *pending = false;
        }
    }
}

fn fn_name(t: &str) -> Option<String> {
    let at = t.find("fn ")?;
    // `fn` must start a word (`pub fn`, `async fn`, or the line start).
    if at > 0 && !t[..at].ends_with(' ') && !t[..at].ends_with(']') {
        return None;
    }
    let name: String = t[at + 3..]
        .trim_start()
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    (!name.is_empty()).then_some(name)
}

fn from_parts(diff: &str, paths: &[String], untracked: &[(String, String)]) -> Added {
    let mut tests = parse_diff(diff);
    for (f, text) in untracked {
        if f.ends_with(".rs") {
            let mut pending = false;
            for l in text.lines() {
                scan_line(f, l, &mut pending, &mut tests);
            }
        }
    }
    let mut exits: Vec<String> = paths
        .iter()
        .filter(|p| is_exit_script(p))
        .cloned()
        .collect();
    exits.sort();
    exits.dedup();
    Added { tests, exits }
}

/// What the worktree adds to the base `base_ref`, committed or not (new files too): what
/// `run_check` reads while the agent is still working.
pub fn in_worktree(wt: &Path, base_ref: &str) -> Result<Added, git::GitError> {
    let mb = git::run(wt, &["merge-base", base_ref, "HEAD"])?;
    let mb = mb.trim();
    let diff = git::run(
        wt,
        &["diff", "-U0", "--no-color", "--no-ext-diff", mb, "--"],
    )?;
    let names = git::run(wt, &["diff", "--name-only", mb, "--"])?;
    let new = git::run(wt, &["ls-files", "--others", "--exclude-standard"]).unwrap_or_default();
    let mut paths: Vec<String> = names.lines().chain(new.lines()).map(String::from).collect();
    paths.retain(|p| !p.is_empty() && wt.join(p).is_file());
    let untracked: Vec<(String, String)> = new
        .lines()
        .filter(|f| f.ends_with(".rs"))
        .filter_map(|f| Some((f.to_string(), std::fs::read_to_string(wt.join(f)).ok()?)))
        .collect();
    Ok(from_parts(&diff, &paths, &untracked))
}

/// What the range `base..head` adds (committed work only): what a landing batch reads.
pub fn in_range(wt: &Path, base: &str, head: &str) -> Result<Added, git::GitError> {
    let range = format!("{base}..{head}");
    let diff = git::run(
        wt,
        &["diff", "-U0", "--no-color", "--no-ext-diff", &range, "--"],
    )?;
    let names = git::run(wt, &["diff", "--name-only", &range, "--"])?;
    let paths: Vec<String> = names.lines().map(String::from).collect();
    Ok(from_parts(&diff, &paths, &[]))
}

/// The test fn names a failing run's output names, among those `added` has (a line like
/// `test a::b::name ... FAILED`).
pub fn failing_among(added: &Added, output: &str) -> Vec<String> {
    let mut v: Vec<String> = vec![];
    for n in added.names() {
        let hit = output.lines().any(|l| {
            l.starts_with("test ")
                && l.contains("FAILED")
                && l.split(" ... ").next().is_some_and(|p| {
                    p.trim_start_matches("test ")
                        .rsplit("::")
                        .next()
                        .is_some_and(|last| last == n)
                })
        });
        if hit {
            v.push(n);
        }
    }
    v
}

const SOURCE_EXT: &[&str] = &[
    "rs", "swift", "kt", "py", "ts", "tsx", "js", "go", "java", "c", "cc", "cpp", "h",
];

/// A path of program source, not a test, a document or a script.
pub fn is_source_path(path: &str) -> bool {
    let ext = path.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    SOURCE_EXT.contains(&ext)
        && !path.starts_with("tests/")
        && !path.starts_with("docs/")
        && !path.starts_with("scripts/")
        && !path.contains("/tests/")
}

/// The source files a plan names under **Change** or **Cause** (a bold heading, or a `#`
/// heading, up to the next one): `crates/x/src/a.rs:12-30` gives `crates/x/src/a.rs`.
pub fn planned_sources(plan: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    let mut inside = false;
    for line in plan.lines() {
        let t = line.trim_start();
        let head = t
            .trim_start_matches('#')
            .trim_start()
            .trim_start_matches("**")
            .to_ascii_lowercase();
        if t.starts_with("**") || t.starts_with('#') {
            let word: String = head.chars().take_while(|c| c.is_alphabetic()).collect();
            if !word.is_empty() && (t.starts_with('#') || head.contains("**")) {
                inside = word == "change" || word == "cause";
            }
        }
        if !inside {
            continue;
        }
        for tok in line.split(|c: char| c.is_whitespace() || "`()[],;'\"*".contains(c)) {
            let path = tok.split(':').next().unwrap_or("").trim_end_matches('.');
            if path.contains('/')
                && !path.contains("//")
                && !path.starts_with('/')
                && path
                    .rsplit_once('.')
                    .is_some_and(|(_, e)| SOURCE_EXT.contains(&e))
                && is_source_path(path)
                && !out.contains(&path.to_string())
            {
                out.push(path.to_string());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIFF: &str = "diff --git a/crates/x/src/a.rs b/crates/x/src/a.rs\n\
--- a/crates/x/src/a.rs\n+++ b/crates/x/src/a.rs\n@@ -10,0 +11,6 @@\n\
+    #[test]\n+    fn adds_one() {\n+        assert!(true);\n+    }\n+    #[test]\n+    // why\n+    fn adds_two() {}\n\
@@ -30,0 +40,2 @@\n+fn helper() {}\n+let x = 1;\n\
diff --git a/README.md b/README.md\n--- a/README.md\n+++ b/README.md\n@@ -1 +1 @@\n+#[test]\n+fn nope() {}\n";

    #[test]
    fn new_test_fns_are_found_by_name_and_file() {
        let t = parse_diff(DIFF);
        assert_eq!(
            t,
            vec![
                ("crates/x/src/a.rs".to_string(), "adds_one".to_string()),
                ("crates/x/src/a.rs".to_string(), "adds_two".to_string()),
            ]
        );
    }

    #[test]
    fn exit_scripts_are_recognised_and_files_listed() {
        assert!(is_exit_script("scripts/exit/s9.sh"));
        assert!(!is_exit_script("scripts/exit/lib/x.sh"));
        assert!(!is_exit_script("scripts/check.sh"));
        let a = from_parts(DIFF, &["scripts/exit/s9.sh".to_string()], &[]);
        assert_eq!(a.names(), vec!["adds_one", "adds_two"]);
        assert_eq!(a.files(), vec!["crates/x/src/a.rs", "scripts/exit/s9.sh"]);
        assert!(a
            .restricted(&["scripts/exit/s9.sh".to_string()])
            .tests
            .is_empty());
    }

    #[test]
    fn a_failing_line_names_the_new_test() {
        let a = from_parts(DIFF, &[], &[]);
        let out = "test m::adds_one ... FAILED\ntest m::adds_two ... ok\n";
        assert_eq!(failing_among(&a, out), vec!["adds_one"]);
    }

    #[test]
    fn untracked_files_count_whole() {
        let a = from_parts(
            "",
            &[],
            &[("t.rs".into(), "#[test]\nfn fresh() {}\n".into())],
        );
        assert_eq!(a.names(), vec!["fresh"]);
    }

    #[test]
    fn a_plan_names_its_source_files_under_change_and_cause() {
        let plan = "Problem about docs/x.md.\n\n**Cause** (on `origin/develop`):\n- `crates/daemon/src/jobs.rs:2920-2932` `not_run_skipped`\n**Change**\n1. edit `crates/x/src/a.rs` and tests/it.rs\n**Coordinate**:\n- crates/other/src/b.rs\n";
        assert_eq!(
            planned_sources(plan),
            vec!["crates/daemon/src/jobs.rs", "crates/x/src/a.rs"]
        );
        assert!(is_source_path("crates/x/src/a.rs"));
        assert!(!is_source_path("crates/x/tests/it.rs"));
        assert!(!is_source_path("scripts/x.py"));
    }
}
