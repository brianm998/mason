// SPDX-License-Identifier: GPL-3.0-or-later
#![allow(dead_code)]

//! A world for landing tests: tasks placed straight into the landing step, real
//! git, real suites, the real coordinator.

use super::*;
use mason_core::*;
use mason_daemon::Daemon;
use mason_landing::git;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

pub const CHECKS: &str = r#"#!/bin/sh
n="$1"
# `hang.NAME` holds a path: record this shell's pid there, then block until killed.
f="$(find . -name "hang.$n" -not -path "./.git/*" | head -1)"
if [ -n "$f" ]; then echo $$ > "$(cat "$f")"; sleep 600; fi
[ -n "$(find . -name "slow.$n" -not -path "./.git/*")" ] && sleep 3
echo "1..1"
if [ -n "$(find . -name "red.$n" -not -path "./.git/*")" ]; then echo "not ok 1 - $n"; exit 1; fi
if [ "$n" = integration ] && [ -n "$(find . -name joint.a -not -path "./.git/*")" ] && [ -n "$(find . -name joint.b -not -path "./.git/*")" ]; then echo "not ok 1 - $n (joint)"; exit 1; fi
echo "ok 1 - $n"
"#;

fn def() -> String {
    let mut s = String::from(
        r#"schema = 1
start = "develop"

[step.develop]
type = "agent"
category = "developing"
next = "land"

[step.land]
type = "landing"
category = "landing"
on_red = "develop"
max_rounds = 3

[impact]
"a/**" = ["alpha"]
"b/**" = ["beta"]
"c/**" = ["alpha"]
"#,
    );
    for n in ["alpha", "beta", "integration", "smoke", "lint"] {
        s.push_str(&format!(
            "\n[suite.{n}]\ncommand = \"sh checks.sh {n}\"\nparser = \"tap\"\nmin_tests = 1\nlevel = \"unit\"\n"
        ));
    }
    s
}

/// What each suite's `build` runs: red when a file `red.build.NAME` is anywhere in the tree,
/// and red when `joint.a` and `joint.b` are both there (two changes that merge cleanly and
/// do not build together).
const BUILDS: &str = r#"#!/bin/sh
n="$1"
has() { [ -n "$(find . -name "$1" -not -path "./.git/*")" ]; }
# `hang.build.NAME` holds a path: record this shell's pid there, then block until killed.
f="$(find . -name "hang.build.$n" -not -path "./.git/*" | head -1)"
if [ -n "$f" ]; then echo $$ > "$(cat "$f")"; sleep 600; fi
if has "red.build.$n"; then
  echo "Compiling $n"
  echo "error[E0061]: this function takes 6 arguments but 5 arguments were supplied"
  exit 1
fi
if has joint.a && has joint.b; then
  echo "Compiling $n"
  echo "error[E0599]: the two changes do not build together"
  exit 1
fi
echo "built $n"
"#;

pub struct W {
    pub s: Scratch,
    pub d: Arc<Daemon>,
}

pub fn world(landing_toml: &str, files: &[(&str, &str)]) -> W {
    world_with(landing_toml, &|_| String::new(), files)
}

/// `extra`: more of the project file (its `[host]`, `[ci]` tables).
pub fn world_with(
    landing_toml: &str,
    extra: &dyn Fn(&Scratch) -> String,
    files: &[(&str, &str)],
) -> W {
    world_full(landing_toml, extra, files, "true", false)
}

/// Like `world_with`, with the project's test command, and with a `test` step in
/// the definition (`uses = { compile = 1 }`) when `test_step`: landing's re-test
/// then runs that command.
pub fn world_full(
    landing_toml: &str,
    extra: &dyn Fn(&Scratch) -> String,
    files: &[(&str, &str)],
    test_cmd: &str,
    test_step: bool,
) -> W {
    world_full_window(landing_toml, extra, files, test_cmd, test_step, true)
}

/// A world whose landing queue forms a batch as soon as no other task is moving (#731), the
/// way the daemon does outside tests; every other world waits out the window.
pub fn world_collecting(landing_toml: &str) -> W {
    world_full_window(landing_toml, &|_| String::new(), &[], "true", false, false)
}

fn world_full_window(
    landing_toml: &str,
    extra: &dyn Fn(&Scratch) -> String,
    files: &[(&str, &str)],
    test_cmd: &str,
    test_step: bool,
    window_always: bool,
) -> W {
    let cfg = |_: &mut mason_daemon::config::DaemonConfig| {};
    world_configured(
        landing_toml,
        extra,
        files,
        test_cmd,
        test_step,
        window_always,
        &cfg,
    )
}

/// `world_with`, with the daemon's own configuration changed by `cfg`.
pub fn world_cfg(
    landing_toml: &str,
    extra: &dyn Fn(&Scratch) -> String,
    cfg: &dyn Fn(&mut mason_daemon::config::DaemonConfig),
) -> W {
    world_configured(landing_toml, extra, &[], "true", false, true, cfg)
}

#[allow(clippy::too_many_arguments)]
fn world_configured(
    landing_toml: &str,
    extra: &dyn Fn(&Scratch) -> String,
    files: &[(&str, &str)],
    test_cmd: &str,
    test_step: bool,
    window_always: bool,
    cfg: &dyn Fn(&mut mason_daemon::config::DaemonConfig),
) -> W {
    let s = Scratch::new("true", json!({"steps": {}}));
    s.write_project("scratch", test_cmd, &extra(&s));
    let mut definition = def();
    if test_step {
        definition = definition.replacen(
            "category = \"developing\"\nnext = \"land\"",
            "category = \"developing\"\nnext = \"test\"",
            1,
        );
        definition.push_str(
            "\n[step.test]\ntype = \"test\"\ncategory = \"testing\"\nuses = { compile = 1 }\nnext = \"land\"\non_red = \"develop\"\nmax_rounds = 3\n\n[resource.compile]\ncount = 1\n",
        );
    }
    let mut all: Vec<(&str, &str)> = vec![
        (".mason/mason.toml", &definition),
        (".mason/landing.toml", landing_toml),
        ("checks.sh", CHECKS),
        ("shared.txt", "base\n"),
    ];
    all.extend_from_slice(files);
    s.push_files(&all, "project setup");
    sh(&s.repo, &["fetch", "-q", "origin"]);
    let d = s.start_with(|c| {
        c.env_retry = Duration::from_millis(200);
        c.landing_window_always = window_always;
        cfg(c);
    });
    // Nothing is claimed by the scheduler: the test places tasks itself.
    mason_daemon::attention::pause(&d, "", "claims", "test", 3600).unwrap();
    W { s, d }
}

/// A world whose definition has a test step that runs the suites `alpha` and `beta`, each
/// with a `build` (`sh build.sh NAME`, red when `red.build.NAME` is in the tree): what the
/// landing build floor runs when the landing config has no batch tier.
pub fn world_built(landing_toml: &str, files: &[(&str, &str)]) -> W {
    world_built_fast(landing_toml, files, false)
}

/// What `alpha`'s fast tier runs: red, naming the test, when `red.fast` is in the tree or when
/// `joint.a` and `joint.b` both are (a merge that two green tasks each cleared alone).
const FAST: &str = r#"#!/bin/sh
has() { [ -n "$(find . -name "$1" -not -path "./.git/*")" ]; }
echo "1..1"
if has red.fast || { has joint.a && has joint.b; }; then echo "not ok 1 - matrix_events"; exit 1; fi
echo "ok 1 - matrix_events"
"#;

/// `world_built`, and when `fast`, `alpha` declares `fast_command = "sh fast.sh"`.
pub fn world_built_fast(landing_toml: &str, files: &[(&str, &str)], fast: bool) -> W {
    world_built_extra(landing_toml, "", files, fast)
}

/// `world_built_fast`, with `extra` appended to the project file (a `[remote_runs]` table).
pub fn world_built_extra(landing_toml: &str, extra: &str, files: &[(&str, &str)], fast: bool) -> W {
    let s = Scratch::new("true", json!({"steps": {}}));
    s.write_project("scratch", "true", extra);
    let mut definition = def().replacen(
        "category = \"developing\"\nnext = \"land\"",
        "category = \"developing\"\nnext = \"test\"",
        1,
    );
    for n in ["alpha", "beta"] {
        let header = format!("[suite.{n}]\ncommand = \"sh checks.sh {n}\"\n");
        assert!(definition.contains(&header), "suite {n} in the definition");
        definition = definition.replacen(
            &header,
            &format!(
                "[suite.{n}]\nbuild = \"sh build.sh {n}\"\ncommand = \"sh checks.sh {n}\"\n{}",
                if fast && n == "alpha" {
                    "fast_command = \"sh fast.sh\"\n"
                } else {
                    ""
                }
            ),
            1,
        );
    }
    definition.push_str(
        "\n[step.test]\ntype = \"test\"\ncategory = \"testing\"\nsuites = [\"alpha\", \"beta\"]\nuses = { compile = 1 }\nnext = \"land\"\non_red = \"develop\"\nmax_rounds = 3\n\n[resource.compile]\ncount = 1\n",
    );
    let mut all: Vec<(&str, &str)> = vec![
        (".mason/mason.toml", &definition),
        (".mason/landing.toml", landing_toml),
        ("checks.sh", CHECKS),
        ("build.sh", BUILDS),
        ("fast.sh", FAST),
        ("shared.txt", "base\n"),
    ];
    all.extend_from_slice(files);
    s.push_files(&all, "project setup");
    sh(&s.repo, &["fetch", "-q", "origin"]);
    // Debts are paid by hand here: no idle pass to race the explicit payment.
    let d = s.start_with(|c| {
        c.env_retry = Duration::from_millis(200);
        c.debt_interval = Duration::ZERO;
    });
    mason_daemon::attention::pause(&d, "", "claims", "test", 3600).unwrap();
    W { s, d }
}

/// What `integration`'s `rerun` runs (#682): it says which tests it was asked for, then
/// checks as the suite does.
const RERUN: &str = r#"#!/bin/sh
echo "rerun asked for: $*"
exec sh checks.sh integration
"#;

/// `world`, with `integration` given a `build` (`sh build.sh integration`) and a `rerun`
/// (`sh rerun.sh {tests}`), the way `cargo-workspace` has one.
pub fn world_rerun(landing_toml: &str, files: &[(&str, &str)]) -> W {
    let s = Scratch::new("true", json!({"steps": {}}));
    s.write_project("scratch", "true", "");
    let header = "[suite.integration]\ncommand = \"sh checks.sh integration\"\n";
    let mut definition = def();
    assert!(definition.contains(header));
    definition = definition.replacen(
        header,
        "[suite.integration]\nbuild = \"sh build.sh integration\"\ncommand = \"sh checks.sh integration\"\nrerun = \"sh rerun.sh {tests}\"\n",
        1,
    );
    let mut all: Vec<(&str, &str)> = vec![
        (".mason/mason.toml", &definition),
        (".mason/landing.toml", landing_toml),
        ("checks.sh", CHECKS),
        ("build.sh", BUILDS),
        ("rerun.sh", RERUN),
        ("shared.txt", "base\n"),
    ];
    all.extend_from_slice(files);
    s.push_files(&all, "project setup");
    sh(&s.repo, &["fetch", "-q", "origin"]);
    let d = s.start_with(|c| c.env_retry = Duration::from_millis(200));
    mason_daemon::attention::pause(&d, "", "claims", "test", 3600).unwrap();
    W { s, d }
}

/// What `alpha`'s `rerun` runs in [`world_new_tests`] (#812): one line per test name asked for,
/// red for a name with `bad` in it.
const NEWTESTS: &str = r#"#!/bin/sh
bad=0
for n in "$@"; do
  case "$n" in
    *bad*) echo "test m::$n ... FAILED"; bad=1 ;;
    *) echo "test m::$n ... ok" ;;
  esac
done
exit $bad
"#;

/// `world_built_fast` with `alpha` given a `rerun` (`sh newtests.sh {tests}`), the way
/// `cargo-workspace` has one: the landing batch runs the new tests of skipped tasks by name.
pub fn world_new_tests(landing_toml: &str) -> W {
    let s = Scratch::new("true", json!({"steps": {}}));
    s.write_project("scratch", "true", "");
    let mut definition = def().replacen(
        "category = \"developing\"\nnext = \"land\"",
        "category = \"developing\"\nnext = \"test\"",
        1,
    );
    for n in ["alpha", "beta"] {
        let header = format!("[suite.{n}]\ncommand = \"sh checks.sh {n}\"\n");
        assert!(definition.contains(&header), "suite {n} in the definition");
        definition = definition.replacen(
            &header,
            &format!(
                "[suite.{n}]\nbuild = \"sh build.sh {n}\"\ncommand = \"sh checks.sh {n}\"\n{}",
                if n == "alpha" {
                    "rerun = \"sh newtests.sh {tests}\"\n"
                } else {
                    ""
                }
            ),
            1,
        );
    }
    definition.push_str(
        "\n[step.test]\ntype = \"test\"\ncategory = \"testing\"\nsuites = [\"alpha\", \"beta\"]\nuses = { compile = 1 }\nnext = \"land\"\non_red = \"develop\"\nmax_rounds = 3\n\n[resource.compile]\ncount = 1\n",
    );
    let all: Vec<(&str, &str)> = vec![
        (".mason/mason.toml", &definition),
        (".mason/landing.toml", landing_toml),
        ("checks.sh", CHECKS),
        ("build.sh", BUILDS),
        ("newtests.sh", NEWTESTS),
        ("shared.txt", "base\n"),
    ];
    s.push_files(&all, "project setup");
    sh(&s.repo, &["fetch", "-q", "origin"]);
    let d = s.start_with(|c| {
        c.env_retry = Duration::from_millis(200);
        c.debt_interval = Duration::ZERO;
    });
    mason_daemon::attention::pause(&d, "", "claims", "test", 3600).unwrap();
    W { s, d }
}

impl W {
    /// A task already in its landing step, with a branch carrying `files`.
    pub fn task(&self, title: &str, files: &[(&str, &str)]) -> TaskId {
        self.task_with(title, files, |_, _| {})
    }

    /// Like `task`, but runs `before` once the task exists and before it is admitted.
    pub fn task_with(
        &self,
        title: &str,
        files: &[(&str, &str)],
        before: impl FnOnce(&W, TaskId),
    ) -> TaskId {
        let t = file_task_as(&self.d, title, "x", true, TrustSource::Owner, false);
        let pid = self.d.project_id_by_name("scratch").unwrap();
        let br = format!("mason/scratch/t{}", t.0);
        let tmp = self.s.root.path().join(format!("mk{}", t.0));
        // The remote's main may have moved: start from it.
        retry_git(&self.s.repo, &["fetch", "-q", "origin"]);
        add_worktree(&self.s.repo, &br, &tmp);
        for (p, body) in files {
            let f = tmp.join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, body).unwrap();
        }
        sh(&tmp, &["add", "-A"]);
        sh(
            &tmp,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-qm",
                title,
            ],
        );
        retry_git(
            &self.s.repo,
            &["worktree", "remove", "--force", tmp.to_str().unwrap()],
        );
        before(self, t);
        let def = self.d.current_def(pid);
        self.d
            .emit(EventKind::Admitted {
                task: t,
                entry: Some(Entry {
                    def: def.hash.clone(),
                    step: "land".into(),
                    state: State::Landing,
                    route: Route::Explore,
                }),
            })
            .unwrap();
        t
    }

    pub fn sent_back(&self, t: TaskId) -> Option<String> {
        self.d
            .store
            .read_since(0, 100_000)
            .unwrap()
            .iter()
            .find_map(|e| match &e.event.kind {
                EventKind::SentBack { task, reason, .. } if *task == t => Some(reason.clone()),
                _ => None,
            })
    }

    /// The tail of every log under the data dir whose name has `-fast-` in it (the fast tier's
    /// output for a batch), so a red test can say what the tier printed.
    pub fn fast_tier_logs(&self) -> String {
        fn walk(dir: &std::path::Path, out: &mut String) {
            let Ok(rd) = std::fs::read_dir(dir) else {
                return;
            };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.to_string_lossy().contains("-fast-") {
                    let text = std::fs::read_to_string(&p).unwrap_or_default();
                    let tail: Vec<&str> = text.lines().rev().take(60).collect();
                    let tail: Vec<&str> = tail.into_iter().rev().collect();
                    out.push_str(&format!("--- {}\n{}\n", p.display(), tail.join("\n")));
                }
            }
        }
        let mut out = String::new();
        walk(&self.s.data_dir, &mut out);
        out
    }

    pub fn on_main(&self, path: &str) -> bool {
        git::ok(&self.s.origin, &["cat-file", "-e", &format!("main:{path}")])
    }

    pub fn landed(&self, t: TaskId) -> bool {
        state_of(&self.d, t) == State::Landed
    }
}

/// Fake CI: `trigger` runs the integration suite over the pushed commit and records
/// the result (sleeping first if the tree holds `slow.ci`); `status` reads it.
pub fn ci_scripts(root: &Path, origin: &Path) -> (String, String, PathBuf) {
    let state = root.join("ci-state");
    std::fs::create_dir_all(&state).unwrap();
    let log = state.join("triggers.log");
    let trigger = format!(
        "d='{st}/'$MASON_SHA; rm -rf $d; mkdir -p $d/t; git --git-dir='{o}' archive $MASON_SHA | tar -x -C $d/t; \
         echo \"$MASON_REF $MASON_SHA\" >> '{log}'; \
         ( cd $d/t; [ -f slow.ci ] && sleep 3; sh checks.sh integration > $d/out 2>&1; echo $? > $d/rc ) ; echo $MASON_SHA",
        st = state.display(),
        o = origin.display(),
        log = log.display()
    );
    let status = format!(
        "d='{st}/'$MASON_RUN; if [ -f $d/rc ]; then if [ \"$(cat $d/rc)\" = 0 ]; then echo green; else echo red; echo integration; fi; else echo pending; fi",
        st = state.display()
    );
    (trigger, status, log)
}

pub fn q(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

pub fn ci_section(s: &Scratch) -> (String, PathBuf) {
    let (trigger, status, log) = ci_scripts(s.root.path(), &s.origin);
    (
        format!("[ci]\nkind = \"command\"\ntrigger = \"{}\"\nstatus = \"{}\"\npoll_s = 1\ntimeout_s = 120\n", q(&trigger), q(&status)),
        log,
    )
}

pub fn host_section(s: &Scratch, approval: bool, queue: bool) -> String {
    format!(
        "[host]\nkind = \"fake\"\ndir = \"{}\"\nremote = \"{}\"\nchecks = \"sh checks.sh integration\"\nrequire_approval = {approval}\nmerge_queue = {queue}\n",
        s.root.path().join("host-state").display(),
        s.origin.display()
    )
}

/// `git worktree add`, tried again when it loses a race with the daemon's own
/// `git worktree prune` (which can remove the half-made entry).
pub fn add_worktree(repo: &std::path::Path, branch: &str, dir: &std::path::Path) {
    let mut last = String::new();
    for _ in 0..8 {
        let _ = mason_landing::git::run(repo, &["branch", "-D", branch]);
        match mason_landing::git::run(
            repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                branch,
                dir.to_str().unwrap(),
                "origin/main",
            ],
        ) {
            Ok(_) => return,
            Err(e) => last = e.to_string(),
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    panic!("git worktree add kept failing: {last}");
}

/// A repo command of the test's own, tried again when it loses a race with the daemon's
/// background git work (a ref or index lock held for a moment). Bounded, no timing assertion.
pub fn retry_git(repo: &std::path::Path, args: &[&str]) -> String {
    let mut last = String::new();
    for _ in 0..8 {
        match mason_landing::git::run(repo, args) {
            Ok(out) => return out,
            Err(e) => last = e.to_string(),
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    panic!("git {args:?} kept failing: {last}");
}
