// SPDX-License-Identifier: GPL-3.0-or-later
//! A counting lock shared by every process on the machine: `limit` lock files under one
//! directory, each taken with `flock`. A scratch world holds one for as long as it lives.
//! The kernel drops a lock when its process dies, so a killed test run leaves nothing stale.
//!
//! The slots are shared fairly between the processes that want them. A process that holds
//! or waits for a slot also holds a presence lock (`procs/<pid>.lock`), and no process
//! takes more than its share: `limit` divided by the processes present, rounded up.
//! Without that, one test binary with many threads re-took every slot the moment it came
//! free (its threads poll far more often than another binary's few), and the other
//! binaries' tests sat idle until their patience ran out, holding their suites' test-suite
//! slots the whole time.

use nix::fcntl::{Flock, FlockArg};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::thread::ThreadId;
use std::time::{Duration, Instant};

/// One held slot; dropping it releases the lock and this process's claim on it.
pub struct Held {
    dir: PathBuf,
    /// The thread that took it, which may not be the one that drops it.
    taker: ThreadId,
    _lock: Flock<File>,
}

impl Drop for Held {
    fn drop(&mut self) {
        let mut st = state();
        let s = entry(&mut st, &self.dir);
        s.own = s.own.saturating_sub(1);
        if let Some(i) = s.takers.iter().position(|t| *t == self.taker) {
            s.takers.swap_remove(i);
        }
        drop(st);
        leave(&self.dir);
    }
}

/// This process's standing in one slot directory.
#[derive(Default)]
struct DirState {
    /// Slots this process holds.
    own: usize,
    /// The thread that took each of them.
    takers: Vec<ThreadId>,
    /// Held slots, waiting `take`s and `Presence` guards. The presence lock is held while
    /// this is above zero.
    users: usize,
    presence: Option<Flock<File>>,
}

static STATE: Mutex<Option<HashMap<PathBuf, DirState>>> = Mutex::new(None);

fn state() -> MutexGuard<'static, Option<HashMap<PathBuf, DirState>>> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

fn entry<'a>(st: &'a mut Option<HashMap<PathBuf, DirState>>, dir: &Path) -> &'a mut DirState {
    st.get_or_insert_with(HashMap::new)
        .entry(dir.to_path_buf())
        .or_default()
}

/// Counts this process as present in a slot directory until it is dropped, even while it
/// holds no slot, so the processes that do hold them leave it its share.
pub struct Presence {
    dir: PathBuf,
}

impl Drop for Presence {
    fn drop(&mut self) {
        leave(&self.dir);
    }
}

/// Be present in `dir` (see [`Presence`]).
pub fn join(dir: &Path) -> Presence {
    enter(dir);
    Presence {
        dir: dir.to_path_buf(),
    }
}

fn enter(dir: &Path) {
    let mut st = state();
    let s = entry(&mut st, dir);
    s.users += 1;
    if s.presence.is_none() {
        s.presence = take_presence(dir);
    }
}

fn leave(dir: &Path) {
    let mut st = state();
    let s = entry(&mut st, dir);
    s.users = s.users.saturating_sub(1);
    if s.users == 0 && s.presence.take().is_some() {
        let _ = std::fs::remove_file(presence_path(dir));
    }
}

fn procs_dir(dir: &Path) -> PathBuf {
    dir.join("procs")
}

fn presence_path(dir: &Path) -> PathBuf {
    procs_dir(dir).join(format!("{}.lock", std::process::id()))
}

/// Lock this process's presence file. Another process sweeping stale files may unlink it
/// between our open and our lock, so the lock counts only if the path still names the file
/// we locked; otherwise open it again. `None` only if the directory is unusable, and then
/// this process is not counted: it still gets slots, others just keep no share for it.
fn take_presence(dir: &Path) -> Option<Flock<File>> {
    std::fs::create_dir_all(procs_dir(dir)).ok()?;
    let path = presence_path(dir);
    for _ in 0..8 {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .ok()?;
        let lock = Flock::lock(file, FlockArg::LockExclusiveNonblock).ok()?;
        let ours = lock.metadata().ok().map(|m| m.ino());
        let named = std::fs::metadata(&path).ok().map(|m| m.ino());
        if ours.is_some() && ours == named {
            return Some(lock);
        }
    }
    None
}

/// How many processes are present in `dir`, this one included when it is. A presence file
/// nobody holds belongs to a process that has gone, and is removed.
pub fn present(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(procs_dir(dir)) else {
        return 0;
    };
    let mut live = 0;
    for e in entries.flatten() {
        let path = e.path();
        if path.extension().and_then(|x| x.to_str()) != Some("lock") {
            continue;
        }
        let Ok(f) = OpenOptions::new().read(true).write(true).open(&path) else {
            continue;
        };
        // flock locks belong to the open file, so this process's own presence is busy to
        // the probe too.
        match Flock::lock(f, FlockArg::LockExclusiveNonblock) {
            Err(_) => live += 1,
            Ok(stale) => {
                let _ = std::fs::remove_file(&path);
                drop(stale);
            }
        }
    }
    live
}

/// The most slots one process may hold out of `limit` while `procs` processes want them.
pub fn share(limit: usize, procs: usize) -> usize {
    limit.div_ceil(procs.max(1)).max(1)
}

/// The directory every test binary on this machine shares. `/tmp`, not `$TMPDIR`: the
/// scratch worlds themselves live there, and a per-task TMPDIR would split the count.
pub fn shared_dir() -> PathBuf {
    PathBuf::from("/tmp/mason-test-worlds")
}

/// Take a free slot of `limit` in `dir` without waiting, or `None` when all are held or
/// this process already holds its share.
pub fn try_take(dir: &Path, limit: usize) -> Option<Held> {
    std::fs::create_dir_all(dir).ok()?;
    enter(dir);
    let procs = present(dir);
    let mut st = state();
    let s = entry(&mut st, dir);
    let lock = if s.own < share(limit, procs) {
        lock_free_slot(dir, limit)
    } else {
        None
    };
    match lock {
        Some(lock) => {
            s.own += 1;
            let taker = std::thread::current().id();
            s.takers.push(taker);
            // The `enter` above stays counted for as long as the slot is held.
            Some(Held {
                dir: dir.to_path_buf(),
                taker,
                _lock: lock,
            })
        }
        None => {
            drop(st);
            leave(dir);
            None
        }
    }
}

fn lock_free_slot(dir: &Path, limit: usize) -> Option<Flock<File>> {
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
            return Some(lock);
        }
    }
    None
}

/// Whether the calling thread holds a slot it took in `dir`.
fn holds_one(dir: &Path) -> bool {
    let me = std::thread::current().id();
    entry(&mut state(), dir).takers.contains(&me)
}

/// Wait up to `patience` for a slot, counted as present the whole time so the processes
/// holding slots leave this one its share. `None` after that, and the caller goes ahead.
///
/// A thread that already holds one does not wait: it gets a free slot or `None` at once. A test
/// that holds one world and makes another would otherwise wait while keeping what others like it
/// wait for, and they all sat out the whole patience (600 s at a time on a two-slot runner).
pub fn take(dir: &Path, limit: usize, patience: Duration) -> Option<Held> {
    if holds_one(dir) {
        return try_take(dir, limit);
    }
    let _waiting = join(dir);
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
