// SPDX-License-Identifier: GPL-3.0-or-later

//! Free space on the volume holding a path.

use std::path::Path;

/// Bytes available to this (non-root) process on the volume holding `path`, or
/// `None` if they cannot be read. `statvfs` on Unix, `GetDiskFreeSpaceExW` on
/// Windows.
pub fn available_bytes(path: &Path) -> Option<u128> {
    #[cfg(unix)]
    {
        let s = nix::sys::statvfs::statvfs(path).ok()?;
        Some((s.blocks_available() as u128) * (s.fragment_size() as u128))
    }
    #[cfg(windows)]
    {
        windows_available(path)
    }
}

/// The volume holding a path: who it is, where it is mounted and how full.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeInfo {
    /// Stable for the volume: the device id on Unix, the drive on Windows.
    pub id: String,
    /// The directory the volume is mounted at (`C:\` on Windows).
    pub mount: std::path::PathBuf,
    pub free_bytes: u64,
    pub total_bytes: u64,
}

/// The volume holding `path` (which must exist), or `None` if it cannot be read.
pub fn volume_of(path: &Path) -> Option<VolumeInfo> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let dev = std::fs::metadata(path).ok()?.dev();
        let s = nix::sys::statvfs::statvfs(path).ok()?;
        let frag = s.fragment_size() as u64;
        // The mount point is the highest ancestor still on the same device.
        let abs = std::fs::canonicalize(path).ok()?;
        let mut mount = abs.clone();
        for a in abs.ancestors().skip(1) {
            match std::fs::metadata(a) {
                Ok(m) if m.dev() == dev => mount = a.to_path_buf(),
                _ => break,
            }
        }
        Some(VolumeInfo {
            id: dev.to_string(),
            mount,
            free_bytes: (s.blocks_available() as u64).saturating_mul(frag),
            total_bytes: (s.blocks() as u64).saturating_mul(frag),
        })
    }
    #[cfg(windows)]
    {
        let (free, total) = windows_space(path)?;
        let prefix = path.components().find_map(|c| match c {
            std::path::Component::Prefix(p) => Some(p.as_os_str().to_string_lossy().into_owned()),
            _ => None,
        })?;
        Some(VolumeInfo {
            id: prefix.to_uppercase(),
            mount: std::path::PathBuf::from(format!("{prefix}\\")),
            free_bytes: free,
            total_bytes: total,
        })
    }
}

/// The wide, NUL-terminated form of a path, as Win32 takes it.
#[cfg(any(windows, test))]
fn wide(path: &Path) -> Vec<u16> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        path.as_os_str().encode_wide().chain([0]).collect()
    }
    #[cfg(not(windows))]
    {
        path.to_string_lossy().encode_utf16().chain([0]).collect()
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn windows_available(path: &Path) -> Option<u128> {
    windows_space(path).map(|(f, _)| u128::from(f))
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn windows_space(path: &Path) -> Option<(u64, u64)> {
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let w = wide(path);
    let mut free_to_caller = 0u64;
    let mut total = 0u64;
    // SAFETY: `w` is NUL-terminated and outlives the call; the out pointers are
    // live u64s and the one we do not want is allowed to be null.
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            w.as_ptr(),
            &mut free_to_caller,
            &mut total,
            std::ptr::null_mut(),
        )
    };
    (ok != 0).then_some((free_to_caller, total))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wide_path_ends_with_one_nul() {
        let w = wide(Path::new("C:\\a"));
        assert_eq!(w.last(), Some(&0));
        assert_eq!(w.iter().filter(|c| **c == 0).count(), 1);
        assert_eq!(w.len(), 5);
    }

    #[test]
    fn the_temp_dir_has_some_space_free() {
        let dir = tempfile::tempdir().unwrap();
        assert!(available_bytes(dir.path()).is_some_and(|b| b > 0));
    }

    #[cfg(unix)]
    #[test]
    fn a_volume_has_a_mount_and_sizes() {
        let dir = tempfile::tempdir().unwrap();
        let v = volume_of(dir.path()).unwrap();
        assert!(v.total_bytes >= v.free_bytes && v.total_bytes > 0);
        assert!(dir.path().canonicalize().unwrap().starts_with(&v.mount));
        assert_eq!(volume_of(&dir.path().join("x/y")), None);
    }

    #[test]
    fn a_missing_path_reads_as_none() {
        assert_eq!(available_bytes(Path::new("/no/such/volume/at/all")), None);
    }
}
