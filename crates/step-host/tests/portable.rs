// SPDX-License-Identifier: GPL-3.0-or-later
//! The step host through its transport on every platform: connect, reattach
//! after the controller is gone, and stop the child's tree. (`host.rs` has the
//! rest of the host's behaviour and is Unix-only: it scripts `sh`.)

use mason_step_host::client::{Event, HostClient};
use mason_step_host::launcher::{spawn_host, SpawnOptions};
use mason_step_host::protocol;
use mason_step_host::registry::{pid_alive, HostInfo};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const HOST: &str = env!("CARGO_BIN_EXE_step-host");

/// A command that prints `first` and then stays alive for a minute.
fn print_then_wait() -> Vec<&'static str> {
    if cfg!(windows) {
        vec!["cmd", "/C", "echo first & ping -n 60 127.0.0.1 >NUL"]
    } else {
        vec!["sh", "-c", "echo first; sleep 60"]
    }
}

fn echo_and_exit() -> Vec<&'static str> {
    if cfg!(windows) {
        vec!["cmd", "/C", "echo out"]
    } else {
        vec!["sh", "-c", "echo out"]
    }
}

fn start(root: &Path, id: &str, cmd: &[&str]) -> HostInfo {
    let mut o = SpawnOptions::new(HOST, root, id, cmd);
    o.heartbeat = Duration::from_millis(50);
    o.linger = Duration::from_secs(30);
    spawn_host(&o).unwrap()
}

fn root() -> (tempfile::TempDir, PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let r = d.path().join("h");
    (d, r)
}

fn connect(sock: &Path) -> HostClient {
    HostClient::connect(sock, protocol::PREVIOUS, protocol::CURRENT, "test").unwrap()
}

/// Read from `offset` until the bytes contain `needle`.
fn read_until(c: &mut HostClient, offset: u64, needle: &str) -> String {
    c.attach(offset).unwrap();
    let mut got = vec![];
    loop {
        if let Event::Data { bytes, .. } = c.next_event().unwrap() {
            got.extend(bytes);
            let text = String::from_utf8_lossy(&got).into_owned();
            if text.contains(needle) {
                return text;
            }
        }
    }
}

fn wait_until(what: &str, mut f: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(30);
    while !f() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_controller_connects_and_gets_output_and_the_exit() {
    let (_d, r) = root();
    let info = start(&r, "a", &echo_and_exit());
    let mut c = connect(&info.sock);
    c.attach(0).unwrap();
    let mut text = String::new();
    let rec = loop {
        match c.next_event().unwrap() {
            Event::Data { bytes, .. } => text.push_str(&String::from_utf8_lossy(&bytes)),
            Event::Heartbeat { .. } => {}
            Event::Exit(rec) => break rec,
        }
    };
    let text = text.trim();
    assert_eq!(text, "out");
    assert_eq!(rec.code, 0);
}

#[test]
fn a_new_controller_reattaches_by_the_endpoint_on_disk_after_the_first_is_gone() {
    let (_d, r) = root();
    let info = start(&r, "a", &print_then_wait());
    let mut first = connect(&info.sock);
    let seen = read_until(&mut first, 0, "first");
    drop(first); // the controller goes away; the host and its child keep running
    assert!(pid_alive(info.host_pid) && pid_alive(info.child_pid));

    // A new daemon finds the host from its record, not from anything in memory.
    let found = HostInfo::read(&r.join("a")).unwrap();
    assert_eq!(found.sock, info.sock);
    let mut second = connect(&found.sock);
    let again = read_until(&mut second, 0, "first");
    assert_eq!(again, seen, "the journal replays from the start");
    second.stop(100).unwrap();
    wait_until("the child to end", || !pid_alive(info.child_pid));
}

#[test]
fn stop_ends_the_child_and_the_exit_is_reported() {
    let (_d, r) = root();
    let info = start(&r, "a", &print_then_wait());
    let mut c = connect(&info.sock);
    read_until(&mut c, 0, "first");
    assert!(pid_alive(info.child_pid));
    c.stop(100).unwrap();
    loop {
        match c.next_event().unwrap() {
            Event::Exit(rec) => {
                assert_ne!(rec.code, 0, "it was stopped, not finished");
                break;
            }
            Event::Data { .. } | Event::Heartbeat { .. } => {}
        }
    }
    wait_until("the child to end", || !pid_alive(info.child_pid));
}
