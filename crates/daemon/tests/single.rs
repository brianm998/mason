// SPDX-License-Identifier: GPL-3.0-or-later

//! One daemon per data directory: a kernel lock, so a stale `daemon.json`
//! never blocks a start and a live daemon always does.

mod support;
use support::*;

fn run(s: &Scratch) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_masond"))
        .args(["--data-dir", &s.data_dir.display().to_string()])
        .args(["--config-dir", &s.config_dir.display().to_string()])
        .args(["--port", "0"])
        .args(["--host-bin", "/nonexistent", "--mcp-bin", "/nonexistent"])
        .output()
        .unwrap()
}

#[test]
fn a_stale_daemon_json_naming_a_live_process_does_not_block() {
    let s = Scratch::new("true", happy_script());
    let mut other = std::process::Command::new("sleep")
        .arg("600")
        .spawn()
        .unwrap();
    std::fs::create_dir_all(&s.data_dir).unwrap();
    std::fs::write(
        s.data_dir.join("daemon.json"),
        format!(r#"{{"pid":{},"port":1,"started_ms":1}}"#, other.id()),
    )
    .unwrap();
    let mut m = s.masond(&[], &[]);
    s.wait_daemon(&mut m);
    assert!(m.child.try_wait().unwrap().is_none());
    let _ = other.kill();
    let _ = other.wait();
}

#[test]
fn a_second_daemon_is_refused_while_the_first_lives_and_starts_after() {
    let s = Scratch::new("true", happy_script());
    let mut m = s.masond(&[], &[]);
    s.wait_daemon(&mut m);
    let out = run(&s);
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains(&format!(
            "a daemon is already running here (pid {})",
            m.child.id()
        )),
        "{err}"
    );
    let _ = m.child.kill();
    let _ = m.child.wait();
    // A killed daemon leaves daemon.json behind; the lock went with it.
    assert!(s.data_dir.join("daemon.json").exists());
    let mut m2 = s.masond(&[], &[]);
    s.wait_daemon(&mut m2);
    assert!(m2.child.try_wait().unwrap().is_none());
}
