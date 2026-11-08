// SPDX-License-Identifier: GPL-3.0-or-later

//! Stage 25 end to end, on the fake provider: devices leased by capability, a
//! missing device as a wait that blames nobody, plugging one in, an expired
//! certificate as the machine's fault, and the lifecycle order.

mod support;
use mason_core::*;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use support::*;

const TAP: &str = include_str!("../../../scripts/exit/fixtures/s25/suite-tap.sh");
const CERT: &str = include_str!("../../../scripts/exit/fixtures/s25/suite-cert.sh");

struct Env {
    s: Scratch,
    fixture: PathBuf,
    log: PathBuf,
    gate: PathBuf,
    flag: PathBuf,
}

fn device(id: &str, kind: &str) -> Value {
    json!({"id": id, "kind": kind, "os": "14", "name": id})
}

fn plug(fixture: &Path, devices: &[Value]) {
    // Write then rename, so a reader never sees half a file.
    let tmp = fixture.with_extension("tmp");
    std::fs::write(&tmp, json!({"devices": devices}).to_string()).unwrap();
    std::fs::rename(&tmp, fixture).unwrap();
}

fn definition(rounds: u32) -> String {
    format!(
        r#"schema = 1
start = "develop"

[step.develop]
type = "agent"
category = "developing"
next = "test"

[step.test]
type = "test"
category = "testing"
suites = ["emu", "sim", "phone"]
on_red = "develop"
max_rounds = {rounds}
next = "land"

[step.land]
type = "landing"
category = "landing"

[suite.emu]
command = "sh tap.sh LOG GATE FLAG"
parser = "tap"
min_tests = 3
level = "unit"
env = "emulator"

[suite.sim]
command = "sh tap.sh LOG GATE FLAG"
parser = "tap"
min_tests = 3
level = "unit"
env = "simulator"

[suite.phone]
command = "sh cert.sh LOG FLAG"
parser = "tap"
min_tests = 3
level = "unit"
env = "iphone"

[environment.emulator]
type = "android-emulator"
[environment.simulator]
type = "ios-simulator"
[environment.iphone]
type = "ios-device"

[impact]
"android/**" = ["emu"]
"ios/**" = ["sim"]
"phone/**" = ["phone"]
"#
    )
}

/// `area_for` is a shell test on the task id (the worktree's directory name)
/// that picks which directory the scripted agent changes, and so which suite runs.
fn setup(devices: &[Value], rounds: u32, area_cmd: &str) -> Env {
    let commit = format!(
        "{area_cmd}; mkdir -p $A && echo x > $A/f-$(basename \"$PWD\").txt && git add -A && git -c user.name=t -c user.email=t@t commit -qm change"
    );
    let s = Scratch::new(
        "false",
        json!({"steps": {"develop": [[
            {"sh": commit},
            {"call": {"tool": "submit_change", "args": {"summary": "change", "head": "$HEAD"}}}
        ]]}}),
    );
    let root = s.root.path().canonicalize().unwrap();
    let (fixture, log, gate, flag) = (
        root.join("devices.json"),
        root.join("calls.log"),
        root.join("gate"),
        root.join("flag"),
    );
    std::fs::create_dir_all(&gate).unwrap();
    plug(&fixture, devices);
    let def = definition(rounds)
        .replace("LOG", &log.display().to_string())
        .replace("GATE", &gate.display().to_string())
        .replace("FLAG", &flag.display().to_string());
    s.push_files(
        &[
            (".mason/mason.toml", &def),
            ("tap.sh", TAP),
            ("cert.sh", CERT),
        ],
        "setup",
    );
    std::fs::write(
        s.config_dir.join("daemon.toml"),
        format!(
            "[targets]\nprovider = \"fake\"\nfake_file = \"{}\"\nfake_log = \"{}\"\npoll_ms = 200\n",
            fixture.display(),
            log.display()
        ),
    )
    .unwrap();
    Env {
        s,
        fixture,
        log,
        gate,
        flag,
    }
}

fn start(e: &Env) -> Arc<mason_daemon::Daemon> {
    e.s.start_with(|c| {
        c.env_retry = Duration::from_millis(200);
        c.max_concurrent = 4;
    })
}

fn file_it(d: &Arc<mason_daemon::Daemon>, title: &str) -> TaskId {
    file_task_as(d, title, "Please do it.", true, TrustSource::Owner, false)
}

fn log_lines(e: &Env) -> Vec<String> {
    std::fs::read_to_string(&e.log)
        .unwrap_or_default()
        .lines()
        .map(String::from)
        .collect()
}

fn waiting_on(d: &Arc<mason_daemon::Daemon>, t: TaskId) -> Option<String> {
    match task_of(d, t).activity {
        Activity::Waiting(WaitReason::On(w)) => Some(w),
        _ => None,
    }
}

#[test]
fn devices_are_leased_by_capability_and_never_shared() {
    // Tasks 0-2 change android/, task 3 changes ios/.
    let e = setup(
        &[
            device("emu-a1", "android-emulator"),
            device("emu-a2", "android-emulator"),
            device("sim-i1", "ios-simulator"),
        ],
        1,
        "if [ $(basename \"$PWD\") -ge 3 ]; then A=ios; else A=android; fi",
    );
    std::fs::write(e.gate.join("hold"), "x").unwrap();
    let d = start(&e);
    let a = file_it(&d, "android one");
    let b = file_it(&d, "android two");
    wait_for("two android suites to hold a device each", 60, || {
        let l = log_lines(&e);
        l.contains(&"suite emu-a1".to_string()) && l.contains(&"suite emu-a2".to_string())
    });
    let c = file_it(&d, "android three");
    wait_for("the third to wait with a place in line", 60, || {
        waiting_on(&d, c).is_some_and(|w| w.contains("in line"))
    });
    let third = waiting_on(&d, c).unwrap();
    assert!(third.contains("device:android-emulator"), "{third}");
    // The iOS task is not blocked by the android queue.
    let ios = file_it(&d, "ios one");
    wait_for(
        "the ios suite to start beside the waiting android one",
        60,
        || log_lines(&e).contains(&"suite sim-i1".to_string()),
    );
    assert!(waiting_on(&d, c).is_some(), "the third is still waiting");
    // The view shows who holds what.
    let v = d.targets.view(&d);
    let held: Vec<&Value> = v["devices"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|x| !x["leased_by"].is_null())
        .collect();
    assert_eq!(held.len(), 3, "{v}");
    assert_eq!(v["pools"][1]["name"], "device:android-emulator");
    assert_eq!(v["pools"][1]["used"], 2);
    // Never two tasks on one device at once.
    let starts = log_lines(&e)
        .into_iter()
        .filter(|l| l.starts_with("suite "))
        .collect::<Vec<_>>();
    assert_eq!(starts.len(), 3, "{starts:?}");
    let mut ids = starts.clone();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 3, "{starts:?}");
    std::fs::remove_file(e.gate.join("hold")).unwrap();
    for t in [a, b, c, ios] {
        wait_state(&d, t, State::Landed, 90);
    }
    let all: Vec<String> = log_lines(&e)
        .into_iter()
        .filter(|l| l.starts_with("suite "))
        .collect();
    assert_eq!(all.len(), 4);
    assert!(all.iter().filter(|l| l.contains("emu-a")).count() == 3);
    assert!(all.iter().filter(|l| l.contains("sim-i")).count() == 1);
}

#[test]
fn a_missing_device_is_a_wait_and_an_alert_and_plugging_one_in_resumes_the_task() {
    let e = setup(&[], 1, "A=phone");
    let d = start(&e);
    let t = file_it(&d, "needs a phone");
    wait_for("the task to wait for a phone", 60, || {
        waiting_on(&d, t).is_some_and(|w| w.contains("environment problem"))
    });
    let w = waiting_on(&d, t).unwrap();
    assert!(
        w.contains("not the task's fault")
            && w.contains("device:ios-device")
            && w.contains("Plug one in"),
        "{w}"
    );
    let task = task_of(&d, t);
    assert_eq!(task.failures, 0, "blamed on nobody");
    assert_ne!(task.state, State::Failed);
    let alerts = mason_daemon::attention::alerts_now(&d);
    assert!(
        alerts
            .iter()
            .any(|a| a.1 == "environment" && a.2.contains("device:ios-device")),
        "{alerts:?}"
    );
    // Plug one in: the task goes on within a read or two.
    plug(&e.fixture, &[device("phone-1", "ios-device")]);
    wait_state(&d, t, State::Landed, 60);
    assert!(log_lines(&e).contains(&"suite phone-1".to_string()));
    assert_eq!(task_of(&d, t).failures, 0);
}

#[test]
fn an_expired_certificate_is_the_machines_fault_and_the_task_goes_on_when_it_is_fixed() {
    let e = setup(&[device("phone-1", "ios-device")], 1, "A=phone");
    std::fs::write(&e.flag, "expired").unwrap();
    let d = start(&e);
    let t = file_it(&d, "signed build");
    wait_for("an environmental wait naming the certificate", 60, || {
        waiting_on(&d, t).is_some_and(|w| w.contains("expired-certificate"))
    });
    let w = waiting_on(&d, t).unwrap();
    assert!(
        w.contains("environment problem, not the task's fault"),
        "{w}"
    );
    assert_eq!(task_of(&d, t).failures, 0);
    assert!(mason_daemon::attention::alerts_now(&d)
        .iter()
        .any(|a| a.1 == "environment"));
    std::fs::remove_file(&e.flag).unwrap();
    wait_state(&d, t, State::Landed, 60);
    assert_eq!(task_of(&d, t).failures, 0);
}

#[test]
fn the_lifecycle_is_setup_health_suite_teardown_and_teardown_follows_a_failure() {
    let e = setup(
        &[device("emu-a1", "android-emulator")],
        1,
        "A=android; mkdir -p android; touch android/red.txt",
    );
    std::fs::write(&e.flag, "fail").unwrap();
    let d = start(&e);
    let t = file_it(&d, "red suite");
    wait_state(&d, t, State::Failed, 90);
    let l = log_lines(&e);
    // The rerun and the baseline check run on the same device, and the last word is teardown.
    assert_eq!(
        &l[..3],
        ["setup emu-a1", "health emu-a1", "suite emu-a1"].map(String::from),
        "{l:?}"
    );
    assert_eq!(l.last().unwrap(), "teardown emu-a1", "{l:?}");
    let count = |p: &str| l.iter().filter(|x| x.starts_with(p)).count();
    assert_eq!(count("setup"), count("teardown"), "{l:?}");
    // And nothing is held after.
    assert!(d.targets.view(&d)["devices"][0]["leased_by"].is_null());
    assert_eq!(d.broker.used("device:android-emulator"), 0);
}

#[test]
fn a_skipped_suite_still_tears_its_device_down() {
    let e = setup(&[device("emu-a1", "android-emulator")], 1, "A=android");
    std::fs::write(e.gate.join("hold"), "x").unwrap();
    let d = start(&e);
    let t = file_it(&d, "skip me");
    wait_for("the suite to start on the device", 60, || {
        log_lines(&e).contains(&"suite emu-a1".to_string())
    });
    mason_daemon::suites::skip(&d, t, "suite:emu", "not worth it", "brian", false).unwrap();
    wait_for("teardown after the skip", 30, || {
        log_lines(&e).contains(&"teardown emu-a1".to_string())
    });
    std::fs::remove_file(e.gate.join("hold")).unwrap();
    wait_state(&d, t, State::Landed, 60);
    let l = log_lines(&e);
    assert_eq!(
        l.iter().filter(|x| x.starts_with("setup")).count(),
        l.iter().filter(|x| x.starts_with("teardown")).count(),
        "{l:?}"
    );
}
