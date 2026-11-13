// SPDX-License-Identifier: GPL-3.0-or-later

//! Process groups, signals, liveness and replacing the running image.
//!
//! On Unix these call `nix` / `std::os::unix` exactly as the callers did
//! before. On Windows there are no process groups or signals: the pieces that
//! need a job object or `TerminateProcess` return `Unsupported` naming the
//! process-tree port task, and `is_alive` is answered for real.

use std::io;
use std::process::{Command, ExitStatus};

/// The signals MASON sends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signal {
    /// Ask politely (SIGTERM).
    Term,
    /// Cannot be caught (SIGKILL).
    Kill,
    /// Resume a stopped process (SIGCONT).
    Cont,
}

#[cfg(unix)]
fn nix_signal(s: Signal) -> nix::sys::signal::Signal {
    match s {
        Signal::Term => nix::sys::signal::Signal::SIGTERM,
        Signal::Kill => nix::sys::signal::Signal::SIGKILL,
        Signal::Cont => nix::sys::signal::Signal::SIGCONT,
    }
}

/// Make the child lead its own process group, so the whole tree can be
/// signalled at once. (`process_group(0)` on Unix. Windows has no groups; the
/// process-tree port task will use a job object here.)
pub fn group_leader(cmd: &mut Command) -> &mut Command {
    #[cfg(unix)]
    {
        std::os::unix::process::CommandExt::process_group(cmd, 0)
    }
    #[cfg(windows)]
    {
        cmd
    }
}

/// Signal every process in the group led by `pgid` (`killpg`).
pub fn signal_group(pgid: u32, sig: Signal) -> io::Result<()> {
    #[cfg(unix)]
    {
        nix::sys::signal::killpg(nix::unistd::Pid::from_raw(pgid as i32), nix_signal(sig))
            .map_err(io::Error::from)
    }
    #[cfg(windows)]
    {
        let _ = (pgid, sig);
        Err(crate::unsupported(
            "signalling a process group",
            "process trees",
        ))
    }
}

/// Signal one process (`kill`).
pub fn signal_process(pid: u32, sig: Signal) -> io::Result<()> {
    #[cfg(unix)]
    {
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid as i32), nix_signal(sig))
            .map_err(io::Error::from)
    }
    #[cfg(windows)]
    {
        let _ = (pid, sig);
        Err(crate::unsupported("signalling a process", "process trees"))
    }
}

/// Whether a process with this pid exists (`kill(pid, 0)`; on Windows an open
/// process handle whose exit code is still "active").
pub fn is_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid as i32), None).is_ok()
    }
    #[cfg(windows)]
    {
        windows_alive(pid)
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn windows_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    /// `STILL_ACTIVE`: the exit code of a process that has not exited.
    const STILL_ACTIVE: u32 = 259;
    // SAFETY: OpenProcess takes plain values; the handle it returns is checked
    // for null, used for one query and closed.
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return false;
        }
        let mut code = 0u32;
        let ok = GetExitCodeProcess(h, &mut code);
        CloseHandle(h);
        ok != 0 && code == STILL_ACTIVE
    }
}

/// Like [`is_alive`], but a process we are not allowed to signal still counts
/// as existing (`EPERM`).
pub fn exists(pid: u32) -> bool {
    #[cfg(unix)]
    {
        use nix::errno::Errno;
        !matches!(
            nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid as i32), None),
            Err(Errno::ESRCH)
        )
    }
    #[cfg(windows)]
    {
        is_alive(pid)
    }
}

/// The process group of this process (`getpgrp`). Windows has none, so this is
/// the pid, which keeps "never signal our own group" checks meaningful.
pub fn own_group() -> u32 {
    #[cfg(unix)]
    {
        nix::unistd::getpgrp().as_raw() as u32
    }
    #[cfg(windows)]
    {
        std::process::id()
    }
}

/// Start a new session, detaching from the controlling terminal (`setsid`).
/// Fails harmlessly if already a session leader.
pub fn detach_session() -> io::Result<()> {
    #[cfg(unix)]
    {
        nix::unistd::setsid().map(|_| ()).map_err(io::Error::from)
    }
    #[cfg(windows)]
    {
        Err(crate::unsupported("detaching a session", "process trees"))
    }
}

/// The signal that ended a child, if one did. Windows has no signals.
pub fn exit_signal(status: &ExitStatus) -> Option<i32> {
    #[cfg(unix)]
    {
        std::os::unix::process::ExitStatusExt::signal(status)
    }
    #[cfg(windows)]
    {
        let _ = status;
        None
    }
}

/// Replace this process with `cmd`, keeping the pid (`exec`). Only returns, with
/// the error, if that fails. Windows cannot replace an image in place; the
/// takeover port task will do an equivalent hand-over.
pub fn replace_process(cmd: &mut Command) -> io::Error {
    #[cfg(unix)]
    {
        std::os::unix::process::CommandExt::exec(cmd)
    }
    #[cfg(windows)]
    {
        let _ = cmd;
        crate::unsupported("replacing the running daemon in place", "daemon takeover")
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn our_own_process_is_alive_and_a_group_of_our_own_is_signalled() {
        assert!(is_alive(std::process::id()));
        let mut c = Command::new("sleep");
        group_leader(&mut c).arg("30");
        let mut child = c.spawn().unwrap();
        let pid = child.id();
        assert!(is_alive(pid));
        signal_group(pid, Signal::Kill).unwrap();
        let st = child.wait().unwrap();
        assert_eq!(exit_signal(&st), Some(9));
        assert!(!is_alive(pid), "reaped, so gone");
    }

    #[test]
    fn replacing_a_missing_program_returns_the_error() {
        let e = replace_process(&mut Command::new("/no/such/program/anywhere"));
        assert_eq!(e.kind(), io::ErrorKind::NotFound);
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;

    #[test]
    fn groups_are_unsupported_and_the_error_names_the_task() {
        let e = signal_group(1, Signal::Kill).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::Unsupported);
        assert!(e.to_string().contains("process trees"));
        assert!(is_alive(std::process::id()));
    }
}
