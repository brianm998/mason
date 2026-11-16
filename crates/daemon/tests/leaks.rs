// SPDX-License-Identifier: GPL-3.0-or-later

//! A test's scratch world leaves no step host, agent or daemon behind (#466).

mod support;
use serde_json::json;
use std::time::Duration;
use support::*;

fn command_lines_with(needle: &str) -> Vec<String> {
    let out = std::process::Command::new("/bin/ps")
        .args(["-axo", "pid=,command="])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| l.contains(needle))
        .map(String::from)
        .collect()
}

#[test]
fn dropping_a_scratch_world_stops_its_hosts_agents_and_daemon() {
    let script = json!({"steps": {"explore": [[{"sh": "sleep 600"}]]}});
    let s = Scratch::new("true", script);
    let root = s.root.path().to_path_buf();
    let needle = root.display().to_string();
    let mut m = s.masond(&[], &[]);
    s.wait_daemon(&mut m);
    s.mason_ok(&["task", "add", "Slow explore"]);
    wait_for("a running step host", 60, || {
        !command_lines_with(&format!("--dir {needle}/d/h")).is_empty()
    });
    assert!(!command_lines_with(&needle).is_empty());

    drop(m);
    drop(s);

    // Hosts go by signal: give them time, with no assertion on how long.
    wait_for("every process of the scratch world to be gone", 60, || {
        std::thread::sleep(Duration::from_millis(100));
        command_lines_with(&needle).is_empty()
    });
}
