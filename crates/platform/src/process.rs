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

/// One row of the process table: enough to find a tree and to tell, later,
/// whether a pid still names the same process.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcEntry {
    pub pid: u32,
    pub ppid: u32,
    pub pgid: u32,
    /// The process start time as `ps` prints it (`lstart`). A reused pid has a
    /// different one.
    pub start: String,
}

/// Parse `ps -axo pid=,ppid=,pgid=,lstart=` output. `lstart` is five words
/// (`Fri Nov 13 10:00:00 2026`); it is kept as one string. Unreadable lines are skipped.
pub fn parse_table(text: &str) -> Vec<ProcEntry> {
    text.lines()
        .filter_map(|line| {
            let mut w = line.split_whitespace();
            let pid = w.next()?.parse().ok()?;
            let ppid = w.next()?.parse().ok()?;
            let pgid = w.next()?.parse().ok()?;
            let start = w.collect::<Vec<_>>().join(" ");
            (!start.is_empty()).then_some(ProcEntry {
                pid,
                ppid,
                pgid,
                start,
            })
        })
        .collect()
}

/// The process table now. Empty if it cannot be read (and on Windows, which has
/// no `ps`: stopping a step there keeps its current behaviour).
pub fn read_table() -> Vec<ProcEntry> {
    #[cfg(unix)]
    {
        Command::new("/bin/ps")
            .args(["-axo", "pid=,ppid=,pgid=,lstart="])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| parse_table(&String::from_utf8_lossy(&o.stdout)))
            .unwrap_or_default()
    }
    #[cfg(windows)]
    {
        Vec::new()
    }
}

/// Everything under `root` by parent pid, plus the members of the process group
/// of `root` and of every descendant that leads one. Walks to a fixed point, so
/// a grandchild that moved to its own group is found by its parent, and that
/// group's other members (reparented to init already) are found by the group.
/// `root` itself is included when present. Pid 0, pid 1 and anything in
/// `exclude` are never returned, and the walk does not pass through them.
pub fn descendants(table: &[ProcEntry], root: u32, exclude: &[u32]) -> Vec<ProcEntry> {
    use std::collections::HashSet;
    let skip = |pid: u32| pid <= 1 || exclude.contains(&pid);
    if skip(root) || !table.iter().any(|p| p.pid == root) {
        return Vec::new();
    }
    let mut seen: HashSet<u32> = HashSet::from([root]);
    let mut groups: HashSet<u32> = HashSet::new();
    loop {
        let before = seen.len();
        for e in table {
            if seen.contains(&e.pid) && e.pgid == e.pid {
                groups.insert(e.pid);
            }
        }
        for e in table {
            if !skip(e.pid) && (seen.contains(&e.ppid) || groups.contains(&e.pgid)) {
                seen.insert(e.pid);
            }
        }
        if seen.len() == before {
            break;
        }
    }
    table
        .iter()
        .filter(|e| seen.contains(&e.pid))
        .cloned()
        .collect()
}

/// The entries of `snapshot` that are still the same process in `now`: same pid
/// and same start time. A pid that was reused is not returned.
pub fn still_running(snapshot: &[ProcEntry], now: &[ProcEntry]) -> Vec<ProcEntry> {
    snapshot
        .iter()
        .filter(|s| now.iter().any(|n| n.pid == s.pid && n.start == s.start))
        .cloned()
        .collect()
}

/// A tree of processes noted before it was signalled, so it can still be found
/// after its parent dies and the rest are reparented to init.
#[derive(Clone, Debug, Default)]
pub struct TreeSnapshot {
    entries: Vec<ProcEntry>,
    exclude: Vec<u32>,
    root: u32,
}

impl TreeSnapshot {
    /// Note `root` and all its descendants now. `exclude` pids (and this
    /// process) are never part of it.
    pub fn take(root: u32, exclude: &[u32]) -> Self {
        let mut exclude = exclude.to_vec();
        exclude.push(std::process::id());
        let entries = descendants(&read_table(), root, &exclude);
        TreeSnapshot {
            entries,
            exclude,
            root,
        }
    }

    /// Add whatever has appeared under the root since the last look.
    pub fn refresh(&mut self) {
        let table = read_table();
        for e in descendants(&table, self.root, &self.exclude) {
            if !self
                .entries
                .iter()
                .any(|o| o.pid == e.pid && o.start == e.start)
            {
                self.entries.push(e);
            }
        }
    }

    /// Signal each member that is still the process it was (same start time),
    /// and the process group of each member that leads one. Returns how many
    /// were still running.
    pub fn signal(&self, sig: Signal) -> usize {
        let live = still_running(&self.entries, &read_table());
        let own = own_group();
        for e in &live {
            if e.pgid == e.pid && e.pid != own && e.pid > 1 {
                let _ = signal_group(e.pid, sig);
            }
            let _ = signal_process(e.pid, sig);
        }
        live.len()
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

    fn e(pid: u32, ppid: u32, pgid: u32) -> ProcEntry {
        ProcEntry {
            pid,
            ppid,
            pgid,
            start: format!("start{pid}"),
        }
    }

    #[test]
    fn the_table_parses_with_multiword_start_times() {
        let t = parse_table(
            "  12   1  12 Fri Nov 13 10:00:00 2026\nbad line\n 13 12 12 Fri Nov 13 10:00:01 2026\n",
        );
        assert_eq!(t.len(), 2);
        assert_eq!(t[1].ppid, 12);
        assert_eq!(t[1].start, "Fri Nov 13 10:00:01 2026");
    }

    #[test]
    fn descendants_include_a_grandchild_in_its_own_group_but_not_strangers() {
        // 100 is the step command (group leader); 101 its child; 102 a grandchild
        // that called setsid (own group) and 103 a member of that group already
        // reparented to init; 200 is unrelated; 300 is in 100's group under init;
        // 50 is the step host, excluded.
        let table = vec![
            e(1, 0, 1),
            e(50, 1, 50),
            e(100, 50, 100),
            e(101, 100, 100),
            e(102, 101, 102),
            e(103, 1, 102),
            e(200, 1, 200),
            e(201, 200, 200),
            e(300, 1, 100),
        ];
        let got: Vec<u32> = descendants(&table, 100, &[50])
            .iter()
            .map(|p| p.pid)
            .collect();
        assert_eq!(got, vec![100, 101, 102, 103, 300]);
    }

    #[test]
    fn pid_one_and_excluded_pids_are_never_returned() {
        let table = vec![
            e(1, 0, 1),
            e(100, 1, 100),
            e(101, 100, 100),
            e(102, 101, 100),
        ];
        let got: Vec<u32> = descendants(&table, 100, &[101])
            .iter()
            .map(|p| p.pid)
            .collect();
        assert!(!got.contains(&1) && !got.contains(&101));
        assert_eq!(got, vec![100, 102]);
        assert!(descendants(&table, 1, &[]).is_empty());
        assert!(descendants(&table, 999, &[]).is_empty());
    }

    #[test]
    fn a_reused_pid_with_a_different_start_is_not_still_running() {
        let snap = vec![e(100, 1, 100), e(101, 100, 100)];
        let mut now = vec![e(100, 1, 100), e(101, 1, 101)];
        now[1].start = "later".into();
        let live = still_running(&snap, &now);
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].pid, 100);
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
