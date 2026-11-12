// SPDX-License-Identifier: GPL-3.0-or-later

//! s184: a self-update waits for running landing batches instead of abandoning them. The
//! drain is exercised in the test process (a real `exec` would replace the test binary):
//! a batch is held in its verify phase by a suite that blocks on a file the test controls,
//! and every wait here is on store state, queue state or the update status, never on time.

mod support;
use mason_core::*;
use mason_daemon::update;
use std::path::Path;
use std::time::Duration;
use support::lw::*;
use support::*;

const LANDING: &str = "window = \"0s\"\n[[tier]]\nname = \"batch\"\nsuites = [\"integration\"]\n";

/// A world whose integration suite blocks until `release` exists (or the scratch root is gone).
fn blocked_world() -> (W, std::path::PathBuf) {
    let w = world(LANDING, &[]);
    let release = w.s.root.path().join("release");
    let script = format!(
        "#!/bin/sh\necho 1..1\nwhile [ -d '{r}' ] && [ ! -e '{f}' ]; do sleep 0.1; done\necho 'ok 1 - integration'\n",
        r = w.s.root.path().display(),
        f = release.display()
    );
    w.s.push_files(&[("checks.sh", &script)], "blocking suite");
    (w, release)
}

fn pid(w: &W) -> ProjectId {
    w.d.project_id_by_name("scratch").unwrap()
}

fn running(w: &W) {
    let p = pid(w);
    wait_for("a batch to run", 90, || {
        w.d.landing().running_batches(p) == 1
    });
}

fn release(f: &Path) {
    std::fs::write(f, "").unwrap();
}

fn phase(w: &W) -> String {
    w.d.update_status.lock().unwrap().phase.clone()
}

fn log(w: &W) -> String {
    w.d.update_status.lock().unwrap().log.join("\n")
}

#[test]
fn a_takeover_waits_for_running_landing_batches_and_holds_new_ones() {
    let (w, rel) = blocked_world();
    let t1 = w.task("one", &[("x/one.txt", "1")]);
    running(&w);
    let d = w.d.clone();
    let waiter = std::thread::spawn(move || update::drain(&d));
    wait_for("the update to say it waits", 60, || {
        phase(&w) == "waiting for landing"
    });
    assert!(
        log(&w).contains("waiting for 1 landing batch(es): scratch #"),
        "{}",
        log(&w)
    );
    // A task that arrives now stays queued: no batch starts for it.
    let t2 = w.task("two", &[("x/two.txt", "2")]);
    let p = pid(&w);
    wait_for("the second task to queue", 60, || {
        w.d.landing().queued(p).contains(&t2)
    });
    assert!(w.d.landing().draining());
    assert_eq!(w.d.landing().running_batches(p), 1);
    assert_eq!(w.d.store.batches(Some("scratch"), 10).unwrap().len(), 1);
    // The batch finishes; only then does the wait end.
    release(&rel);
    waiter.join().unwrap();
    assert!(w.d.landing().busy_projects().is_empty());
    wait_for("the first task to land", 90, || w.landed(t1));
    assert!(
        w.d.landing().queued(p).contains(&t2),
        "t2 must still be queued"
    );
    assert!(!w.landed(t2));
    // Landing resumes when the drain is let go; the queued task loses nothing.
    w.d.landing().set_draining(false);
    wait_for("the queued task to land", 90, || w.landed(t2));
}

#[test]
fn a_takeover_past_its_cap_names_the_batches_it_abandons() {
    let (w, rel) = blocked_world();
    let _t1 = w.task("one", &[("x/one.txt", "1")]);
    running(&w);
    update::drain_within(&w.d, Duration::ZERO);
    let id = w.d.store.batches(Some("scratch"), 10).unwrap()[0].id;
    let l = log(&w);
    assert!(
        l.contains("taking over anyway") && l.contains(&format!("scratch #{id}")),
        "{l}"
    );
    assert_eq!(phase(&w), "taking over");
    release(&rel);
    w.d.landing().set_draining(false);
}

#[test]
fn a_failed_takeover_lets_landing_resume() {
    let w = world(LANDING, &[]);
    w.d.landing().set_draining(true);
    let t = w.task("one", &[("x/one.txt", "1")]);
    let p = pid(&w);
    wait_for("the task to queue", 60, || {
        w.d.landing().queued(p).contains(&t)
    });
    assert!(w.d.store.batches(Some("scratch"), 10).unwrap().is_empty());
    // No runtime of that name exists, so the exec fails and take_over returns.
    let why = update::drain_and_take_over(&w.d, "0123456789abcdef", false);
    assert!(!why.is_empty());
    assert!(!w.d.landing().draining());
    wait_for("the queued task to land", 90, || w.landed(t));
}
