// SPDX-License-Identifier: GPL-3.0-or-later

//! s168: a failure that is not about the code is retried at the step that failed.
//! The real `masond` and `mason`, a local bare remote whose pre-receive hook rejects
//! pushes, so the task fails at landing. A retry with no step goes back to landing
//! and no develop step runs again; `--from` still overrides.

mod support;
use serde_json::Value;
use std::os::unix::fs::PermissionsExt;
use support::*;

fn hook(s: &Scratch, on: bool) {
    let p = s.origin.join("hooks/pre-receive");
    if on {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, "#!/bin/sh\necho 'rejected by the hook' >&2\nexit 1\n").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    } else {
        let _ = std::fs::remove_file(&p);
    }
}

fn develop_runs(t: &Value) -> usize {
    t["steps"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|x| x["step"] == "developing")
        .count()
}

#[test]
fn a_landing_push_failure_is_retried_at_land_and_no_develop_runs_again() {
    let s = Scratch::new("test -f hello.txt", happy_script());
    hook(&s, true);
    let mut m = s.masond(&[], &[]);
    s.wait_daemon(&mut m);
    s.mason_ok(&["task", "add", "Push is rejected"]);
    s.wait_task(0, "failed", 120);
    let t = s.task_json(0);
    assert!(
        t["failure"].as_str().unwrap().contains("git push"),
        "failure was {} (step {})",
        t["failure"],
        t["step"]
    );
    assert_eq!(
        t["failure_class"], "outside",
        "failure was {}",
        t["failure"]
    );
    let at = t["step"].as_str().unwrap().to_string();
    assert_eq!(t["retry_to"], at.as_str(), "retry_to for {}", t["failure"]);
    assert_eq!(develop_runs(&t), 1);

    hook(&s, false);
    let out = s.mason_ok(&["task", "retry", "0"]);
    assert!(out.contains(&format!("`{at}`")), "{out}");
    s.wait_task(0, "landed", 120);
    let t = s.task_json(0);
    assert_eq!(develop_runs(&t), 1, "{t:#}");
    drop(m);
    kill_hosts(&s.data_dir);
}

#[test]
fn from_still_overrides_the_failed_step() {
    let s = Scratch::new("test -f hello.txt", happy_script());
    hook(&s, true);
    let mut m = s.masond(&[], &[]);
    s.wait_daemon(&mut m);
    s.mason_ok(&["task", "add", "Push is rejected"]);
    s.wait_task(0, "failed", 120);
    let out = s.mason_ok(&["task", "retry", "0", "--from", "developing"]);
    assert!(out.contains("from developing"), "{out}");
    drop(m);
    kill_hosts(&s.data_dir);
}
