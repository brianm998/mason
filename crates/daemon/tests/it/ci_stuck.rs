// SPDX-License-Identifier: GPL-3.0-or-later

//! s1043 (decision 1036): a batch whose CI stays pending with the same jobs raises a `landing`
//! alert that names the batch and the running job, and clears it when the run turns green or
//! red. The scripted CI is held by a file; nothing here asserts how long anything took.

use crate::support;
use std::path::PathBuf;
use support::lw::*;
use support::*;

fn world(release: &PathBuf) -> W {
    let toml = "mode = \"direct-ci\"\nwindow = \"30s\"\nmax_size = 2\nmax_fold = 4\n[[tier]]\nname = \"batch\"\nsuites = [\"alpha\"]\n";
    let release = release.clone();
    world_with(
        toml,
        &move |_| {
            let status = format!(
                "if [ -f '{r}' ]; then cat '{r}'; else echo pending; echo 'running: trd (windows-2022)'; echo 'queued: 2'; fi",
                r = release.display()
            );
            format!(
                "[ci]\nkind = \"command\"\ntrigger = \"echo $MASON_SHA\"\nstatus = \"{}\"\npoll_s = 1\ntimeout_s = 120\nstuck_after = \"1s\"\n",
                q(&status)
            )
        },
        &[],
    )
}

fn stuck_alert(w: &W) -> Option<String> {
    mason_daemon::attention::alerts_now(&w.d)
        .into_iter()
        .find(|(k, kind, _)| k.contains("ci-stuck") && kind == "landing")
        .map(|(_, _, t)| t)
}

fn run(verdict: &str) {
    let dir = tempfile::tempdir().unwrap();
    let release = dir.path().join("release");
    let w = world(&release);
    let _t = w.task("slow CI", &[("x/ok.txt", "1")]);
    let mut text = String::new();
    wait_for("the stuck alert", 90, || {
        stuck_alert(&w).is_some_and(|t| {
            text = t;
            true
        })
    });
    assert!(text.contains("trd (windows-2022)"), "{text}");
    assert!(text.contains("2 queued"), "{text}");
    assert!(text.contains("mason landing"), "{text}");
    std::fs::write(&release, format!("{verdict}\n")).unwrap();
    wait_for("the alert to clear", 90, || stuck_alert(&w).is_none());
}

#[test]
fn a_pending_run_that_does_not_change_raises_an_alert_cleared_by_green() {
    run("green");
}

#[test]
fn the_alert_is_cleared_when_the_run_turns_red() {
    run("red");
}
