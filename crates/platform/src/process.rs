// SPDX-License-Identifier: GPL-3.0-or-later

//! Process groups, signals, liveness and replacing the running image.
//!
//! On Unix these call `nix` / `std::os::unix` exactly as the callers did
//! before. On Windows there are no process groups or signals: the pieces that
//! need a job object or `TerminateProcess` use those: [`spawn_group`] puts the
//! child in a named Job Object and [`signal_group`] terminates that job.

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

/// Make `cmd` start detached from our console and process group, so closing
/// the terminal that started us, or our exit, does not reach it. Unix needs
/// nothing here (the step host calls `setsid` itself). Windows: no console
/// window of its own (children inherit the hidden one) and its own group.
pub fn detached(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        /// `CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP`.
        const FLAGS: u32 = 0x0800_0000 | 0x0000_0200;
        std::os::windows::process::CommandExt::creation_flags(cmd, FLAGS)
    }
    #[cfg(unix)]
    {
        cmd
    }
}

/// An opaque string that, with the pid, identifies one process even after the
/// pid is reused: the creation time. Windows only; Unix callers use `ps`.
pub fn start_time(pid: u32) -> Option<String> {
    #[cfg(windows)]
    {
        windows_job::start_time(pid)
    }
    #[cfg(unix)]
    {
        let _ = pid;
        None
    }
}

/// The path of a process's executable. Windows only; Unix callers use `ps`.
pub fn image_path(pid: u32) -> Option<String> {
    #[cfg(windows)]
    {
        windows_job::image_path(pid)
    }
    #[cfg(unix)]
    {
        let _ = pid;
        None
    }
}

/// Start `cmd` as the leader of a new process tree that [`signal_group`] can
/// stop as a whole. Unix: `process_group(0)` then spawn. Windows: spawn, then
/// put the child in a Job Object named after its pid. The job does not kill on
/// close, so a step host (and its tree) outlives the daemon that started it,
/// as on Unix, and any process can find the job by pid to terminate it later.
///
/// On Windows a grandchild started in the instant between the spawn and the
/// job assignment is not in the job. If the assignment fails the child still
/// runs, and `signal_group` then terminates just that process.
pub fn spawn_group(cmd: &mut Command) -> io::Result<std::process::Child> {
    group_leader(cmd);
    let child = cmd.spawn()?;
    #[cfg(windows)]
    windows_job::adopt(&child);
    Ok(child)
}

/// Signal every process in the group led by `pgid` (`killpg`). On Windows the
/// group is the job [`spawn_group`] made for `pgid`; there are no signals, so
/// `Term` and `Kill` both terminate it, and `Cont` is `Unsupported`.
pub fn signal_group(pgid: u32, sig: Signal) -> io::Result<()> {
    #[cfg(unix)]
    {
        nix::sys::signal::killpg(nix::unistd::Pid::from_raw(pgid as i32), nix_signal(sig))
            .map_err(io::Error::from)
    }
    #[cfg(windows)]
    {
        if sig == Signal::Cont {
            return Err(crate::unsupported(
                "resuming a stopped process",
                "process trees",
            ));
        }
        windows_job::terminate_group(pgid)
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
        if sig == Signal::Cont {
            return Err(crate::unsupported(
                "resuming a stopped process",
                "process trees",
            ));
        }
        windows_job::terminate_process(pid)
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
mod windows_job {
    #![allow(unsafe_code)]
    use std::io;
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, OpenJobObjectW, TerminateJobObject,
    };
    use windows_sys::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};

    /// `JOB_OBJECT_TERMINATE`, which windows-sys does not define.
    const JOB_OBJECT_TERMINATE: u32 = 0x0008;
    /// The exit code of a process we terminated (like 128 + SIGKILL).
    const KILLED: u32 = 137;

    fn job_name(pid: u32) -> Vec<u16> {
        format!("mason-job-{pid}\0").encode_utf16().collect()
    }

    /// Put `child` in the job named for its pid. Best effort: see `spawn_group`.
    pub fn adopt(child: &Child) {
        let name = job_name(child.id());
        // SAFETY: `name` is NUL-terminated and outlives the calls; the handles
        // are checked, and the job handle is closed. Closing it does not end
        // the job while the child lives (a job lasts until its last handle is
        // closed AND its processes have exited) and, with kill-on-close not
        // set, does not kill the child.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), name.as_ptr());
            if job.is_null() {
                return;
            }
            let _ = AssignProcessToJobObject(job, child.as_raw_handle() as HANDLE);
            CloseHandle(job);
        }
    }

    pub fn start_time(pid: u32) -> Option<String> {
        use windows_sys::Win32::Foundation::FILETIME;
        use windows_sys::Win32::System::Threading::{
            GetProcessTimes, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        // SAFETY: plain values in; FILETIMEs are written by the call; the
        // handle is checked and closed.
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h.is_null() {
                return None;
            }
            let z = FILETIME {
                dwLowDateTime: 0,
                dwHighDateTime: 0,
            };
            let (mut created, mut a, mut b, mut c) = (z, z, z, z);
            let ok = GetProcessTimes(h, &mut created, &mut a, &mut b, &mut c);
            CloseHandle(h);
            (ok != 0).then(|| {
                let t =
                    (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime);
                t.to_string()
            })
        }
    }

    pub fn image_path(pid: u32) -> Option<String> {
        use windows_sys::Win32::System::Threading::{
            QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        // SAFETY: the buffer and its length are ours; the handle is checked
        // and closed.
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h.is_null() {
                return None;
            }
            let mut buf = [0u16; 1024];
            let mut len = buf.len() as u32;
            let ok = QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len);
            CloseHandle(h);
            (ok != 0).then(|| String::from_utf16_lossy(&buf[..len as usize]))
        }
    }

    pub fn terminate_group(pid: u32) -> io::Result<()> {
        let name = job_name(pid);
        // SAFETY: as in `adopt`.
        unsafe {
            let job = OpenJobObjectW(JOB_OBJECT_TERMINATE, 0, name.as_ptr());
            if job.is_null() {
                // No job: it was never made, or everything in it has exited.
                return terminate_process(pid);
            }
            let ok = TerminateJobObject(job, KILLED);
            let err = io::Error::last_os_error();
            CloseHandle(job);
            if ok == 0 {
                return Err(err);
            }
        }
        Ok(())
    }

    pub fn terminate_process(pid: u32) -> io::Result<()> {
        // SAFETY: plain values in; the handle is checked and closed.
        unsafe {
            let h = OpenProcess(PROCESS_TERMINATE, 0, pid);
            if h.is_null() {
                return Err(io::Error::last_os_error());
            }
            let ok = TerminateProcess(h, KILLED);
            let err = io::Error::last_os_error();
            CloseHandle(h);
            if ok == 0 {
                return Err(err);
            }
        }
        Ok(())
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
    fn killing_the_group_kills_the_tree() {
        // cmd starts a ping child; killing the job ends the whole tree.
        let mut c = Command::new("cmd");
        c.args(["/C", "ping -n 60 127.0.0.1 >NUL"]);
        let mut child = spawn_group(&mut c).unwrap();
        let pid = child.id();
        assert!(is_alive(pid));
        signal_group(pid, Signal::Kill).unwrap();
        child.wait().unwrap();
        assert!(!is_alive(pid));
    }

    #[test]
    fn cont_is_unsupported_and_names_the_task() {
        let e = signal_group(1, Signal::Cont).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::Unsupported);
        assert!(e.to_string().contains("process trees"));
        assert!(is_alive(std::process::id()));
    }
}
