// SPDX-License-Identifier: GPL-3.0-or-later

//! Free disk space, for the disk floor (s85). The scheduler is pure and is only handed a number;
//! reading the volume happens here. Like the CPU reading it is injectable: tests and exit scripts
//! never depend on the real disk (`MASON_DISK_FILE`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The default `[disk] floor_gb`.
pub const DEFAULT_FLOOR_GB: u64 = 20;

/// How often the volume is read.
pub const DEFAULT_EVERY: Duration = Duration::from_secs(30);

/// Where the number comes from.
pub trait DiskSource: Send + Sync + std::fmt::Debug {
    /// Megabytes free for a non-root process on the volume holding `path`, or `None` if it
    /// cannot be read (no gate then).
    fn free_mb(&self, path: &Path) -> Option<u64>;
}

/// The real volume, by `statvfs` (`GetDiskFreeSpaceExW` on Windows).
#[derive(Debug)]
pub struct Statvfs;

impl DiskSource for Statvfs {
    fn free_mb(&self, path: &Path) -> Option<u64> {
        let bytes = mason_platform::disk::available_bytes(path)?;
        Some((bytes / (1024 * 1024)) as u64)
    }
}

/// A fixed, settable value, the same for every path.
#[derive(Debug)]
pub struct FakeDisk(AtomicU64);

impl FakeDisk {
    pub fn new_gb(gb: u64) -> Arc<FakeDisk> {
        Arc::new(FakeDisk(AtomicU64::new(gb * 1024)))
    }
    pub fn set_gb(&self, gb: u64) {
        self.0.store(gb * 1024, Ordering::SeqCst);
    }
    pub fn set_mb(&self, mb: u64) {
        self.0.store(mb, Ordering::SeqCst);
    }
}

impl DiskSource for FakeDisk {
    fn free_mb(&self, _: &Path) -> Option<u64> {
        Some(self.0.load(Ordering::SeqCst))
    }
}

/// Gigabytes free (a decimal number) in a file, read each time (`MASON_DISK_FILE`: how the exit
/// script drives the floor against a real daemon).
#[derive(Debug)]
pub struct FileDisk(pub PathBuf);

impl DiskSource for FileDisk {
    fn free_mb(&self, _: &Path) -> Option<u64> {
        let gb: f64 = std::fs::read_to_string(&self.0).ok()?.trim().parse().ok()?;
        (gb >= 0.0).then_some((gb * 1024.0) as u64)
    }
}

/// The nearest directory at or above `p` that exists: a `task_cache` root may not be made yet.
pub fn nearest_existing(p: &Path) -> Option<PathBuf> {
    p.ancestors().find(|a| a.exists()).map(Path::to_path_buf)
}

/// The least free space over the volumes the paths are on.
pub fn least_free_mb(src: &dyn DiskSource, paths: &[PathBuf]) -> Option<u64> {
    paths
        .iter()
        .filter_map(|p| src.free_mb(&nearest_existing(p)?))
        .min()
}

/// Bytes under `p`, not following symlinks.
pub fn dir_size(p: &Path) -> u64 {
    let Ok(m) = std::fs::symlink_metadata(p) else {
        return 0;
    };
    if !m.is_dir() {
        return m.len();
    }
    let Ok(rd) = std::fs::read_dir(p) else {
        return 0;
    };
    rd.flatten().map(|e| dir_size(&e.path())).sum()
}

/// What MASON knows is taking room, measured on a thread of its own: a 100 GB tree takes a
/// while to walk, and the alert list is asked for often.
#[derive(Default)]
pub struct Sizes {
    inner: Mutex<SizesInner>,
}

#[derive(Default)]
struct SizesInner {
    at: Option<Instant>,
    measuring: bool,
    got: BTreeMap<String, u64>,
}

impl Sizes {
    /// The last measurement of each named group of directories. Starts a new one in the
    /// background when the last is older than `max_age` (or there is none).
    pub fn get(
        self: &Arc<Self>,
        groups: Vec<(String, Vec<PathBuf>)>,
        max_age: Duration,
    ) -> BTreeMap<String, u64> {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let stale = g.at.is_none_or(|t| t.elapsed() > max_age);
        if stale && !g.measuring {
            g.measuring = true;
            let me = self.clone();
            let spawned = std::thread::Builder::new()
                .name("disk-sizes".into())
                .spawn(move || {
                    let got: BTreeMap<String, u64> = groups
                        .into_iter()
                        .map(|(n, ps)| (n, ps.iter().map(|p| dir_size(p)).sum()))
                        .collect();
                    let mut g = me.inner.lock().unwrap_or_else(|e| e.into_inner());
                    g.got = got;
                    g.at = Some(Instant::now());
                    g.measuring = false;
                });
            if spawned.is_err() {
                g.measuring = false;
            }
        }
        g.got.clone()
    }
}

/// Is this store error the disk being full?
pub fn is_disk_full(e: &mason_store::StoreError) -> bool {
    let t = format!("{e:?}").to_lowercase();
    t.contains("diskfull")
        || t.contains("database or disk is full")
        || t.contains("no space left")
        || t.contains("storagefull")
}

pub fn gb(mb: u64) -> u64 {
    mb / 1024
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_disk_is_recognised_in_a_store_error() {
        let full = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_FULL),
            Some("database or disk is full".into()),
        );
        assert!(is_disk_full(&mason_store::StoreError::Sqlite(full)));
        assert!(!is_disk_full(&mason_store::StoreError::WriterGone));
    }

    #[test]
    fn the_real_volume_reads_something() {
        assert!(Statvfs
            .free_mb(&std::env::temp_dir())
            .is_some_and(|m| m > 0));
    }

    #[test]
    fn a_file_reading_takes_gigabytes_and_fractions() {
        let t = tempfile::tempdir().unwrap();
        let f = t.path().join("disk");
        std::fs::write(&f, "12\n").unwrap();
        assert_eq!(FileDisk(f.clone()).free_mb(t.path()), Some(12 * 1024));
        std::fs::write(&f, "0.5").unwrap();
        assert_eq!(FileDisk(f.clone()).free_mb(t.path()), Some(512));
        std::fs::write(&f, "junk").unwrap();
        assert_eq!(FileDisk(f).free_mb(t.path()), None);
    }

    #[test]
    fn a_root_that_is_not_made_yet_is_read_on_its_nearest_parent() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("a/b/c");
        assert_eq!(nearest_existing(&p), Some(t.path().to_path_buf()));
        let d = FakeDisk::new_gb(7);
        assert_eq!(least_free_mb(&*d, &[p]), Some(7 * 1024));
    }

    #[test]
    fn sizes_add_up_and_skip_symlinks() {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir(t.path().join("x")).unwrap();
        std::fs::write(t.path().join("x/f"), vec![0u8; 1000]).unwrap();
        std::os::unix::fs::symlink(t.path().join("x"), t.path().join("l")).unwrap();
        assert!(dir_size(&t.path().join("x")) >= 1000);
        assert!(dir_size(t.path()) < 2000 + 8192);
    }
}
