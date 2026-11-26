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
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// One held slot; dropping it releases the lock and this process's claim on it.
pub struct Held {
    dir: PathBuf,
    _lock: Flock<File>,
}

impl Drop for Held {
    fn drop(&mut self) {
        let mut st = state();
        let s = entry(&mut st, &self.dir);
        s.own = s.own.saturating_sub(1);
        drop(st);
        leave(&self.dir);
    }
}

/// Open a lock file for `flock`, which needs no write access: another user's file (the
/// pipeline's node jobs and an interactive session share this directory on one machine under
/// different accounts) opens read-only. A file that does not exist yet is created writable
/// by everybody, so whoever comes first does not lock the rest out.
fn open_lock(path: &Path) -> Option<File> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    if let Ok(f) = OpenOptions::new().read(true).open(path) {
        return Some(f);
    }
    let f = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o666)
        .open(path)
        .ok()?;
    // The creating process's umask may have narrowed the mode.
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o666));
    Some(f)
}

/// Create `dir` (and parents) open to every user, for the same reason.
fn make_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let fresh = !dir.exists();
    std::fs::create_dir_all(dir)?;
    if fresh {
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o777));
    }
    Ok(())
}

/// This process's standing in one slot directory.
#[derive(Default)]
struct DirState {
    /// Slots this process holds.
    own: usize,
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
    make_dir(&procs_dir(dir)).ok()?;
    let path = presence_path(dir);
    for _ in 0..8 {
        let file = open_lock(&path)?;
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
        let Ok(f) = OpenOptions::new().read(true).open(&path) else {
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
    make_dir(dir).ok()?;
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
            // The `enter` above stays counted for as long as the slot is held.
            Some(Held {
                dir: dir.to_path_buf(),
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
        let Some(file) = open_lock(&dir.join(format!("slot-{i}.lock"))) else {
            continue;
        };
        if let Ok(lock) = Flock::lock(file, FlockArg::LockExclusiveNonblock) {
            return Some(lock);
        }
    }
    None
}

/// Wait up to `patience` for a slot, counted as present the whole time so the processes
/// holding slots leave this one its share. `None` after that: a test that holds one world
/// and makes another could wait for ever if every slot were held that way, so it goes ahead.
pub fn take(dir: &Path, limit: usize, patience: Duration) -> Option<Held> {
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

thread_local! {
    /// Cloned into every [`Claim`] this thread makes, so its count says whether one lives.
    static CLAIMING: Arc<()> = Arc::new(());
}

/// One scratch world's claim: a slot, or none when it went ahead without one.
pub struct Claim {
    pub held: Option<Held>,
    _mark: Arc<()>,
}

/// [`take`] for a thread's first live claim. A thread (a test) that already has one takes a
/// free slot or goes ahead at once: waiting could not end any other way if every slot were
/// held by tests wanting another, and the machine ends up no more loaded than after the wait.
pub fn claim(dir: &Path, limit: usize, patience: Duration) -> Claim {
    let mark = CLAIMING.with(Arc::clone);
    // The thread's own reference and `mark`; any more belong to live claims.
    let held = if Arc::strong_count(&mark) > 2 {
        try_take(dir, limit)
    } else {
        take(dir, limit, patience)
    };
    Claim { held, _mark: mark }
}

/// How many of the `limit` slots in `dir` are held right now, by anybody.
pub fn held_count(dir: &Path, limit: usize) -> usize {
    (0..limit)
        .filter(|i| {
            let Ok(f) = OpenOptions::new()
                .read(true)
                .open(dir.join(format!("slot-{i}.lock")))
            else {
                return false;
            };
            Flock::lock(f, FlockArg::LockExclusiveNonblock).is_err()
        })
        .count()
}
