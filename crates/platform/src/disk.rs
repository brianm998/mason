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
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let w = wide(path);
    let mut free_to_caller = 0u64;
    // SAFETY: `w` is NUL-terminated and outlives the call; the out pointer is a
    // live u64 and the two we do not want are allowed to be null.
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            w.as_ptr(),
            &mut free_to_caller,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    (ok != 0).then_some(u128::from(free_to_caller))
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

    #[test]
    fn a_missing_path_reads_as_none() {
        assert_eq!(available_bytes(Path::new("/no/such/volume/at/all")), None);
    }
}
