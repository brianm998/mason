// SPDX-License-Identifier: GPL-3.0-or-later

//! The few places where MASON touches the operating system in a way that
//! differs between Unix and Windows: process groups and signals, Unix sockets,
//! file permission bits, positioned reads, and replacing the running image.
//!
//! Every crate that needs one of these calls it here, so the `cfg(unix)` /
//! `cfg(windows)` split lives in one place. On Unix each function is a thin
//! call to the same `std` or `nix` API the callers used before, so behaviour
//! there is unchanged. On Windows a piece that belongs to a later port task
//! returns an [`unsupported`] error naming that task; none is a silent no-op.

pub mod disk;
pub mod fs;
pub mod host;
pub mod net;
pub mod process;
pub mod shell;

use std::io;

/// The error a Windows stub returns for work a later port task will do.
/// `what` is the operation, `task` the area of the port that will provide it.
pub fn unsupported(what: &str, task: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        format!("{what} is not available on Windows yet (Windows port: {task})"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unsupported_error_names_the_operation_and_the_task() {
        let e = unsupported("killing a process group", "process trees");
        assert_eq!(e.kind(), io::ErrorKind::Unsupported);
        let text = e.to_string();
        assert!(text.contains("killing a process group"), "{text}");
        assert!(text.contains("process trees"), "{text}");
    }
}
