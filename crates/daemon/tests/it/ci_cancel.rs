// SPDX-License-Identifier: GPL-3.0-or-later

//! s1088 (decision 1077): when the landing stops waiting on a CI run without a verdict (a
//! timeout, Land now, a batch a dead daemon left, a debt run), it cancels the run before it
//! deletes the run's branch. The fake CI writes down every cancel. Nothing here asserts how
//! long anything took.

use crate::landing::post_api;
use crate::support;
use mason_store::BatchRow;
use std::path::{Path, PathBuf};
use support::lw::*;
use support::*;

struct World {
    w: W,
    dir: tempfile::TempDir,
}

/// A `direct-ci` (or PR) world whose command CI answers `status`, gives up after
/// `timeout_s`, and whose cancel writes `<ref> <run>` down, then runs `after_cancel`.
fn world(mode: &str, status: &str, timeout_s: u64, after_cancel: &str) -> World {
    let dir = tempfile::tempdir().unwrap();
    let cancels = dir.path().join("cancels.log");
    let triggers = dir.path().join("triggers.log");
    let toml = format!("mode = \"{mode}\"\nwindow = \"3s\"\nmax_size = 2\nmax_fold = 4\n[[tier]]\nname = \"batch\"\nsuites = [\"alpha\"]\n");
    let w = world_with(
        &toml,
        &|s| {
            let host = if mode.starts_with("pr-") {
                host_section(s, false, false)
            } else {
                String::new()
            };
            let trigger = format!(
                "echo $MASON_REF >> '{}'; echo $MASON_SHA",
                triggers.display()
            );
            let cancel = format!(
                "echo \"$MASON_REF $MASON_RUN\" >> '{}'; {after_cancel}",
                cancels.display()
            );
            format!(
                "{host}[ci]\nkind = \"command\"\ntrigger = \"{}\"\nstatus = \"{}\"\ncancel = \"{}\"\npoll_s = 1\ntimeout_s = {timeout_s}\n",
                q(&trigger),
                q(status),
                q(&cancel)
            )
        },
        &[],
    );
    World { w, dir }
}

fn lines(p: &Path) -> Vec<String> {
    std::fs::read_to_string(p)
        .unwrap_or_default()
        .lines()
        .map(String::from)
        .collect()
}

impl World {
    fn cancels_path(&self) -> PathBuf {
        self.dir.path().join("cancels.log")
    }

    /// The cancels asked for on refs that start with `prefix`.
    fn cancels(&self, prefix: &str) -> Vec<String> {
        lines(&self.cancels_path())
            .into_iter()
            .filter(|l| l.starts_with(prefix))
            .collect()
    }

    fn batch(&self, id: u64) -> Option<BatchRow> {
        self.w
            .d
            .store
            .batches(Some("scratch"), 50)
            .unwrap()
            .into_iter()
            .find(|b| b.id == id)
    }

    fn finished(&self, id: u64) -> BatchRow {
        wait_for("the batch to finish", 120, || {
            self.batch(id).is_some_and(|b| b.finished_at.is_some())
        });
        self.batch(id).unwrap()
    }

    fn no_open_ref_of(&self, batch: u64) {
        wait_for("the batch's refs to be removed", 60, || {
            !self
                .w
                .d
                .store
                .open_refs("scratch")
                .unwrap()
                .iter()
                .any(|r| r.batch == batch)
        });
    }
}

#[test]
fn a_direct_ci_batch_whose_ci_never_finishes_cancels_its_run_once() {
    let x = world("direct-ci", "echo pending", 1, "true");
    let _t = x.w.task("stuck CI", &[("x/a.txt", "1")]);
    let b = x.finished(1);
    x.no_open_ref_of(1);
    let c = x.cancels("mason/ci/scratch-b1-");
    assert_eq!(c.len(), 1, "{c:?}");
    // The run the trigger named: the commit.
    let run = c[0].split_whitespace().nth(1).unwrap_or_default();
    assert_eq!(run.len(), 40, "{c:?}");
    assert!(
        b.note.contains("still not finished after 1 s"),
        "{}",
        b.note
    );
    assert!(b.note.contains("1 run cancelled"), "{}", b.note);
}

#[test]
fn land_now_on_a_batch_waiting_for_ci_cancels_its_run() {
    let x = world("direct-ci", "echo pending", 600, "true");
    let t = x.w.task("slow CI", &[("x/a.txt", "1")]);
    wait_for("CI to be triggered", 120, || {
        !lines(&x.dir.path().join("triggers.log")).is_empty()
    });
    assert!(x.cancels("mason/").is_empty());
    let (code, body) = post_api(&x.w.d, "/batches/1/land-now".to_string());
    assert_eq!(code, 200, "{body}");
    wait_for("the task to land", 120, || x.w.landed(t));
    let b = x.finished(1);
    x.no_open_ref_of(1);
    let c = x.cancels("mason/ci/scratch-b1-");
    assert_eq!(c.len(), 1, "{c:?}");
    assert!(
        b.note
            .contains("not waited for (land now); 1 run cancelled"),
        "{}",
        b.note
    );
}

#[test]
fn a_batch_a_dead_daemon_left_has_its_run_cancelled_before_its_ref_goes() {
    let x = world("direct-ci", "echo green", 120, "true");
    // What a daemon that died during the CI wait leaves: an unfinished batch and its ref.
    // The project's coordinator starts with the first task that lands, and finds them.
    let old = x.w.d.store.add_batch("scratch", "999", 1).unwrap();
    let name = format!("mason/ci/scratch-b{old}-0");
    sh(&x.w.s.origin, &["branch", &name, "main"]);
    x.w.d
        .store
        .add_ref(old, "scratch", "ci", &name, None, 1)
        .unwrap();
    let t = x.w.task("after the restart", &[("x/a.txt", "1")]);
    wait_for("the task to land", 120, || x.w.landed(t));
    x.no_open_ref_of(old);
    let c = x.cancels("mason/");
    assert_eq!(c, vec![format!("{name} ")], "by the ref, with no run id");
    let b = x.batch(old).unwrap();
    assert_eq!(b.phase, "abandoned");
    assert!(
        b.note
            .contains("not waited for (the daemon restarted); 1 run cancelled"),
        "{}",
        b.note
    );
    let left = sh(&x.w.s.origin, &["branch", "--list", &name]);
    assert!(left.trim().is_empty(), "{left}");
}

#[test]
fn a_green_run_cancels_nothing() {
    let x = world("direct-ci", "echo green", 120, "true");
    let t = x.w.task("fine", &[("x/a.txt", "1")]);
    wait_for("the task to land", 120, || x.w.landed(t));
    x.finished(1);
    x.no_open_ref_of(1);
    assert!(x.cancels("mason/").is_empty(), "{:?}", x.cancels("mason/"));
}

#[test]
fn a_cancel_that_fails_leaves_the_batchs_outcome_unchanged() {
    let x = world("direct-ci", "echo pending", 1, "exit 1");
    let t = x.w.task("stuck CI", &[("x/a.txt", "1")]);
    let b = x.finished(1);
    x.no_open_ref_of(1);
    assert_eq!(x.cancels("mason/ci/scratch-b1-").len(), 1);
    assert_eq!(b.phase, "failed", "{}", b.note);
    assert!(
        b.note.contains("still not finished after 1 s"),
        "{}",
        b.note
    );
    assert!(!b.note.contains("cancelled"), "{}", b.note);
    assert!(!x.w.landed(t));
}

#[test]
fn a_merged_pull_request_cancels_nothing() {
    let x = world("pr-batch", "echo green", 120, "true");
    let t = x.w.task("fine", &[("x/a.txt", "1")]);
    wait_for("the task to land", 120, || x.w.landed(t));
    x.finished(1);
    x.no_open_ref_of(1);
    assert!(x.cancels("mason/").is_empty(), "{:?}", x.cancels("mason/"));
}

#[test]
fn a_red_pull_request_has_its_other_checks_cancelled() {
    let x = world("pr-batch", "echo green", 120, "true");
    let _t = x.w.task("breaks checks", &[("red.integration", "x")]);
    wait_for("the pull request's runs to be cancelled", 120, || {
        !x.cancels("mason/pr/scratch-b1-").is_empty()
    });
}

#[test]
fn a_debt_ci_run_that_times_out_is_cancelled() {
    let x = world("direct-ci", "echo pending", 0, "true");
    let debt =
        x.w.d
            .store
            .add_debt(0, "scratch", "ci:ci", "land now", "owner", true, 1)
            .unwrap();
    let said = mason_daemon::suites::pay(&x.w.d, debt).unwrap_err();
    assert!(said.contains("still not finished after 0 s"), "{said}");
    let name = format!("mason/ci/scratch-debt{debt}");
    assert!(!x.cancels(&name).is_empty(), "{:?}", x.cancels("mason/"));
}
