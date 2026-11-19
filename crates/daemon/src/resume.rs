// SPDX-License-Identifier: GPL-3.0-or-later

//! Going on after a lost step host without running again what already finished (#621).
//!
//! A cargo-workspace run prints one block per test executable: a `Running <src> (<exe>)` line
//! (`Doc-tests <crate>` for doc tests), the tests, and a `test result:` line when the executable
//! ends. When the host of such a run is lost, the lost run's log says which executables
//! finished. The fresh host runs only the others, rendered like a `{tests_for_change}`
//! narrowing, and the verdict reads the finished blocks of the lost log followed by the fresh
//! run's output, so every test of both counts and a red in the finished part stays red.
//!
//! An executable that was mid-run when the host died has no `test result:` line, so it is run
//! again whole. Anything this cannot place (a command that already selects targets, a log that
//! cannot be read, a target it cannot name) gives no plan: the whole command runs again.
//!
//! The parts here are pure given the log text and `cargo metadata --no-deps` output.

use crate::affected::{render, Group};
use crate::scope::{self, Narrow};
use mason_testing::Suite;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Kind {
    Lib,
    Bins,
    Test(String),
    Doc,
}

/// What cargo runs as one unit of `cargo test`: a package's lib unit tests, all its bins' unit
/// tests, one integration target, or its doc tests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unit {
    pkg: String,
    kind: Kind,
    /// The root files cargo prints for its executables (workspace-relative), or for doc tests
    /// the crate name.
    srcs: Vec<String>,
    /// The package directory (workspace-relative), for logs that print paths relative to it.
    dir: String,
}

/// The units of the workspace in `cargo metadata --no-deps` output (`root` is its canonical
/// directory). Targets `cargo test` skips (no `test`, or missing required features) are left out.
pub fn units(meta: &Value, root: &Path) -> Option<Vec<Unit>> {
    let members: BTreeSet<&str> = meta["workspace_members"]
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let rel = |p: &str| -> Option<String> {
        Some(
            Path::new(p)
                .strip_prefix(root)
                .ok()?
                .to_string_lossy()
                .to_string(),
        )
    };
    let mut out = vec![];
    for p in meta["packages"].as_array()? {
        if !members.is_empty() && !members.contains(p["id"].as_str().unwrap_or_default()) {
            continue;
        }
        let pkg = p["name"].as_str()?.to_string();
        let manifest = rel(p["manifest_path"].as_str()?)?;
        let dir = manifest
            .rsplit_once('/')
            .map_or("", |(d, _)| d)
            .trim_matches('/')
            .to_string();
        let mut bins = vec![];
        let mut mine = vec![];
        for t in p["targets"].as_array().into_iter().flatten() {
            let kinds: Vec<&str> = t["kind"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            let name = t["name"].as_str().unwrap_or_default().to_string();
            let src = t["src_path"].as_str().and_then(rel).unwrap_or_default();
            let needs_features = t["required-features"]
                .as_array()
                .is_some_and(|f| !f.is_empty());
            let tested = t["test"].as_bool().unwrap_or(true) && !needs_features;
            if kinds
                .iter()
                .any(|k| k.ends_with("lib") || *k == "proc-macro")
            {
                if tested {
                    mine.push((Kind::Lib, vec![src]));
                }
                if t["doctest"].as_bool().unwrap_or(true) {
                    mine.push((Kind::Doc, vec![name.replace('-', "_")]));
                }
            } else if kinds.contains(&"bin") {
                if tested {
                    bins.push(src);
                }
            } else if kinds.contains(&"test") && tested {
                mine.push((Kind::Test(name), vec![src]));
            }
        }
        if !bins.is_empty() {
            mine.push((Kind::Bins, bins));
        }
        for (kind, srcs) in mine {
            out.push(Unit {
                pkg: pkg.clone(),
                kind,
                srcs,
                dir: dir.clone(),
            });
        }
    }
    Some(out)
}

/// One executable's part of a log.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Block {
    /// The file after `Running` (without `unittests`), or the crate after `Doc-tests`.
    key: String,
    doc: bool,
    /// From its header line to its `test result:` line.
    text: String,
    done: bool,
}

fn header(line: &str) -> Option<(String, bool)> {
    let l = line.trim();
    if let Some(rest) = l.strip_prefix("Doc-tests ") {
        return Some((rest.trim().to_string(), true));
    }
    let rest = l.strip_prefix("Running ")?;
    let rest = rest.strip_prefix("unittests ").unwrap_or(rest);
    let path = rest.rsplit_once(" (").map_or(rest, |(p, _)| p);
    Some((path.trim().to_string(), false))
}

fn blocks(log: &str) -> Vec<Block> {
    let mut out: Vec<Block> = vec![];
    let mut open = false;
    for line in log.split_inclusive('\n') {
        if let Some((key, doc)) = header(line) {
            out.push(Block {
                key,
                doc,
                text: String::new(),
                done: false,
            });
            open = true;
        }
        if !open {
            continue;
        }
        let Some(b) = out.last_mut() else { continue };
        b.text.push_str(line);
        if line.trim_start().starts_with("test result:") {
            b.done = true;
            open = false;
        }
    }
    out
}

/// What to run again, and what to keep.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    /// The finished executables' output from the lost run, in log order.
    pub reused: String,
    /// The argument groups of the executables not finished, as `{tests_for_change}` takes them.
    pub rerun: Vec<String>,
    pub done: usize,
    pub total: usize,
}

impl Plan {
    /// For the step's note.
    pub fn note(&self) -> String {
        format!(
            "lost host after {} of {} executables; reran {}",
            self.done,
            self.total,
            self.total - self.done
        )
    }
}

/// The plan for the lost run's `log` over `units`, or `None` when nothing finished, everything
/// did (the exit status is unknown then), or a log block names no unit.
pub fn plan(units: &[Unit], log: &str) -> Option<Plan> {
    let bs = blocks(log);
    let matches = |u: &Unit, b: &Block| {
        if u.kind == Kind::Doc {
            return b.doc && u.srcs.first() == Some(&b.key);
        }
        !b.doc
            && u.srcs
                .iter()
                .any(|s| *s == b.key || (!u.dir.is_empty() && *s == format!("{}/{}", u.dir, b.key)))
    };
    let mut reused = String::new();
    let mut groups: BTreeMap<String, Group> = BTreeMap::new();
    let (mut done, mut total) = (0, 0);
    let mut used = vec![false; bs.len()];
    for u in units {
        total += 1;
        let mine: Vec<usize> = (0..bs.len()).filter(|i| matches(u, &bs[*i])).collect();
        let finished = u.srcs.iter().all(|s| {
            let one = Unit {
                srcs: vec![s.clone()],
                ..u.clone()
            };
            mine.iter().any(|i| bs[*i].done && matches(&one, &bs[*i]))
        });
        if finished {
            done += 1;
            for i in mine.into_iter().filter(|i| bs[*i].done) {
                used[i] = true;
            }
        } else {
            let g = groups.entry(u.pkg.clone()).or_default();
            match &u.kind {
                Kind::Lib => g.lib = true,
                Kind::Bins => g.bins = true,
                Kind::Doc => g.doc = true,
                Kind::Test(t) => {
                    g.tests.insert(t.clone());
                }
            }
        }
    }
    if done == 0 || done == total {
        return None;
    }
    for (b, u) in bs.iter().zip(&used) {
        if *u {
            reused.push_str(&b.text);
            if !reused.ends_with('\n') {
                reused.push('\n');
            }
        }
    }
    let rerun = render(&groups);
    (!rerun.is_empty()).then_some(Plan {
        reused,
        rerun,
        done,
        total,
    })
}

/// The command template a suite's rerun of the unfinished targets is rendered from: its
/// `dev_command` when that takes `{tests_for_change}`, else (with `resume_after_loss`) its
/// `command` with the `--workspace` flag replaced by it. `None` when the command that ran
/// selects targets itself, or is not a plain whole-workspace `cargo test`, so the unit list
/// would not be what it ran.
pub fn template(suite: &Suite) -> Option<String> {
    if suite.narrowed {
        return None;
    }
    let ran = &suite.command;
    let flagged = [
        " --lib",
        " --bins",
        " --bin ",
        " --tests",
        " --test ",
        " --doc",
        " --exclude",
        " -p ",
        " --package",
        " --example",
        " --bench",
        " --features",
        " --no-default-features",
        " --release",
        " --profile",
    ];
    if !ran.contains("cargo test") || !ran.contains("--workspace") {
        return None;
    }
    if ran.contains('|') || ran.contains("&&") || ran.contains(';') {
        return None;
    }
    if flagged.iter().any(|f| ran.contains(f)) {
        return None;
    }
    if let Some(dc) = suite.dev_command.as_ref().filter(|d| scope::wants_tests(d)) {
        return Some(dc.clone());
    }
    suite
        .resume_after_loss
        .then(|| ran.replacen(" --workspace", &format!(" {}", scope::TESTS_FOR_CHANGE), 1))
}

/// The command for the fresh host: `template` for the unfinished targets.
pub fn command(template: &str, plan: &Plan) -> String {
    scope::expand(template, &Narrow::Targets(plan.rerun.clone()), false)
}

/// The plan for a lost run: its log at `log`, the workspace at `cwd`. `None` runs everything.
pub fn plan_for(cwd: &Path, log: &Path) -> Option<Plan> {
    let text = crate::jobs::read_lossy(log).ok()?;
    if !text.contains("test result:") {
        return None;
    }
    let out = std::process::Command::new("cargo")
        .args([
            "metadata",
            "--no-deps",
            "--format-version",
            "1",
            "--offline",
        ])
        .current_dir(cwd)
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let meta: Value = serde_json::from_slice(&out.stdout).ok()?;
    let root = std::fs::canonicalize(cwd).ok()?;
    plan(&units(&meta, &root)?, &text)
}

/// The output and exit status to judge: the lost run's finished blocks, then the fresh run's
/// output. A finished block that failed makes the status non-zero even if the rerun was green.
pub fn combine(reused: &str, code: i32, output: &str) -> (i32, String) {
    let code = if code == 0 && reused.contains("test result: FAILED") {
        101
    } else {
        code
    };
    (code, format!("{reused}{output}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mason_testing::{judge, parse::parse, ParserKind, Verdict};

    fn meta() -> Value {
        let t = |kind: &str, name: &str, src: &str| {
            serde_json::json!({"kind": [kind], "name": name, "src_path": format!("/w/{src}"),
                "test": true, "doctest": kind == "lib"})
        };
        serde_json::json!({
            "workspace_members": [],
            "packages": [
                {"id": "a", "name": "a", "manifest_path": "/w/crates/a/Cargo.toml",
                 "targets": [t("lib", "a", "crates/a/src/lib.rs"),
                             t("test", "one", "crates/a/tests/one.rs"),
                             t("test", "two", "crates/a/tests/two.rs")]},
                {"id": "b", "name": "b-x", "manifest_path": "/w/crates/b/Cargo.toml",
                 "targets": [t("bin", "b-x", "crates/b/src/main.rs")]}
            ]
        })
    }

    fn block(src: &str, tests: &[&str], end: bool) -> String {
        let mut s = format!("     Running {src} (target/debug/deps/x-0123456789abcdef)\n");
        s.push_str(&format!("running {} tests\n", tests.len()));
        for t in tests {
            s.push_str(&format!("test {t} ... ok\n"));
        }
        if end {
            s.push_str("\ntest result: ok. 1 passed; 0 failed; 0 ignored\n\n");
        }
        s
    }

    fn all() -> Vec<Unit> {
        units(&meta(), Path::new("/w")).unwrap()
    }

    #[test]
    fn a_log_cut_off_in_the_third_of_five_executables_has_two_done() {
        let us = all();
        assert_eq!(us.len(), 5, "{us:?}");
        let log = format!(
            "   Compiling a\n{}{}{}",
            block("unittests crates/a/src/lib.rs", &["a::t1"], true),
            block("crates/a/tests/one.rs", &["one::t"], true),
            block("crates/a/tests/two.rs", &["two::t"], false),
        );
        let p = plan(&us, &log).unwrap();
        assert_eq!((p.done, p.total), (2, 5));
        assert_eq!(
            p.rerun,
            vec![
                "-p a --doc --test two".to_string(),
                "-p b-x --bins".to_string()
            ]
        );
        assert!(p.reused.contains("a::t1") && p.reused.contains("one::t"));
        assert!(!p.reused.contains("two::t"), "the cut-off one runs whole");
        assert_eq!(p.note(), "lost host after 2 of 5 executables; reran 3");
        let cmd = command("cargo test {tests_for_change}", &p);
        assert!(cmd.contains("cargo test -p a --doc --test two"), "{cmd}");
        assert!(cmd.contains("cargo test -p b-x --bins"), "{cmd}");
    }

    #[test]
    fn paths_relative_to_the_package_and_doc_tests_are_placed() {
        let us = all();
        let log = format!(
            "{}Doc-tests a\nrunning 0 tests\n\ntest result: ok. 0 passed\n",
            block("unittests src/lib.rs", &["x"], true)
        );
        let p = plan(&us, &log).unwrap();
        assert_eq!(p.done, 2);
        assert!(
            p.rerun.iter().all(|r| !r.contains("--doc")),
            "{:?}",
            p.rerun
        );
    }

    #[test]
    fn nothing_or_everything_finished_gives_no_plan() {
        let us = all();
        assert!(plan(&us, "   Compiling a\n").is_none());
        assert!(plan(&us, &block("crates/a/tests/two.rs", &["t"], false)).is_none());
    }

    #[test]
    fn a_red_test_in_a_done_executable_keeps_the_combined_verdict_red() {
        let reused = "     Running crates/a/tests/one.rs (x)\ntest one::bad ... FAILED\n\ntest result: FAILED. 0 passed; 1 failed\n";
        let fresh = "test two::ok ... ok\n\ntest result: ok. 1 passed\n";
        let (code, text) = combine(reused, 0, fresh);
        assert_eq!(code, 101);
        let mut s = Suite::new("cargo", "cargo test", ParserKind::Regex);
        s.pass_regex = Some(r"^test (.+) \.\.\. ok$".into());
        s.fail_regex = Some(r"^test (.+) \.\.\. FAILED$".into());
        let r = parse(s.parser, &text, &s).unwrap();
        assert_eq!(r.executed(), 2);
        assert!(matches!(
            judge(&r, 2, Some(code)),
            Verdict::Red { failures, .. } if failures == vec!["one::bad".to_string()]
        ));
        let (code, text) = combine("", 0, fresh);
        assert_eq!((code, text.as_str()), (0, fresh));
    }

    #[test]
    fn only_a_plain_whole_workspace_command_is_resumable() {
        let mut s = Suite::new(
            "c",
            "cargo test --workspace --no-fail-fast",
            ParserKind::Regex,
        );
        s.dev_command = Some("cargo test {tests_for_change}".into());
        assert_eq!(
            template(&s).as_deref(),
            Some("cargo test {tests_for_change}")
        );
        s.dev_command = None;
        assert_eq!(template(&s), None);
        s.resume_after_loss = true;
        assert_eq!(
            template(&s).as_deref(),
            Some("cargo test {tests_for_change} --no-fail-fast")
        );
        s.command = "cargo test --workspace --lib".into();
        assert_eq!(template(&s), None);
        s.command = "cargo test --workspace".into();
        s.narrowed = true;
        assert_eq!(template(&s), None);
    }
}
