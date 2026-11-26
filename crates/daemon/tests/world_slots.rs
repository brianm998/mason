// SPDX-License-Identifier: GPL-3.0-or-later
#![cfg(unix)]
//! The scratch-world limit holds across processes, counted by lock files held.

mod support;

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use support::slots;

const LIMIT: usize = 3;

/// Run as a child of `limit_holds_across_processes`: take as many slots as it can,
/// say how many, and hold them until the parent closes stdin.
#[test]
fn slots_child() {
    let Some(dir) = std::env::var_os("MASON_SLOTS_CHILD_DIR") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let mut held = Vec::new();
    while held.len() < LIMIT + 2 {
        match slots::try_take(&dir, LIMIT) {
            Some(h) => held.push(h),
            None => break,
        }
    }
    // libtest has already written "test slots_child ... " without a newline.
    println!("\nHELD {}", held.len());
    let mut s = String::new();
    let _ = std::io::stdin().read_line(&mut s);
}

#[test]
fn limit_holds_across_processes() {
    let dir = tempfile::tempdir().unwrap();
    let exe = std::env::current_exe().unwrap();
    let mut children: Vec<_> = (0..2)
        .map(|_| {
            Command::new(&exe)
                .args(["--exact", "slots_child", "--nocapture", "--test-threads=1"])
                .env("MASON_SLOTS_CHILD_DIR", dir.path())
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    // Read each child's report on a thread, so a child that never reports fails the test
    // (with what it did print) instead of leaving it waiting for ever.
    let mut total = 0;
    for c in &mut children {
        let out = c.stdout.take().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut seen = Vec::new();
            let mut lines = BufReader::new(out).lines().map_while(Result::ok);
            while let Some(l) = lines.next() {
                if let Some(n) = l.trim().strip_prefix("HELD ") {
                    let _ = tx.send(Ok(n.parse::<usize>().unwrap()));
                    // Keep draining: a closed pipe would fail the child's own output.
                    for _ in lines {}
                    return;
                }
                seen.push(l);
            }
            let _ = tx.send(Err(seen));
        });
        match rx.recv_timeout(std::time::Duration::from_secs(120)) {
            Ok(Ok(n)) => total += n,
            other => {
                for c in &mut children {
                    let _ = c.kill();
                }
                panic!("child did not report: {other:?}");
            }
        }
    }
    // Both children are still holding: nothing is timed, the locks are counted.
    let held = slots::held_count(dir.path(), LIMIT);
    assert!(held <= LIMIT, "{held} slots held of {LIMIT}");
    assert_eq!(total, LIMIT, "two children together got {total}");
    assert_eq!(held, LIMIT);
    assert!(slots::try_take(dir.path(), LIMIT).is_none());
    for c in &mut children {
        drop(c.stdin.take());
        assert!(c.wait().unwrap().success());
    }
    assert_eq!(slots::held_count(dir.path(), LIMIT), 0);
}

/// Run as a child of `a_process_waiting_for_a_slot_gets_its_share`: be present in the
/// directory without holding a slot, say so, and stay until the parent closes stdin.
#[test]
fn present_child() {
    let Some(dir) = std::env::var_os("MASON_PRESENT_CHILD_DIR") else {
        return;
    };
    let _here = slots::join(std::path::Path::new(&dir));
    println!("\nPRESENT");
    let mut s = String::new();
    let _ = std::io::stdin().read_line(&mut s);
}

/// Wait for a child's marker line, keeping its stdout drained afterwards.
fn await_line(c: &mut std::process::Child, marker: &'static str) {
    let out = c.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut lines = BufReader::new(out).lines().map_while(Result::ok);
        while let Some(l) = lines.next() {
            if l.trim() == marker {
                let _ = tx.send(());
                for _ in lines {}
                return;
            }
        }
    });
    if rx
        .recv_timeout(std::time::Duration::from_secs(120))
        .is_err()
    {
        let _ = c.kill();
        panic!("child never printed {marker}");
    }
}

#[test]
fn a_process_waiting_for_a_slot_gets_its_share() {
    const FOUR: usize = 4;
    let dir = tempfile::tempdir().unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "present_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("MASON_PRESENT_CHILD_DIR", dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    await_line(&mut child, "PRESENT");

    // Two processes want slots, so this one stops at half of them and leaves the rest free.
    let mut held: Vec<_> = std::iter::from_fn(|| slots::try_take(dir.path(), FOUR)).collect();
    assert_eq!(held.len(), slots::share(FOUR, 2));
    assert_eq!(slots::held_count(dir.path(), FOUR), 2);

    // Once the other has gone, its share is this one's to take.
    drop(child.stdin.take());
    assert!(child.wait().unwrap().success());
    held.extend(std::iter::from_fn(|| slots::try_take(dir.path(), FOUR)));
    assert_eq!(held.len(), FOUR);
    drop(held);
    assert_eq!(slots::held_count(dir.path(), FOUR), 0);
}

#[test]
fn a_presence_file_nobody_holds_is_not_counted_and_is_swept() {
    let dir = tempfile::tempdir().unwrap();
    let procs = dir.path().join("procs");
    std::fs::create_dir_all(&procs).unwrap();
    let stale = procs.join("1.lock");
    std::fs::write(&stale, "").unwrap();
    assert_eq!(slots::present(dir.path()), 0);
    assert!(!stale.exists());
    // Alone, this process may take every slot.
    let held: Vec<_> = std::iter::from_fn(|| slots::try_take(dir.path(), 3)).collect();
    assert_eq!(held.len(), 3);
    assert_eq!(slots::present(dir.path()), 1);
    drop(held);
    assert_eq!(slots::present(dir.path()), 0);
}

#[test]
fn a_thread_holding_a_world_takes_its_next_one_without_waiting() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_path_buf();
    // On a thread of its own, so a claim that waits fails the test instead of hanging it.
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let patience = std::time::Duration::from_secs(24 * 3600);
        let first = slots::claim(&path, 1, patience);
        let second = slots::claim(&path, 1, patience);
        let got = (first.held.is_some(), second.held.is_some());
        // With the first gone but the second alive, the thread still holds a world.
        drop(first);
        let third = slots::claim(&path, 1, patience);
        let fourth = slots::claim(&path, 1, patience);
        let later = (third.held.is_some(), fourth.held.is_some());
        drop((second, third, fourth));
        let alone = slots::claim(&path, 1, patience).held.is_some();
        let _ = tx.send((got, later, alone));
    });
    let seen = rx
        .recv_timeout(std::time::Duration::from_secs(120))
        .expect("a claim waited while its thread held a world");
    // A free slot is still taken; with none free the claim goes ahead without one.
    assert_eq!(seen, ((true, false), (true, false), true));
}

#[test]
fn shares_round_up_so_no_slot_is_left_idle() {
    assert_eq!(slots::share(8, 1), 8);
    assert_eq!(slots::share(8, 3), 3);
    assert_eq!(slots::share(3, 2), 2);
    assert_eq!(slots::share(2, 5), 1);
    assert_eq!(slots::share(4, 0), 4);
}
