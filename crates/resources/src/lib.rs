// SPDX-License-Identifier: GPL-3.0-or-later

//! Machine classes (`compile`, `test-suite`, `exclusive`) held by kernel file
//! locks, so a dead holder frees its resource automatically (Part 7).
//!
//! Layout under the locks directory:
//!
//! * `<class>.<i>.lock`: one file per slot of a class. A holder takes the
//!   slots it needs with a non-blocking exclusive `flock`.
//! * `machine.lock`: every holder of a machine class takes it *shared*; an
//!   exclusive (whole machine) holder takes it exclusively, so the two can
//!   never overlap.
//! * `lease-<id>.lock`: a marker each holder locks for as long as it lives.
//!   Whether a lease is still alive is whether that marker is still locked.
//! * `<class>.<i>.holder`: who holds a slot and since when, for display.
//!
//! Nothing here queues. Order is decided by `mason-scheduler`; this only
//! makes the grant real, and proves after the fact that a holder is gone.

use nix::fcntl::{Flock, FlockArg};
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

/// What a holder wants from the machine classes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Take {
    /// Units of each machine class.
    pub classes: BTreeMap<String, u32>,
    /// The whole machine.
    pub exclusive: bool,
}

/// What a slot's sidecar says about its holder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HolderInfo {
    pub label: String,
    pub since_ms: u64,
    /// The process that took the slot, and its lease (0 when an older holder
    /// did not record them).
    pub pid: u32,
    pub lease: u64,
}

#[derive(Debug)]
pub enum TakeError {
    /// Something is held by another process right now.
    Busy,
    Io(io::Error),
}

impl From<io::Error> for TakeError {
    fn from(e: io::Error) -> Self {
        TakeError::Io(e)
    }
}

impl std::fmt::Display for TakeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TakeError::Busy => write!(f, "a slot is held by another process"),
            TakeError::Io(e) => write!(f, "{e}"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Locks {
    dir: PathBuf,
}

/// Everything one holder has locked. Dropping it (or the process dying)
/// releases every lock.
#[derive(Debug)]
pub struct Held {
    // Held only to keep the kernel locks; released on drop.
    _locks: Vec<Flock<File>>,
    paths: Vec<PathBuf>,
    marker: PathBuf,
}

impl Held {
    /// The slot files this holder has, for display.
    pub fn slots(&self) -> &[PathBuf] {
        &self.paths
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        for p in &self.paths {
            let _ = std::fs::remove_file(p.with_extension("holder"));
        }
        let _ = std::fs::remove_file(&self.marker);
    }
}

fn open(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
}

fn lock(path: &Path, arg: FlockArg) -> Result<Option<Flock<File>>, io::Error> {
    match Flock::lock(open(path)?, arg) {
        Ok(l) => Ok(Some(l)),
        Err((_, nix::errno::Errno::EWOULDBLOCK)) => Ok(None),
        Err((_, e)) => Err(io::Error::from(e)),
    }
}

impl Locks {
    pub fn new(dir: impl Into<PathBuf>) -> io::Result<Locks> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)?;
        Ok(Locks { dir })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn slot(&self, class: &str, i: u32) -> PathBuf {
        self.dir.join(format!("{class}.{i}.lock"))
    }

    fn marker(&self, lease: u64) -> PathBuf {
        self.dir.join(format!("lease-{lease}.lock"))
    }

    /// Take everything or nothing. `capacity` is each class's current limit
    /// (how many slot files exist for it).
    pub fn take(
        &self,
        lease: u64,
        label: &str,
        since_ms: u64,
        want: &Take,
        capacity: &BTreeMap<String, u32>,
    ) -> Result<Held, TakeError> {
        let mut locks: Vec<Flock<File>> = vec![];
        let mut paths: Vec<PathBuf> = vec![];
        let machine = self.dir.join("machine.lock");
        let uses_machine = want.exclusive || !want.classes.is_empty();
        if uses_machine {
            let arg = if want.exclusive {
                FlockArg::LockExclusiveNonblock
            } else {
                FlockArg::LockSharedNonblock
            };
            match lock(&machine, arg)? {
                Some(l) => locks.push(l),
                None => return Err(TakeError::Busy),
            }
        }
        for (class, n) in &want.classes {
            let cap = capacity.get(class).copied().unwrap_or(0);
            let mut got = 0;
            for i in 0..cap {
                if got == *n {
                    break;
                }
                let p = self.slot(class, i);
                if let Some(l) = lock(&p, FlockArg::LockExclusiveNonblock)? {
                    locks.push(l);
                    paths.push(p);
                    got += 1;
                }
            }
            if got < *n {
                // Dropping `locks` releases the partial set: all or nothing.
                return Err(TakeError::Busy);
            }
        }
        let marker = self.marker(lease);
        match lock(&marker, FlockArg::LockExclusiveNonblock)? {
            Some(l) => locks.push(l),
            None => return Err(TakeError::Busy),
        }
        for p in &paths {
            let _ = std::fs::write(
                p.with_extension("holder"),
                format!("{label}\t{since_ms}\t{}\t{lease}\n", std::process::id()),
            );
        }
        Ok(Held {
            _locks: locks,
            paths,
            marker,
        })
    }

    /// Is the holder of this lease still running? True while its marker is
    /// locked. A lease that never locked its marker reads as not alive, so a
    /// caller gives a new grant a moment to take its locks before asking.
    pub fn alive(&self, lease: u64) -> bool {
        let m = self.marker(lease);
        if !m.exists() {
            return false;
        }
        match lock(&m, FlockArg::LockExclusiveNonblock) {
            Ok(Some(_ours)) => {
                // We got it, so nobody holds it: the holder is gone.
                let _ = std::fs::remove_file(&m);
                false
            }
            Ok(None) => true,
            Err(_) => true,
        }
    }

    /// Who holds each slot of a class right now (`None` is free). Reads the
    /// kernel, not the sidecar, to decide whether a slot is held.
    pub fn slots(&self, class: &str, capacity: u32) -> Vec<Option<HolderInfo>> {
        (0..capacity)
            .map(|i| {
                let p = self.slot(class, i);
                if !p.exists() {
                    return None;
                }
                match lock(&p, FlockArg::LockExclusiveNonblock) {
                    Ok(Some(_free)) => None,
                    _ => Some(
                        std::fs::read_to_string(p.with_extension("holder"))
                            .ok()
                            .and_then(|t| {
                                let mut f = t.trim_end().split('\t');
                                let label = f.next()?.to_string();
                                let since_ms = f.next()?.parse().ok()?;
                                let pid = f.next().and_then(|v| v.parse().ok()).unwrap_or(0);
                                let lease = f.next().and_then(|v| v.parse().ok()).unwrap_or(0);
                                Some(HolderInfo {
                                    label,
                                    since_ms,
                                    pid,
                                    lease,
                                })
                            })
                            .unwrap_or(HolderInfo {
                                label: "unknown".into(),
                                since_ms: 0,
                                pid: 0,
                                lease: 0,
                            }),
                    ),
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(items: &[(&str, u32)]) -> BTreeMap<String, u32> {
        items.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    fn want(items: &[(&str, u32)], exclusive: bool) -> Take {
        Take {
            classes: caps(items),
            exclusive,
        }
    }

    #[test]
    fn slots_are_taken_all_or_nothing_and_released_on_drop() {
        let d = tempfile::tempdir().unwrap();
        let l = Locks::new(d.path()).unwrap();
        let cap = caps(&[("compile", 2)]);
        let a = l
            .take(1, "#1", 5, &want(&[("compile", 1)], false), &cap)
            .unwrap();
        let b = l
            .take(2, "#2", 6, &want(&[("compile", 1)], false), &cap)
            .unwrap();
        assert!(matches!(
            l.take(3, "#3", 7, &want(&[("compile", 1)], false), &cap),
            Err(TakeError::Busy)
        ));
        let held = l.slots("compile", 2);
        assert!(held.iter().all(|s| s.is_some()));
        assert_eq!(held[0].as_ref().unwrap().label, "#1");
        drop(a);
        // Two wanted, one free: nothing is kept.
        assert!(matches!(
            l.take(4, "#4", 8, &want(&[("compile", 2)], false), &cap),
            Err(TakeError::Busy)
        ));
        assert!(
            l.slots("compile", 2)[0].is_none(),
            "the partial take was undone"
        );
        drop(b);
        assert!(l
            .take(5, "#5", 9, &want(&[("compile", 2)], false), &cap)
            .is_ok());
    }

    #[test]
    fn exclusive_and_machine_class_holders_never_overlap() {
        let d = tempfile::tempdir().unwrap();
        let l = Locks::new(d.path()).unwrap();
        let cap = caps(&[("compile", 4)]);
        let c = l
            .take(1, "#1", 0, &want(&[("compile", 1)], false), &cap)
            .unwrap();
        assert!(matches!(
            l.take(2, "#2", 0, &want(&[], true), &cap),
            Err(TakeError::Busy)
        ));
        drop(c);
        let e = l.take(2, "#2", 0, &want(&[], true), &cap).unwrap();
        assert!(matches!(
            l.take(3, "#3", 0, &want(&[("compile", 1)], false), &cap),
            Err(TakeError::Busy)
        ));
        drop(e);
        assert!(l
            .take(3, "#3", 0, &want(&[("compile", 1)], false), &cap)
            .is_ok());
    }

    #[test]
    fn a_lease_is_alive_while_its_marker_is_locked() {
        let d = tempfile::tempdir().unwrap();
        let l = Locks::new(d.path()).unwrap();
        let cap = caps(&[("compile", 1)]);
        assert!(!l.alive(1), "never taken");
        let h = l
            .take(1, "#1", 0, &want(&[("compile", 1)], false), &cap)
            .unwrap();
        assert!(l.alive(1));
        drop(h);
        assert!(!l.alive(1));
    }
}
