// SPDX-License-Identifier: GPL-3.0-or-later

//! The pipeline-wide stall alert (s115): with `[stall] after` very short and a task that cannot
//! start (the disk is below the floor), nothing moves, so `stalled` is raised naming the disk. Any
//! event clears it.

mod support;
use mason_core::*;
use mason_daemon::attention::alerts_now;
use mason_daemon::disk::FakeDisk;
use std::time::Duration;
use support::*;

fn stalled(d: &std::sync::Arc<mason_daemon::daemon::Daemon>) -> Option<String> {
    alerts_now(d)
        .into_iter()
        .find(|(k, kind, _)| k == "stalled" && kind == "stalled")
        .map(|(_, _, t)| t)
}

#[test]
fn a_quiet_pipeline_raises_stalled_naming_the_disk_and_the_next_event_clears_it() {
    let s = Scratch::new("true", happy_script());
    std::fs::write(
        s.config_dir.join("notify.toml"),
        "[stall]\nafter = \"1s\"\n",
    )
    .unwrap();
    let disk = FakeDisk::new_gb(12);
    let d = s.start_with(|c| {
        c.disk = Some(disk.clone());
        c.disk_every = Duration::from_millis(50);
    });
    let t = file_task(&d, "Waits for room");
    wait_for("the stall alert", 30, || stalled(&d).is_some());
    let text = stalled(&d).unwrap();
    assert!(text.contains("1 task"), "{text}");
    assert!(text.contains("12 GB free, floor 20 GB"), "{text}");

    disk.set_gb(40);
    wait_state(&d, t, State::Landed, 60);
    wait_for("the alert to clear", 15, || stalled(&d).is_none());
}

#[test]
fn off_by_default_threshold_is_thirty_minutes_and_zero_disables() {
    let s = Scratch::new("true", happy_script());
    std::fs::write(s.config_dir.join("notify.toml"), "[stall]\nafter = 0\n").unwrap();
    let d = s.start_with(|c| {
        c.disk = Some(FakeDisk::new_gb(12));
        c.disk_every = Duration::from_millis(50);
    });
    file_task(&d, "Waits");
    std::thread::sleep(Duration::from_secs(2));
    assert!(stalled(&d).is_none());
}
