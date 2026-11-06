// SPDX-License-Identifier: GPL-3.0-or-later

//! Stage 15: each landing mode end to end against a local fake git host and a
//! fake CI (scripts over the bare repository): green, red with a culprit, and,
//! in the CI modes, a base that moves while CI runs.

mod support;
use mason_core::*;
use mason_landing::git;
use std::path::{Path, PathBuf};
use std::time::Duration;
use support::lw::*;
use support::*;

fn toml(mode: &str, more: &str) -> String {
    format!("mode = \"{mode}\"\nwindow = \"30s\"\nmax_size = 2\nmax_fold = 4\n[[tier]]\nname = \"batch\"\nsuites = [\"alpha\"]\n{more}")
}

fn remote_branches(s: &Scratch) -> Vec<String> {
    sh(
        &s.origin,
        &["for-each-ref", "--format=%(refname:short)", "refs/heads"],
    )
    .lines()
    .filter(|b| b.starts_with("mason/ci/") || b.starts_with("mason/pr/"))
    .map(String::from)
    .collect()
}

fn triggers(log: &Path) -> usize {
    std::fs::read_to_string(log).map_or(0, |t| t.lines().count())
}

// ---- direct-ci ------------------------------------------------------------------------------

fn ci_world(mode: &str) -> (W, PathBuf) {
    let log = std::sync::Mutex::new(PathBuf::new());
    let w = world_with(
        &toml(mode, ""),
        &|s| {
            let (sec, l) = ci_section(s);
            *log.lock().unwrap() = l;
            sec
        },
        &[],
    );
    let l = log.lock().unwrap().clone();
    (w, l)
}

#[test]
fn direct_ci_pushes_a_candidate_runs_ci_and_fast_forwards_the_base() {
    let (w, log) = ci_world("direct-ci");
    let a = w.task("one", &[("x/a.txt", "1")]);
    let b = w.task("two", &[("x/b.txt", "2")]);
    wait_for("both to land", 90, || w.landed(a) && w.landed(b));
    assert_eq!(triggers(&log), 1, "one CI run for the whole batch");
    let t = std::fs::read_to_string(&log).unwrap();
    assert!(t.starts_with("mason/ci/scratch-b1-"), "{t}");
    assert!(w.on_main("x/a.txt") && w.on_main("x/b.txt"));
    // A fast-forward: no merge commit at the tip other than the batch's own merges, and the
    // candidate ref is gone.
    wait_for("the candidate to be deleted", 30, || {
        remote_branches(&w.s).is_empty()
    });
    assert!(w.d.store.open_refs("scratch").unwrap().is_empty());
}

#[test]
fn direct_ci_red_goes_to_the_same_ladder_and_only_the_culprit_is_parked() {
    let (w, log) = ci_world("direct-ci");
    let ok = w.task("fine", &[("x/ok.txt", "1")]);
    let bad = w.task("breaks CI", &[("red.integration", "x")]);
    wait_for("the good one to land", 120, || w.landed(ok));
    wait_for("the culprit to be sent back", 60, || {
        w.sent_back(bad).is_some()
    });
    assert!(w.on_main("x/ok.txt") && !w.on_main("red.integration"));
    assert!(
        triggers(&log) >= 3,
        "CI ran for the batch, the rerun and the bisection: {}",
        triggers(&log)
    );
    wait_for("every candidate to be deleted", 30, || {
        remote_branches(&w.s).is_empty()
    });
}

#[test]
fn direct_ci_folds_in_a_moved_base_and_runs_ci_again() {
    let (w, log) = ci_world("direct-ci");
    let t = w.task("slow one", &[("x/a.txt", "1"), ("slow.ci", "x")]);
    wait_for("CI to start", 60, || triggers(&log) >= 1);
    // The base moves while CI runs.
    w.s.push_files(&[("moved.txt", "m")], "somebody else lands");
    wait_for("it to land", 120, || w.landed(t));
    assert_eq!(triggers(&log), 2, "CI ran again on the new tip");
    assert!(w.on_main("moved.txt") && w.on_main("x/a.txt"));
}

// ---- local-only-ci ----------------------------------------------------------------------------

fn local_head(w: &W, rev: &str) -> String {
    sh(&w.s.repo, &["rev-parse", rev])
}

#[test]
fn local_only_ci_leaves_the_base_to_the_owner_and_only_the_candidate_leaves() {
    let (w, log) = ci_world("local-only-ci");
    let before = sh(&w.s.origin, &["rev-parse", "main"]);
    let t = w.task("local", &[("x/a.txt", "1")]);
    wait_for("it to land", 90, || w.landed(t));
    assert_eq!(triggers(&log), 1);
    assert_eq!(
        sh(&w.s.origin, &["rev-parse", "main"]),
        before,
        "the remote's base is the owner's to push"
    );
    assert_eq!(
        sh(&w.s.repo, &["show", "main:x/a.txt"]),
        "1",
        "the local base moved"
    );
    wait_for("the candidate to be deleted", 30, || {
        remote_branches(&w.s).is_empty()
    });
    let _ = local_head(&w, "main");
}

#[test]
fn local_only_ci_red_parks_the_culprit_and_a_moved_local_base_is_folded_in() {
    let (w, log) = ci_world("local-only-ci");
    let ok = w.task("fine", &[("x/ok.txt", "1"), ("slow.ci", "x")]);
    let bad = w.task("breaks CI", &[("red.integration", "x")]);
    wait_for("CI to start", 60, || triggers(&log) >= 1);
    // The owner commits to the local base while CI runs.
    let tmp = w.s.root.path().join("owner-wt");
    sh(
        &w.s.repo,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            tmp.to_str().unwrap(),
            "main",
        ],
    );
    std::fs::write(tmp.join("owner.txt"), "o").unwrap();
    sh(&tmp, &["add", "-A"]);
    sh(
        &tmp,
        &[
            "-c",
            "user.name=o",
            "-c",
            "user.email=o@o",
            "commit",
            "-qm",
            "owner work",
        ],
    );
    let new = sh(&tmp, &["rev-parse", "HEAD"]);
    sh(&w.s.repo, &["update-ref", "refs/heads/main", &new]);
    sh(
        &w.s.repo,
        &["worktree", "remove", "--force", tmp.to_str().unwrap()],
    );
    wait_for("the good one to land", 180, || w.landed(ok));
    wait_for("the culprit to be sent back", 60, || {
        w.sent_back(bad).is_some()
    });
    assert_eq!(sh(&w.s.repo, &["show", "main:owner.txt"]), "o");
    assert_eq!(sh(&w.s.repo, &["show", "main:x/ok.txt"]), "1");
    assert!(git::run(&w.s.repo, &["cat-file", "-e", "main:red.integration"]).is_err());
}

// ---- the pull-request modes ---------------------------------------------------------------------

fn pr_world(mode: &str, approval: bool, queue: bool) -> W {
    world_with(
        &toml(mode, ""),
        &|s| {
            format!("{}[ci]\nkind = \"command\"\ntrigger = \"true\"\nstatus = \"echo green\"\npoll_s = 1\ntimeout_s = 120\n", host_section(s, approval, queue))
        },
        &[],
    )
}

fn prs(w: &W) -> usize {
    std::fs::read_dir(w.s.root.path().join("host-state/prs")).map_or(0, |d| {
        d.flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .count()
    })
}

#[test]
fn pr_each_opens_one_pull_request_per_task_and_merges_each() {
    let w = pr_world("pr-each", false, false);
    let a = w.task("one", &[("x/a.txt", "1")]);
    let b = w.task("two", &[("x/b.txt", "2")]);
    wait_for("both to land", 120, || w.landed(a) && w.landed(b));
    assert_eq!(prs(&w), 2, "one pull request per task");
    assert!(w.on_main("x/a.txt") && w.on_main("x/b.txt"));
    wait_for("the branches to be deleted", 30, || {
        remote_branches(&w.s).is_empty()
    });
}

#[test]
fn pr_batch_opens_one_pull_request_for_the_batch() {
    let w = pr_world("pr-batch", false, false);
    let a = w.task("one", &[("x/a.txt", "1")]);
    let b = w.task("two", &[("x/b.txt", "2")]);
    wait_for("both to land", 120, || w.landed(a) && w.landed(b));
    assert_eq!(prs(&w), 1, "one pull request for the whole batch");
    assert!(w.on_main("x/a.txt") && w.on_main("x/b.txt"));
}

#[test]
fn pr_batch_red_checks_go_to_the_ladder_and_a_required_approval_is_waited_for() {
    let w = pr_world("pr-batch", true, false);
    let ok = w.task("fine", &[("x/ok.txt", "1")]);
    let bad = w.task("breaks checks", &[("red.integration", "x")]);
    // The culprit is found by bisection over pull requests; then the clean one waits for an approval.
    wait_for("the culprit to be sent back", 120, || {
        w.sent_back(bad).is_some()
    });
    wait_for("the alert that an approval is wanted", 60, || {
        mason_daemon::attention::alerts_now(&w.d)
            .iter()
            .any(|(_, k, t)| k == "needs_you" && t.contains("waiting for an approval"))
    });
    assert!(!w.landed(ok), "it must wait for the approval");
    // The reviewer approves the open pull request.
    let dir = w.s.root.path().join("host-state");
    wait_for("the good PR to be approved and merged", 60, || {
        for n in 1..=prs(&w) as u64 {
            mason_adapters::fakehost::FakeHost::approve(&dir, n);
        }
        w.landed(ok)
    });
    assert!(w.on_main("x/ok.txt") && !w.on_main("red.integration"));
    wait_for("every branch to be deleted", 30, || {
        remote_branches(&w.s).is_empty()
    });
    wait_for("the alert to clear", 30, || {
        !mason_daemon::attention::alerts_now(&w.d)
            .iter()
            .any(|(_, _, t)| t.contains("waiting for an approval"))
    });
}

#[test]
fn host_queue_hands_the_pull_request_to_the_hosts_own_queue() {
    let w = pr_world("host-queue", false, true);
    let a = w.task("one", &[("x/a.txt", "1")]);
    let b = w.task("two", &[("x/b.txt", "2")]);
    wait_for("both to land", 120, || w.landed(a) && w.landed(b));
    assert_eq!(prs(&w), 1);
    assert!(w.on_main("x/a.txt") && w.on_main("x/b.txt"));
    let st = std::fs::read_to_string(w.s.root.path().join("host-state/prs/1.json")).unwrap();
    assert!(
        st.contains("\"queued\":true") && st.contains("\"state\":\"merged\""),
        "{st}"
    );
}

#[test]
fn host_queue_red_is_found_before_the_queue_and_never_queued() {
    let w = pr_world("host-queue", false, true);
    let ok = w.task("fine", &[("x/ok.txt", "1")]);
    let bad = w.task("breaks checks", &[("red.integration", "x")]);
    wait_for("the good one to land", 180, || w.landed(ok));
    wait_for("the culprit to be sent back", 60, || {
        w.sent_back(bad).is_some()
    });
    assert!(!w.on_main("red.integration"));
}

#[test]
fn a_ci_mode_without_a_remote_or_ci_section_is_refused_naming_what_is_missing() {
    let w = world("mode = \"direct-ci\"\n", &[]);
    let t = w.task("never", &[("x/n.txt", "1")]);
    wait_state(&w.d, t, State::Failed, 60);
    assert!(task_of(&w.d, t).failure.unwrap().contains("[ci]"));
    let _ = Duration::from_secs(1);
}

// ---- direct and local-only: the same matrix without CI -------------------------------------------

fn plain_world(mode: &str) -> W {
    world(&toml(mode, ""), &[])
}

#[test]
fn plain_direct_lands_a_batch_parks_a_culprit_and_folds_in_a_moved_base() {
    let w = plain_world("direct");
    let ok = w.task("fine", &[("x/ok.txt", "1"), ("a/slow.alpha", "x")]);
    let bad = w.task("breaks the suite", &[("red.alpha", "x")]);
    // The base moves while the suites run.
    w.s.push_files(&[("moved.txt", "m")], "somebody else lands");
    wait_for("the good one to land", 120, || w.landed(ok));
    wait_for("the culprit to be sent back", 60, || {
        w.sent_back(bad).is_some()
    });
    assert!(w.on_main("x/ok.txt") && w.on_main("moved.txt") && !w.on_main("red.alpha"));
}

#[test]
fn plain_local_only_lands_on_the_local_base_and_leaves_the_push_to_the_owner() {
    let w = plain_world("local-only");
    let before = sh(&w.s.origin, &["rev-parse", "main"]);
    let ok = w.task("fine", &[("x/ok.txt", "1")]);
    let bad = w.task("breaks the suite", &[("red.alpha", "x")]);
    wait_for("the good one to land", 120, || w.landed(ok));
    wait_for("the culprit to be sent back", 60, || {
        w.sent_back(bad).is_some()
    });
    assert_eq!(
        sh(&w.s.origin, &["rev-parse", "main"]),
        before,
        "nothing was pushed"
    );
    assert_eq!(sh(&w.s.repo, &["show", "main:x/ok.txt"]), "1");
    assert!(git::run(&w.s.repo, &["cat-file", "-e", "main:red.alpha"]).is_err());
    // The owner commits to the local base; the next landing folds it in.
    let tmp = w.s.root.path().join("owner-wt");
    sh(
        &w.s.repo,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            tmp.to_str().unwrap(),
            "main",
        ],
    );
    std::fs::write(tmp.join("owner.txt"), "o").unwrap();
    sh(&tmp, &["add", "-A"]);
    sh(
        &tmp,
        &[
            "-c",
            "user.name=o",
            "-c",
            "user.email=o@o",
            "commit",
            "-qm",
            "owner work",
        ],
    );
    let new = sh(&tmp, &["rev-parse", "HEAD"]);
    sh(&w.s.repo, &["update-ref", "refs/heads/main", &new]);
    sh(
        &w.s.repo,
        &["worktree", "remove", "--force", tmp.to_str().unwrap()],
    );
    let next = w.task("later", &[("x/later.txt", "1")]);
    wait_for("the next to land on top", 120, || w.landed(next));
    assert_eq!(sh(&w.s.repo, &["show", "main:owner.txt"]), "o");
    assert_eq!(sh(&w.s.repo, &["show", "main:x/later.txt"]), "1");
}
