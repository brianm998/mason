// SPDX-License-Identifier: GPL-3.0-or-later
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
    println!("HELD {}", held.len());
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
    let mut total = 0;
    for c in &mut children {
        let out = BufReader::new(c.stdout.as_mut().unwrap());
        let n = out
            .lines()
            .map_while(Result::ok)
            .find_map(|l| l.strip_prefix("HELD ").map(|n| n.parse::<usize>().unwrap()))
            .expect("child reported");
        total += n;
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
