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

    /// The volume holding `path` (which exists), with its free and total bytes, or `None` if it
    /// cannot be read. For the history; the floor uses `free_mb`.
    fn volume(&self, _path: &Path) -> Option<VolumeReading> {
        None
    }
}

/// One volume as read: who it is and how full.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeReading {
    pub id: String,
    pub name: String,
    pub mount: PathBuf,
    pub free_bytes: u64,
    pub total_bytes: u64,
}

/// A volume's display name: the mount point's last component; for `/` the label macOS shows
/// (the entry in `/Volumes` that links to `/`, `Macintosh HD`), else `/`; the drive on Windows.
pub fn volume_name(mount: &Path) -> String {
    if let Some(n) = mount.file_name() {
        return n.to_string_lossy().into_owned();
    }
    #[cfg(unix)]
    {
        if mount == Path::new("/") {
            if let Ok(rd) = std::fs::read_dir("/Volumes") {
                let mut names: Vec<String> = rd
                    .flatten()
                    .filter(|e| std::fs::canonicalize(e.path()).is_ok_and(|c| c == Path::new("/")))
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect();
                names.sort();
                if let Some(n) = names.into_iter().next() {
                    return n;
                }
            }
            return "/".into();
        }
    }
    mount
        .to_string_lossy()
        .trim_end_matches(['\\', '/'])
        .to_string()
}

/// The real volume, by `statvfs` (`GetDiskFreeSpaceExW` on Windows).
#[derive(Debug)]
pub struct Statvfs;

impl DiskSource for Statvfs {
    fn free_mb(&self, path: &Path) -> Option<u64> {
        let bytes = mason_platform::disk::available_bytes(path)?;
        Some((bytes / (1024 * 1024)) as u64)
    }
    fn volume(&self, path: &Path) -> Option<VolumeReading> {
        let v = mason_platform::disk::volume_of(path)?;
        Some(VolumeReading {
            name: volume_name(&v.mount),
            id: v.id,
            mount: v.mount,
            free_bytes: v.free_bytes,
            total_bytes: v.total_bytes,
        })
    }
}

/// A fixed, settable value, the same for every path.
#[derive(Debug)]
pub struct FakeDisk(AtomicU64, Mutex<Vec<FakeVolume>>);

/// A volume a test sets: every path under `prefix` is on it.
#[derive(Debug, Clone)]
struct FakeVolume {
    prefix: PathBuf,
    name: String,
    free_bytes: u64,
    total_bytes: u64,
}

const GB: u64 = 1024 * 1024 * 1024;

impl FakeDisk {
    pub fn new_gb(gb: u64) -> Arc<FakeDisk> {
        Arc::new(FakeDisk(AtomicU64::new(gb * 1024), Mutex::default()))
    }
    /// A volume named `name` holding every path under `prefix` (the longest prefix wins);
    /// setting the same prefix again changes its sizes.
    pub fn set_volume(&self, prefix: &Path, name: &str, free_gb: u64, total_gb: u64) {
        let mut v = self.1.lock().unwrap_or_else(|e| e.into_inner());
        v.retain(|x| x.prefix != prefix);
        v.push(FakeVolume {
            prefix: prefix.to_path_buf(),
            name: name.into(),
            free_bytes: free_gb * GB,
            total_bytes: total_gb * GB,
        });
    }
    /// A volume that cannot be read any more.
    pub fn remove_volume(&self, prefix: &Path) {
        self.1
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|x| x.prefix != prefix);
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
    fn volume(&self, path: &Path) -> Option<VolumeReading> {
        let v = self.1.lock().unwrap_or_else(|e| e.into_inner());
        let x = v
            .iter()
            .filter(|x| path.starts_with(&x.prefix))
            .max_by_key(|x| x.prefix.components().count())?;
        Some(VolumeReading {
            id: format!("fake:{}", x.prefix.display()),
            name: x.name.clone(),
            mount: x.prefix.clone(),
            free_bytes: x.free_bytes,
            total_bytes: x.total_bytes,
        })
    }
}

/// Gigabytes free (a decimal number) in a file, read each time (`MASON_DISK_FILE`: how the exit
/// script drives the floor against a real daemon). Or, for the history, one volume per line,
/// `name gb_free gb_total`: a path is on the volume whose name is the last of its components to
/// match one (else on the first line's), and `free_mb` is that volume's free space.
#[derive(Debug)]
pub struct FileDisk(pub PathBuf);

impl FileDisk {
    /// `(name, free_gb, total_gb)` per line that parses; empty for the one-number form.
    fn lines(&self) -> Vec<(String, f64, f64)> {
        let Ok(t) = std::fs::read_to_string(&self.0) else {
            return vec![];
        };
        t.lines()
            .filter_map(|l| {
                let mut f = l.split_whitespace();
                let (n, free, total) = (f.next()?, f.next()?, f.next()?);
                let (free, total) = (free.parse::<f64>().ok()?, total.parse::<f64>().ok()?);
                (free >= 0.0 && total >= 0.0).then(|| (n.to_string(), free, total))
            })
            .collect()
    }

    /// The line a path is on and the part of the path that is the volume's mount.
    fn pick(&self, path: &Path) -> Option<((String, f64, f64), PathBuf)> {
        let lines = self.lines();
        let comps: Vec<_> = path.components().collect();
        for i in (0..comps.len()).rev() {
            let c = comps[i].as_os_str().to_string_lossy();
            if let Some(l) = lines.iter().find(|l| l.0 == c) {
                let mount: PathBuf = comps[..=i].iter().collect();
                return Some((l.clone(), mount));
            }
        }
        lines.first().map(|l| (l.clone(), PathBuf::from("/")))
    }
}

impl DiskSource for FileDisk {
    fn free_mb(&self, path: &Path) -> Option<u64> {
        if let Some(((_, free, _), _)) = self.pick(path) {
            return Some((free * 1024.0) as u64);
        }
        let gb: f64 = std::fs::read_to_string(&self.0).ok()?.trim().parse().ok()?;
        (gb >= 0.0).then_some((gb * 1024.0) as u64)
    }
    fn volume(&self, path: &Path) -> Option<VolumeReading> {
        let ((name, free, total), mount) = self.pick(path)?;
        Some(VolumeReading {
            id: format!("file:{name}"),
            name,
            mount,
            free_bytes: (free * GB as f64) as u64,
            total_bytes: (total * GB as f64) as u64,
        })
    }
}

/// The nearest directory at or above `p` that exists: a `task_cache` root may not be made yet.
pub fn nearest_existing(p: &Path) -> Option<PathBuf> {
    p.ancestors().find(|a| a.exists()).map(Path::to_path_buf)
}

/// Why a path matters to MASON, for a volume's tooltip.
pub type Role = &'static str;

/// The paths whose volumes matter, each with its role: the data dir; MASON's cache data (each
/// project's `task_cache` parent, and `~/.cache/mason` if it exists); each project's path; and
/// the directory its worktrees are made in. Recomputed by the caller as projects come and go.
/// The floor, the stall prober and the history all take this, so they agree.
pub fn relevant_paths<'a>(
    cfg: &crate::config::DaemonConfig,
    projects: impl IntoIterator<Item = &'a crate::config::ProjectCfg>,
    home: &Path,
) -> Vec<(PathBuf, Role)> {
    let mut out: Vec<(PathBuf, Role)> = vec![(cfg.data_dir.clone(), "data")];
    let mason_cache = home.join(".cache").join("mason");
    if mason_cache.exists() {
        out.push((mason_cache, "cache"));
    }
    for p in projects {
        if let Some(t) = &p.task_cache {
            let e = crate::taskcache::expand(t, 0, home);
            if !e.trim().is_empty() {
                let e = PathBuf::from(e);
                out.push((e.parent().map_or(e.clone(), Path::to_path_buf), "cache"));
            }
        }
        out.push((p.path.clone(), "project"));
        out.push((cfg.worktrees_dir().join(&p.id), "worktrees"));
    }
    out
}

/// One volume of a sampling, with what lives on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeSample {
    pub volume: VolumeReading,
    pub roles: Vec<Role>,
}

/// Read each path's volume (a path not made yet on its nearest existing parent) and group the
/// paths by volume: two on one volume make one entry. Ordered by id, so the order is stable.
/// On macOS two APFS volumes of one container have different device ids and so show as two
/// entries, though they share their free space.
pub fn sample_volumes(src: &dyn DiskSource, paths: &[(PathBuf, Role)]) -> Vec<VolumeSample> {
    let mut by: BTreeMap<String, VolumeSample> = BTreeMap::new();
    for (p, role) in paths {
        let Some(v) = nearest_existing(p).and_then(|e| src.volume(&e)) else {
            continue;
        };
        let e = by.entry(v.id.clone()).or_insert_with(|| VolumeSample {
            volume: v,
            roles: vec![],
        });
        if !e.roles.contains(role) {
            e.roles.push(*role);
        }
    }
    by.into_values().collect()
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

    fn paths(p: &[(&Path, Role)]) -> Vec<(PathBuf, Role)> {
        p.iter().map(|(a, r)| (a.to_path_buf(), *r)).collect()
    }

    #[test]
    fn paths_on_one_volume_make_one_entry_and_two_volumes_two() {
        let t = tempfile::tempdir().unwrap();
        let (a, b) = (t.path().join("a"), t.path().join("b"));
        std::fs::create_dir_all(a.join("x")).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let d = FakeDisk::new_gb(1);
        d.set_volume(&a, "alpha", 10, 100);
        let got = sample_volumes(
            &*d,
            &paths(&[
                (&a, "data"),
                (&a.join("x"), "project"),
                (&a.join("y/z"), "cache"),
            ]),
        );
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].volume.name, "alpha");
        assert_eq!(got[0].roles, vec!["data", "project", "cache"]);
        d.set_volume(&b, "beta", 5, 50);
        let got = sample_volumes(&*d, &paths(&[(&b, "worktrees"), (&a, "data")]));
        assert_eq!(got.len(), 2);
        // A path no volume holds is not listed.
        assert!(sample_volumes(&*d, &paths(&[(t.path(), "data")])).is_empty());
    }

    #[test]
    fn a_path_not_made_yet_is_read_on_its_nearest_parent_volume() {
        let t = tempfile::tempdir().unwrap();
        let d = FakeDisk::new_gb(1);
        d.set_volume(t.path(), "v", 1, 2);
        let got = sample_volumes(&*d, &paths(&[(&t.path().join("no/such/dir"), "worktrees")]));
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].roles, vec!["worktrees"]);
    }

    #[test]
    fn a_file_names_volumes_one_per_line() {
        let t = tempfile::tempdir().unwrap();
        let f = t.path().join("disk");
        std::fs::write(&f, "one 10 100\ntwo 2.5 50\n").unwrap();
        let d = FileDisk(f);
        let v = d.volume(&t.path().join("two/proj")).unwrap();
        assert_eq!((v.name.as_str(), v.free_bytes), ("two", 5 * GB / 2));
        assert_eq!(v.mount, t.path().join("two"));
        // Off any named volume: the first line's.
        assert_eq!(d.volume(t.path()).unwrap().name, "one");
        assert_eq!(d.free_mb(&t.path().join("two")), Some(2560));
    }

    #[test]
    fn the_real_volume_has_a_name_and_sizes() {
        let v = Statvfs.volume(&std::env::temp_dir()).unwrap();
        assert!(!v.name.is_empty() && v.total_bytes >= v.free_bytes && v.total_bytes > 0);
    }

    #[test]
    fn a_mount_is_named_by_its_last_component() {
        assert_eq!(volume_name(Path::new("/Volumes/sp")), "sp");
    }

    #[cfg(unix)]
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
