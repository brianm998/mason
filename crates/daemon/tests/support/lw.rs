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

const CHECKS: &str = r#"#!/bin/sh
n="$1"
[ -n "$(find . -name "slow.$n" -not -path "./.git/*")" ] && sleep 3
echo "1..1"
if [ -n "$(find . -name "red.$n" -not -path "./.git/*")" ]; then echo "not ok 1 - $n"; exit 1; fi
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
    for n in ["alpha", "beta", "integration", "smoke"] {
        s.push_str(&format!(
            "\n[suite.{n}]\ncommand = \"sh checks.sh {n}\"\nparser = \"tap\"\nmin_tests = 1\nlevel = \"unit\"\n"
        ));
    }
    s
}

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
    let s = Scratch::new("true", json!({"steps": {}}));
    s.write_project("scratch", "true", &extra(&s));
    let definition = def();
    let mut all: Vec<(&str, &str)> = vec![
        (".mason/mason.toml", &definition),
        (".mason/landing.toml", landing_toml),
        ("checks.sh", CHECKS),
        ("shared.txt", "base\n"),
    ];
    all.extend_from_slice(files);
    s.push_files(&all, "project setup");
    sh(&s.repo, &["fetch", "-q", "origin"]);
    let d = s.start_with(|c| c.env_retry = Duration::from_millis(200));
    // Nothing is claimed by the scheduler: the test places tasks itself.
    mason_daemon::attention::pause(&d, "", "claims", "test", 3600).unwrap();
    W { s, d }
}

impl W {
    /// A task already in its landing step, with a branch carrying `files`.
    pub fn task(&self, title: &str, files: &[(&str, &str)]) -> TaskId {
        let t = file_task_as(&self.d, title, "x", true, TrustSource::Owner, false);
        let pid = self.d.project_id_by_name("scratch").unwrap();
        let br = format!("mason/scratch/t{}", t.0);
        let tmp = self.s.root.path().join(format!("mk{}", t.0));
        // The remote's main may have moved: start from it.
        sh(&self.s.repo, &["fetch", "-q", "origin"]);
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
        sh(
            &self.s.repo,
            &["worktree", "remove", "--force", tmp.to_str().unwrap()],
        );
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
