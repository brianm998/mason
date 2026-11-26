// SPDX-License-Identifier: GPL-3.0-or-later

//! 2026-11-13: nine batches landed on a `develop` that did not compile, because nothing built
//! them. The owner had skipped every task's test step, MASON's own landing config has no batch
//! tier, and nothing refused the conflict markers one agent had committed. These tests are the
//! two guards that stop it: landing builds a batch that no suite verifies (the build floor),
//! and a branch that adds conflict markers goes back before it joins a batch.
//!
//! Real git, real suites, the real coordinator, as in `landing.rs`; no timing is asserted.

use crate::support;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use support::lw::*;
use support::*;

/// A merge that left its markers behind, as an agent commits it. Built from repeated
/// characters so this file holds no marker line itself.
fn conflicted() -> String {
    format!(
        "keep\n{} HEAD\nmine\n{}\ntheirs\n{} origin/develop\n",
        "<".repeat(7),
        "=".repeat(7),
        ">".repeat(7)
    )
}

/// The tail of every file under `dir` whose path has `part` in it (a build floor's logs and
/// its step hosts' records), so a timed-out wait can say what the floor was doing.
fn logs_named(dir: &std::path::Path, part: &str) -> String {
    fn walk(dir: &std::path::Path, part: &str, out: &mut String) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, part, out);
            } else if p.to_string_lossy().contains(part) {
                let text =
                    String::from_utf8_lossy(&std::fs::read(&p).unwrap_or_default()).into_owned();
                let tail: Vec<&str> = text.lines().rev().take(20).collect();
                let tail: Vec<&str> = tail.into_iter().rev().collect();
                out.push_str(&format!("--- {}\n{}\n", p.display(), tail.join("\n")));
            }
        }
    }
    let mut out = String::new();
    walk(dir, part, &mut out);
    out
}

#[test]
fn a_batch_that_does_not_build_is_not_landed_and_the_task_that_broke_it_goes_back() {
    // No batch tier: the floor is the only thing that builds the union.
    let w = world_built("window = \"2s\"\nmax_size = 5\n", &[]);
    let fine = w.task("beta only", &[("b/ok.txt", "1")]);
    let broken = w.task("alpha does not build", &[("a/red.build.alpha", "x")]);
    // The red runs seen on GitHub were timeouts of these waits (no verdict in 900 s, s774), so
    // a timeout says where the batch stood: its rows, both tasks, and the floor's build logs.
    let report = || {
        format!(
            "broken sent back: {:?}\nfine sent back: {:?}\nbatches: {:?}\nfloor and fast-tier logs:\n{}{}",
            w.sent_back(broken),
            w.sent_back(fine),
            w.d.store.batches(Some("scratch"), 10).unwrap(),
            logs_named(&w.s.data_dir, "-floor-"),
            w.fast_tier_logs()
        )
    };
    let wait = |what: &str, f: &dyn Fn() -> bool| {
        if let Err(e) = try_wait_for_exact(what, 900, 50, f) {
            panic!("{e}\n{}", report());
        }
    };
    wait("the task that builds to land", &|| w.landed(fine));
    wait("the one that does not to go back", &|| {
        w.sent_back(broken).is_some()
    });
    let why = w.sent_back(broken).unwrap();
    assert!(why.contains("alpha"), "{why}\n{}", report());
    assert!(w.on_main("b/ok.txt"), "{}", report());
    assert!(
        !w.on_main("a/red.build.alpha"),
        "a batch that does not build must not land\n{}",
        report()
    );
    assert!(
        w.sent_back(fine).is_none(),
        "the innocent task is not parked\n{}",
        report()
    );
}

#[test]
fn two_changes_that_merge_cleanly_and_do_not_build_together_do_not_land() {
    // Each builds alone; the union does not (a call and the definition it calls, changed by
    // two tasks). Nothing can pin it on one of them: both go back, nothing lands.
    // The batch forms when the second task is admitted (max_size), never on the timer, so a
    // slow setup of the second task cannot let the first land alone.
    let w = world_built("window = \"20m\"\nmax_size = 2\n", &[]);
    let left = w.task("left", &[("x/joint.a", "1")]);
    let right = w.task("right", &[("x/joint.b", "1")]);
    wait_for("both to go back", 120, || {
        w.sent_back(left).is_some() && w.sent_back(right).is_some()
    });
    assert!(!w.on_main("x/joint.a") && !w.on_main("x/joint.b"));
    let why = w.sent_back(left).unwrap();
    assert!(why.contains("alpha") || why.contains("beta"), "{why}");
}

#[test]
fn a_base_that_does_not_build_holds_the_batch_and_blames_nobody() {
    // The base itself is red in `alpha`. A change under `x/` is named by no line of the impact
    // map, so everything is built: the red is the base's, not the task's.
    let w = world_built(
        "window = \"2s\"\n",
        &[("a/red.build.alpha", "the base does not build")],
    );
    let t = w.task("innocent", &[("x/one.txt", "1")]);
    let held = || {
        w.d.store
            .batches(Some("scratch"), 10)
            .unwrap()
            .iter()
            .any(|b| b.phase == "failed" && b.note.contains("does not build"))
    };
    if let Err(e) = try_wait_for_exact("the batch to be held", 900, 50, held) {
        // Name the path taken: which row, which note, which alerts.
        let rows: Vec<String> =
            w.d.store
                .batches(Some("scratch"), 10)
                .unwrap()
                .iter()
                .map(|b| format!("phase={:?} note={:?}", b.phase, b.note))
                .collect();
        panic!(
            "{e}\nbatch rows: {rows:#?}\nalerts: {:?}",
            w.d.landing().attention()
        );
    }
    assert!(
        !w.on_main("x/one.txt"),
        "nothing lands on a base that does not build"
    );
    assert!(
        w.sent_back(t).is_none(),
        "nobody is blamed for the base's red"
    );
    let alerts = w.d.landing().attention();
    assert!(
        alerts
            .iter()
            .any(|(k, _, text)| k.ends_with(":redbase") && text.contains("does not build")),
        "the owner is told: {alerts:?}"
    );
}

#[test]
fn a_tree_that_already_passed_the_suite_is_not_built_again() {
    // `alpha` would not build, but this exact tree is on record as having passed it (the
    // evidence a task's own test step leaves): identical code never tests twice.
    let w = world_built("window = \"5s\"\n", &[]);
    let t = w.task("proven", &[("a/red.build.alpha", "x")]);
    let tree = sh(
        &w.s.repo,
        &["rev-parse", &format!("mason/scratch/t{}^{{tree}}", t.0)],
    );
    w.d.store
        .put_evidence(&tree, "alpha", 1, 1, t.0, mason_daemon::daemon::now_ms())
        .unwrap();
    wait_for("it to land without a build", 120, || w.landed(t));
    assert!(w.on_main("a/red.build.alpha"));
}

#[test]
fn a_project_with_a_batch_tier_is_left_to_it() {
    // The owner named what verifies a batch: the floor adds nothing to it.
    let w = world_built(
        "window = \"2s\"\n[[tier]]\nname = \"batch\"\nsuites = [\"integration\"]\n",
        &[],
    );
    let t = w.task("alpha does not build", &[("a/red.build.alpha", "x")]);
    wait_for("it to land: only `integration` is run", 120, || w.landed(t));
    assert!(w.on_main("a/red.build.alpha"));
}

#[test]
fn a_branch_that_adds_conflict_markers_goes_back_before_it_joins_a_batch() {
    let w = world("window = \"2s\"\n", &[]);
    let bad = w.task("a merge left its markers", &[("x/m.txt", &conflicted())]);
    let fine = w.task("clean", &[("y/c.txt", "1")]);
    wait_for("the clean task to land", 90, || w.landed(fine));
    wait_for("the other to go back", 60, || w.sent_back(bad).is_some());
    let why = w.sent_back(bad).unwrap();
    assert!(
        why.contains("conflict markers") && why.contains("x/m.txt:2"),
        "{why}"
    );
    assert!(w.on_main("y/c.txt"));
    assert!(!w.on_main("x/m.txt"), "markers must not reach the base");
    // It was never in a batch: the only batch has the clean one.
    let rows = w.d.store.batches(Some("scratch"), 10).unwrap();
    let never = bad.0.to_string();
    assert!(
        rows.iter().all(|b| !b.tasks.split(',').any(|t| t == never)),
        "{rows:?}"
    );
}

#[test]
fn a_branch_that_adds_a_secret_or_a_banned_word_goes_back_and_an_allowed_line_is_logged() {
    let w = world("window = \"2s\"\n", &[]);
    std::fs::create_dir_all(&w.d.cfg.config_dir).unwrap();
    std::fs::write(w.d.cfg.config_dir.join("banned-words.txt"), "zebraword\n").unwrap();
    // Put together here, so this file holds no token.
    let token = format!("ghp_{}", "d4".repeat(18));
    let leak = format!("keep\nGH={token}\n");
    let allowed = format!("GH={token} # mason:allow-secret\n");
    let secret = w.task("adds a token", &[("x/s.txt", &leak)]);
    let word = w.task("adds a banned word", &[("x/w.txt", "ZebraWord\n")]);
    let fine = w.task("an allowed line", &[("y/a.txt", &allowed)]);
    wait_for("the allowed task to land", 90, || w.landed(fine));
    wait_for("the others to go back", 60, || {
        w.sent_back(secret).is_some() && w.sent_back(word).is_some()
    });
    let why = w.sent_back(secret).unwrap();
    assert!(why.contains("x/s.txt:2") && !why.contains(&token), "{why}");
    let why = w.sent_back(word).unwrap();
    assert!(why.contains("x/w.txt:1"), "{why}");
    assert!(!why.to_lowercase().contains("zebraword"), "{why}");
    assert!(!w.on_main("x/s.txt") && !w.on_main("x/w.txt"));
    let batches = w.d.cfg.task_dir(0).parent().unwrap().join("batches");
    let logs: String = std::fs::read_dir(&batches)
        .unwrap()
        .filter_map(|e| std::fs::read_to_string(e.ok()?.path().join("content-scan.log")).ok())
        .collect();
    assert!(
        logs.contains(&format!("task #{}: y/a.txt:1", fine.0)) && !logs.contains(&token),
        "{logs}"
    );
}

// ---- #816: the floor builds on this Mac while the switch says GitHub --------------------------

const WORKFLOW: &str = "name: mason-run\non:\n  workflow_dispatch:\n";

fn floor_github_world(landing: &str, files: &[(&str, &str)]) -> (W, Arc<AtomicUsize>) {
    let (url, n) = crate::landing::silent_github();
    std::env::set_var("MASON_T816_TOKEN", "x");
    let extra = format!(
        "[remote_runs]\nkind = \"github-actions\"\napi = \"{url}\"\nrepo = \"o/r\"\ntoken = \"env:MASON_T816_TOKEN\"\nruns_on = \"ubuntu-latest\"\n"
    );
    // The snapshot needs the base's copy of the workflow, as in the real repo.
    let mut all = vec![(".github/workflows/mason-run.yml", WORKFLOW)];
    all.extend_from_slice(files);
    let w = world_built_extra(landing, &extra, &all, false);
    mason_daemon::place::set_global(&w.d, mason_daemon::place::Mode::Remote, "test").unwrap();
    (w, n)
}

#[test]
fn the_floor_builds_here_while_the_switch_says_github_and_lands() {
    let (w, github) = floor_github_world("window = \"1s\"\n", &[]);
    let t = w.task("alpha", &[("a/ok.txt", "1")]);
    wait_for("the batch to land", 90, || w.landed(t));
    assert_eq!(github.load(Ordering::SeqCst), 0, "the floor reached GitHub");
}

#[test]
fn a_floor_set_to_github_goes_there() {
    let (w, github) = floor_github_world("floor_where = \"github\"\nwindow = \"1s\"\n", &[]);
    let _t = w.task("alpha", &[("a/ok.txt", "1")]);
    wait_for("the floor to reach GitHub", 90, || {
        github.load(Ordering::SeqCst) > 0
    });
}

#[test]
fn a_floor_red_built_here_parks_the_task() {
    let (w, github) = floor_github_world("window = \"1s\"\n", &[]);
    let t = w.task("alpha does not build", &[("a/red.build.alpha", "x")]);
    wait_for("the task to be sent back", 90, || w.sent_back(t).is_some());
    assert!(!w.on_main("a/red.build.alpha"));
    assert_eq!(github.load(Ordering::SeqCst), 0);
}
