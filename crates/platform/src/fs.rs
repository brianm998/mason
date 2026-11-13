// SPDX-License-Identifier: GPL-3.0-or-later

//! File helpers whose `std` form exists only on Unix: positioned reads and the
//! permission bits.

use std::fs::{File, Metadata, OpenOptions};
use std::io;
use std::path::Path;

/// Read at `offset` without moving the file position; like `FileExt::read_at`.
pub fn read_at(f: &File, buf: &mut [u8], offset: u64) -> io::Result<usize> {
    #[cfg(unix)]
    {
        std::os::unix::fs::FileExt::read_at(f, buf, offset)
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::FileExt::seek_read(f, buf, offset)
    }
}

/// Fill `buf` from `offset`, or fail with `UnexpectedEof`; like
/// `FileExt::read_exact_at`.
pub fn read_exact_at(f: &File, mut buf: &mut [u8], mut offset: u64) -> io::Result<()> {
    while !buf.is_empty() {
        match read_at(f, buf, offset) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "failed to fill whole buffer",
                ))
            }
            Ok(n) => {
                buf = &mut buf[n..];
                offset += n as u64;
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Create files private to their owner: mode `0o600` on Unix. Windows has no
/// permission bits; a new file there carries the access list of its folder,
/// which for the per-user data directory is already the user's alone.
pub fn owner_only(opts: &mut OpenOptions) -> &mut OpenOptions {
    #[cfg(unix)]
    {
        std::os::unix::fs::OpenOptionsExt::mode(opts, 0o600)
    }
    #[cfg(windows)]
    {
        opts
    }
}

/// Whether the file's execute bit is set for anyone. Windows has no such bit
/// (what runs is decided by the extension), so every file counts as runnable
/// there only if its extension says so.
pub fn is_executable(meta: &Metadata, path: &Path) -> bool {
    #[cfg(unix)]
    {
        let _ = path;
        std::os::unix::fs::PermissionsExt::mode(&meta.permissions()) & 0o111 != 0
    }
    #[cfg(windows)]
    {
        meta.is_file() && is_runnable_name(path)
    }
}

/// Windows decides what runs by extension.
#[cfg(any(windows, test))]
fn is_runnable_name(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        ["exe", "bat", "cmd", "com", "ps1"].contains(&e.to_ascii_lowercase().as_str())
    })
}

/// Set a file to `0o755` (runnable) or `0o644` on Unix; a no-op on Windows,
/// where the extension already decides it.
pub fn set_executable(path: &Path, exec: bool) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            path,
            std::fs::Permissions::from_mode(if exec { 0o755 } else { 0o644 }),
        )
    }
    #[cfg(windows)]
    {
        let _ = (path, exec);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn positioned_reads_do_not_move_the_file_and_stop_at_the_end() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f");
        std::fs::File::create(&p)
            .unwrap()
            .write_all(b"hello world")
            .unwrap();
        let f = File::open(&p).unwrap();
        let mut b = [0u8; 5];
        read_exact_at(&f, &mut b, 6).unwrap();
        assert_eq!(&b, b"world");
        let mut big = [0u8; 8];
        let e = read_exact_at(&f, &mut big, 6).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::UnexpectedEof);
        assert_eq!(read_at(&f, &mut big, 11).unwrap(), 0);
    }

    #[test]
    fn windows_runs_files_by_extension() {
        assert!(is_runnable_name(Path::new("a/tool.EXE")));
        assert!(is_runnable_name(Path::new("run.cmd")));
        assert!(!is_runnable_name(Path::new("notes.txt")));
        assert!(!is_runnable_name(Path::new("tool")));
    }

    #[cfg(unix)]
    #[test]
    fn the_execute_bit_round_trips_on_unix() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f");
        std::fs::write(&p, b"x").unwrap();
        set_executable(&p, true).unwrap();
        assert!(is_executable(&std::fs::metadata(&p).unwrap(), &p));
        set_executable(&p, false).unwrap();
        assert!(!is_executable(&std::fs::metadata(&p).unwrap(), &p));
    }
}
