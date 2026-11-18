// SPDX-License-Identifier: GPL-3.0-or-later
//! A counting lock shared by every process on the machine: `limit` lock files under one
//! directory, each taken with `flock`. A scratch world holds one for as long as it lives.
//! The kernel drops a lock when its process dies, so a killed test run leaves nothing stale.

use nix::fcntl::{Flock, FlockArg};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// One held slot; dropping it releases the lock.
pub struct Held {
    _lock: Flock<File>,
}

/// The directory every test binary on this machine shares. `/tmp`, not `$TMPDIR`: the
/// scratch worlds themselves live there, and a per-task TMPDIR would split the count.
pub fn shared_dir() -> PathBuf {
    PathBuf::from("/tmp/mason-test-worlds")
}

/// Take a free slot of `limit` in `dir` without waiting, or `None` when all are held.
pub fn try_take(dir: &Path, limit: usize) -> Option<Held> {
    std::fs::create_dir_all(dir).ok()?;
    for i in 0..limit {
        let Ok(file) = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(dir.join(format!("slot-{i}.lock")))
        else {
            continue;
        };
        if let Ok(lock) = Flock::lock(file, FlockArg::LockExclusiveNonblock) {
            return Some(Held { _lock: lock });
        }
    }
    None
}

/// Wait up to `patience` for a slot. `None` after that: a test that holds one world and
/// makes another could wait for ever if every slot were held that way, so it goes ahead.
pub fn take(dir: &Path, limit: usize, patience: Duration) -> Option<Held> {
    let end = Instant::now() + patience;
    loop {
        if let Some(h) = try_take(dir, limit) {
            return Some(h);
        }
        if Instant::now() >= end {
            return None;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// How many of the `limit` slots in `dir` are held right now, by anybody.
pub fn held_count(dir: &Path, limit: usize) -> usize {
    (0..limit)
        .filter(|i| {
            let Ok(f) = OpenOptions::new()
                .read(true)
                .write(true)
                .open(dir.join(format!("slot-{i}.lock")))
            else {
                return false;
            };
            Flock::lock(f, FlockArg::LockExclusiveNonblock).is_err()
        })
        .count()
}
