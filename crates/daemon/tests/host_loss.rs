// SPDX-License-Identifier: GPL-3.0-or-later

//! A step whose host is lost (s66): the step is run once more on a fresh host and
//! the task goes on; two losses in a row fail the step and name both; the cause is in
//! `mason task show`. The tests kill only hosts they watched start, after checking
//! that the registry's start string and command line still match.

mod support;
use mason_store::HostLossRow;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use support::*;

/// SIGKILL the host in `dir` (not its child), if it still is the process the registry
/// recorded. Returns whether it was killed.
fn kill_host_only(dir: &Path) -> bool {
    use mason_step_host::registry::{proc_command, proc_start, read_kv};
    let Ok(kv) = read_kv(&dir.join("host.kv")) else {
        return false;
    };
    let Some(pid) = kv.get("host_pid").and_then(|p| p.parse::<u32>().ok()) else {
        return false;
    };
    let ours = pid > 1
        && kv.get("host_start").is_some_and(|s| !s.is_empty())
        && proc_start(pid) == kv.get("host_start").cloned()
        && proc_command(pid).is_some_and(|c| c.contains(&dir.display().to_string()));
    ours && nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(pid as i32),
        nix::sys::signal::Signal::SIGKILL,
    )
    .is_ok()
}

/// Kills the host of each `-test` run (and its `-host1` re-run) up to `limit` times,
/// once its command has printed `started`.
fn watcher(hosts: PathBuf, limit: usize, stop: Arc<AtomicBool>, kills: Arc<AtomicUsize>) {
    std::thread::spawn(move || {
        let mut done: Vec<String> = vec![];
        while !stop.load(Ordering::SeqCst) && kills.load(Ordering::SeqCst) < limit {
            for e in std::fs::read_dir(&hosts).into_iter().flatten().flatten() {
                let id = e.file_name().to_string_lossy().to_string();
                if !(id.ends_with("-test") || id.ends_with("-test-host1")) || done.contains(&id) {
                    continue;
                }
                let journal = std::fs::read_to_string(e.path().join("journal")).unwrap_or_default();
                if journal.contains("started") && kill_host_only(&e.path()) {
                    done.push(id);
                    kills.fetch_add(1, Ordering::SeqCst);
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    });
}

fn world(always_hang: bool) -> (Scratch, PathBuf) {
    let s = Scratch::new("true", happy_script());
    let r = s.repo.parent().unwrap().to_path_buf();
    let marker = r.join("marker");
    let script = if always_hang {
        "echo started; sleep 60\n".to_string()
    } else {
        format!(
            "if [ -f {m} ]; then echo second run; exit 0; fi\ntouch {m}; echo started; sleep 60\n",
            m = marker.display()
        )
    };
    std::fs::write(r.join("t.sh"), script).unwrap();
    s.write_project("scratch", &format!("sh {}", r.join("t.sh").display()), "");
    (s, r)
}

#[test]
fn a_killed_host_is_run_again_once_and_the_task_goes_on() {
    let (s, _r) = world(false);
    let mut m = s.masond(&[], &[]);
    s.wait_daemon(&mut m);
    let (stop, kills) = (
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicUsize::new(0)),
    );
    watcher(s.data_dir.join("h"), 1, stop.clone(), kills.clone());
    s.mason_ok(&[
        "task",
        "add",
        "Add a hello file",
        "--body",
        "Create hello.txt.",
    ]);
    s.wait_task(0, "landed", 120);
    stop.store(true, Ordering::SeqCst);
    assert_eq!(kills.load(Ordering::SeqCst), 1);

    let hosts: Vec<String> = std::fs::read_dir(s.data_dir.join("h"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert!(
        hosts.iter().any(|h| h.ends_with("-test-host1")),
        "{hosts:?}"
    );

    let j = s.task_json(0);
    assert_eq!(
        j["failures"], 0,
        "a lost host is not a failed attempt: {j:#}"
    );
    let tests = j["steps"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|st| st["step"] == "testing")
        .count();
    assert_eq!(tests, 1, "one test step run, on two hosts: {j:#}");
    let losses = j["host_losses"].as_array().unwrap();
    assert_eq!(losses.len(), 1, "{j:#}");
    assert_eq!(losses[0]["rerun"], true);
    let detail = losses[0]["detail"].as_str().unwrap();
    assert!(
        detail.contains("host pid") && detail.contains("killed by signal 9, no exit record"),
        "{detail}"
    );
    let show = s.mason_ok(&["task", "show", "0"]);
    assert!(
        show.contains("host lost `test` (re-run once on a fresh host)")
            && show.contains("killed by signal 9, no exit record"),
        "{show}"
    );
    let logs = s.mason_ok(&["logs", "0"]);
    assert!(
        logs.contains("second run"),
        "the re-run's output is in the logs: {logs}"
    );
    drop(m);
}

#[test]
fn two_losses_in_a_row_fail_the_step_and_name_both() {
    let (s, _r) = world(true);
    let mut m = s.masond(&[], &[]);
    s.wait_daemon(&mut m);
    let (stop, kills) = (
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicUsize::new(0)),
    );
    watcher(s.data_dir.join("h"), 2, stop.clone(), kills.clone());
    s.mason_ok(&[
        "task",
        "add",
        "Add a hello file",
        "--body",
        "Create hello.txt.",
    ]);
    s.wait_task(0, "failed", 120);
    stop.store(true, Ordering::SeqCst);
    let j = s.task_json(0);
    let failure = j["failure"].as_str().unwrap();
    assert!(failure.contains("lost twice in a row"), "{failure}");
    assert_eq!(failure.matches("host pid").count(), 2, "{failure}");
    let losses = j["host_losses"].as_array().unwrap();
    assert_eq!(losses.len(), 2, "{j:#}");
    assert_eq!(
        (losses[0]["rerun"].clone(), losses[1]["rerun"].clone()),
        (Value::Bool(true), Value::Bool(false))
    );
    let show = s.mason_ok(&["task", "show", "0"]);
    assert_eq!(show.matches("host lost `test`").count(), 2, "{show}");
    // The second loss is a repeat: it is in the alerts.
    let alerts = s.mason_ok(&["alerts"]);
    assert!(
        alerts.contains("lost again") || alerts.contains("lost at the same time"),
        "{alerts}"
    );
    drop(m);
}

fn loss(id: &str, task: u64, at: u64) -> HostLossRow {
    HostLossRow {
        host_id: id.into(),
        task,
        run: task,
        step: "test".into(),
        lost_at_ms: at,
        detail: format!("host pid {task} gone"),
        retried: true,
    }
}

#[test]
fn hosts_of_different_tasks_lost_together_raise_one_alert() {
    use mason_daemon::attention::host_loss_alerts;
    let a = host_loss_alerts(&[loss("a", 12, 1000), loss("b", 15, 1200)]);
    assert_eq!(a.len(), 1, "{a:?}");
    assert!(
        a[0].2.starts_with(
            "2 step hosts were lost at the same time (tasks #12, #15): something killed them"
        ),
        "{}",
        a[0].2
    );
    assert!(a[0].2.contains("host pid 12 gone") && a[0].2.contains("host pid 15 gone"));
    // Far apart, or the same task: no cluster alert, and a single answered loss is quiet.
    assert!(host_loss_alerts(&[loss("a", 12, 1000), loss("b", 15, 20_000)]).is_empty());
    assert!(
        host_loss_alerts(&[loss("a", 12, 1000), loss("b", 12, 1200)])
            .iter()
            .all(|x| x.0 == "host_lost:b")
    );
    // Three tasks in a chain of short gaps are one event.
    let c = host_loss_alerts(&[loss("a", 1, 0), loss("b", 2, 4000), loss("c", 3, 8000)]);
    assert_eq!(c.len(), 1);
    assert!(c[0].2.starts_with("3 step hosts were lost"), "{}", c[0].2);
}
