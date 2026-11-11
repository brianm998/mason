// SPDX-License-Identifier: GPL-3.0-or-later

//! The registry: one directory per host under a root. The daemon scans it after
//! a restart to find hosts that kept running, and reads journals and exit
//! records from disk for hosts that have already gone.
//!
//! ```text
//! <root>/<id>/host.kv     written once the host is up (pid, child pid, socket)
//! <root>/<id>/journal     every byte the child wrote (stdout and stderr merged)
//! <root>/<id>/exit.kv     written once, when the child has exited
//! <root>/<id>/hb          last heartbeat time in ms, rewritten atomically
//! <root>/<id>/sock        unix socket while the host is running
//! ```

use std::collections::HashMap;
use std::fs;
use std::io;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// Atomically replace `path` with `key=value` lines.
pub fn write_kv(path: &Path, pairs: &[(&str, String)]) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    let body: String = pairs.iter().map(|(k, v)| format!("{k}={v}\n")).collect();
    fs::write(&tmp, body)?;
    fs::rename(&tmp, path)
}

pub fn read_kv(path: &Path) -> io::Result<HashMap<String, String>> {
    let text = fs::read_to_string(path)?;
    Ok(text
        .lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect())
}

fn bad(m: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, m.to_string())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostInfo {
    pub id: String,
    pub dir: PathBuf,
    pub host_pid: u32,
    pub child_pid: u32,
    pub sock: PathBuf,
    pub started_ms: u64,
    pub heartbeat_ms: u64,
}

impl HostInfo {
    pub fn read(dir: &Path) -> io::Result<HostInfo> {
        let kv = read_kv(&dir.join("host.kv"))?;
        let num = |k: &str| -> io::Result<u64> {
            kv.get(k).and_then(|v| v.parse().ok()).ok_or_else(|| bad(k))
        };
        Ok(HostInfo {
            id: kv.get("id").cloned().ok_or_else(|| bad("id"))?,
            dir: dir.to_path_buf(),
            host_pid: num("host_pid")? as u32,
            child_pid: num("child_pid")? as u32,
            sock: PathBuf::from(kv.get("sock").ok_or_else(|| bad("sock"))?),
            started_ms: num("started_ms")?,
            heartbeat_ms: num("heartbeat_ms")?,
        })
    }
}

/// What is left of `limit` for a host that started at `started_ms`, given the time `now`.
pub fn remaining_at(limit: std::time::Duration, started_ms: u64, now: u64) -> std::time::Duration {
    limit.saturating_sub(std::time::Duration::from_millis(
        now.saturating_sub(started_ms),
    ))
}

/// What is left of `limit` for the host in `dir`, counted from when that host started
/// (`started_ms` in its `host.kv`), so a restarted daemon does not begin the count again.
/// A host whose record cannot be read gets the whole limit.
pub fn remaining_limit(limit: std::time::Duration, dir: &Path) -> std::time::Duration {
    match HostInfo::read(dir) {
        Ok(i) => remaining_at(limit, i.started_ms, now_ms()),
        Err(_) => limit,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExitRecord {
    /// -1 when killed by a signal.
    pub code: i32,
    /// 0 when the child exited normally.
    pub signal: i32,
    pub output_len: u64,
    pub ended_ms: u64,
}

impl ExitRecord {
    pub fn read(dir: &Path) -> io::Result<ExitRecord> {
        let kv = read_kv(&dir.join("exit.kv"))?;
        let n = |k: &str| {
            kv.get(k)
                .and_then(|v| v.parse::<i64>().ok())
                .ok_or_else(|| bad(k))
        };
        Ok(ExitRecord {
            code: n("code")? as i32,
            signal: n("signal")? as i32,
            output_len: n("output_len")? as u64,
            ended_ms: n("ended_ms")? as u64,
        })
    }

    pub fn write(&self, dir: &Path) -> io::Result<()> {
        write_kv(
            &dir.join("exit.kv"),
            &[
                ("code", self.code.to_string()),
                ("signal", self.signal.to_string()),
                ("output_len", self.output_len.to_string()),
                ("ended_ms", self.ended_ms.to_string()),
            ],
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostState {
    /// The host process is alive and the child has not been reported exited.
    Running,
    /// The child exited. The record and journal are on disk; the host may or
    /// may not still be lingering for a controller to collect them.
    Exited(ExitRecord),
    /// The host is gone and left no exit record (killed, or the machine
    /// restarted). The journal holds output up to that point.
    Lost,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostStatus {
    pub info: HostInfo,
    pub state: HostState,
    pub last_heartbeat_ms: Option<u64>,
    pub journal_len: u64,
}

pub fn pid_alive(pid: u32) -> bool {
    use nix::sys::signal::kill;
    use nix::unistd::Pid;
    kill(Pid::from_raw(pid as i32), None).is_ok()
}

pub fn status_of(dir: &Path) -> io::Result<HostStatus> {
    let info = HostInfo::read(dir)?;
    let state = match ExitRecord::read(dir) {
        Ok(rec) => HostState::Exited(rec),
        Err(_) if pid_alive(info.host_pid) => HostState::Running,
        Err(_) => HostState::Lost,
    };
    let last_heartbeat_ms = fs::read_to_string(dir.join("hb"))
        .ok()
        .and_then(|s| s.trim().parse().ok());
    let journal_len = fs::metadata(dir.join("journal")).map_or(0, |m| m.len());
    Ok(HostStatus {
        info,
        state,
        last_heartbeat_ms,
        journal_len,
    })
}

pub struct Registry {
    pub root: PathBuf,
}

impl Registry {
    pub fn new(root: impl Into<PathBuf>) -> Registry {
        Registry { root: root.into() }
    }

    pub fn host_dir(&self, id: &str) -> PathBuf {
        self.root.join(id)
    }

    /// Every host that ever started under this root, sorted by id. Directories
    /// without a `host.kv` (a host that never finished starting) are skipped.
    pub fn scan(&self) -> io::Result<Vec<HostStatus>> {
        self.scan_where(|_| true)
    }

    /// Like `scan`, but only reads the directories whose name `keep` accepts, so
    /// a caller that wants one task's hosts does not read every host's files.
    pub fn scan_where(&self, keep: impl Fn(&str) -> bool) -> io::Result<Vec<HostStatus>> {
        let mut out = vec![];
        let entries = match fs::read_dir(&self.root) {
            Ok(e) => e,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(out),
            Err(e) => return Err(e),
        };
        for e in entries {
            let e = e?;
            if !keep(&e.file_name().to_string_lossy()) {
                continue;
            }
            let p = e.path();
            if p.is_dir() {
                if let Ok(s) = status_of(&p) {
                    out.push(s);
                }
            }
        }
        out.sort_by(|a, b| a.info.id.cmp(&b.info.id));
        Ok(out)
    }
}

/// Read journal bytes `[offset, offset + max)` straight from disk. Used when
/// the host is gone.
pub fn read_journal(dir: &Path, offset: u64, max: usize) -> io::Result<Vec<u8>> {
    let f = fs::File::open(dir.join("journal"))?;
    let mut buf = vec![0u8; max];
    let n = f.read_at(&mut buf, offset)?;
    buf.truncate(n);
    Ok(buf)
}

// ---- Why a host was lost, and killing only what is verifiably ours ----

fn ps_field(pid: u32, field: &str) -> Option<String> {
    let out = std::process::Command::new("ps")
        .args(["-o", &format!("{field}="), "-p", &pid.to_string()])
        .output()
        .ok()?;
    let t = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if out.status.success() && !t.is_empty() {
        Some(t)
    } else {
        None
    }
}

/// The start time of a process as `ps` prints it (whole seconds). With the pid it
/// identifies one process, even after the OS reuses the pid.
pub fn proc_start(pid: u32) -> Option<String> {
    ps_field(pid, "lstart")
}

/// The command line of a process, as `ps` prints it.
pub fn proc_command(pid: u32) -> Option<String> {
    ps_field(pid, "command")
}

/// The process group of a process.
pub fn proc_pgid(pid: u32) -> Option<u32> {
    ps_field(pid, "pgid")?.parse().ok()
}

/// `HH:MM:SS` local time of a time in ms since the epoch.
pub fn clock(ms: u64) -> String {
    let secs = ms / 1000;
    for flag in ["-r", "-d@"] {
        let arg = if flag == "-r" {
            vec!["-r".to_string(), secs.to_string(), "+%H:%M:%S".to_string()]
        } else {
            vec![format!("-d@{secs}"), "+%H:%M:%S".to_string()]
        };
        if let Ok(o) = std::process::Command::new("date").args(&arg).output() {
            let t = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if o.status.success() && t.len() == 8 {
                return t;
            }
        }
    }
    let d = secs % 86400;
    format!("{:02}:{:02}:{:02} UTC", d / 3600, d % 3600 / 60, d % 60)
}

/// What is known about a host that went away without an exit record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LostReport {
    pub id: String,
    pub host_pid: u32,
    pub child_pid: u32,
    /// The host process still exists (and is the one recorded).
    pub host_alive: bool,
    /// The child still exists (and is the one recorded).
    pub child_alive: bool,
    /// The signal the host's parent saw end it; None when unknown (a daemon that
    /// did not start it, or restarted since).
    pub signal: Option<i32>,
    pub journal_len: u64,
    /// The journal's last byte is not a newline: the child was cut off mid-line.
    pub ends_mid_line: bool,
    pub last_heartbeat_ms: Option<u64>,
    /// When the host was seen gone (ms since the epoch).
    pub at_ms: u64,
}

impl std::fmt::Display for LostReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "host pid {} {} at {}",
            self.host_pid,
            if self.host_alive {
                "unreachable"
            } else {
                "gone"
            },
            clock(self.at_ms)
        )?;
        match self.signal {
            Some(s) => write!(f, ", killed by signal {s}")?,
            None => write!(f, ", signal unknown")?,
        }
        write!(f, ", no exit record")?;
        write!(
            f,
            "; child pid {} {}",
            self.child_pid,
            if self.child_alive {
                "still running"
            } else {
                "gone"
            }
        )?;
        write!(f, "; journal {} bytes", self.journal_len)?;
        if self.ends_mid_line {
            write!(f, " (cut off mid-line)")?;
        }
        Ok(())
    }
}

fn kv_matches(kv: &HashMap<String, String>, key: &str, pid: u32) -> bool {
    match kv.get(key) {
        Some(want) if !want.is_empty() => proc_start(pid).as_deref() == Some(want.as_str()),
        _ => pid_alive(pid),
    }
}

/// Read-only: gather what is known about a lost host in `dir`.
pub fn lost_report(dir: &Path) -> io::Result<LostReport> {
    let info = HostInfo::read(dir)?;
    let kv = read_kv(&dir.join("host.kv")).unwrap_or_default();
    let journal = fs::read(dir.join("journal")).unwrap_or_default();
    let (signal, seen) =
        crate::launcher::exit_signal(dir).map_or((None, None), |(s, at)| (s, Some(at)));
    Ok(LostReport {
        id: info.id.clone(),
        host_pid: info.host_pid,
        child_pid: info.child_pid,
        host_alive: info.host_pid > 1 && kv_matches(&kv, "host_start", info.host_pid),
        child_alive: info.child_pid > 1 && kv_matches(&kv, "child_start", info.child_pid),
        signal,
        journal_len: journal.len() as u64,
        ends_mid_line: journal.last().is_some_and(|b| *b != b'\n'),
        last_heartbeat_ms: fs::read_to_string(dir.join("hb"))
            .ok()
            .and_then(|s| s.trim().parse().ok()),
        at_ms: seen.unwrap_or_else(now_ms),
    })
}

/// What `reap_dir` did with one registry entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reaped {
    pub id: String,
    pub host_killed: bool,
    pub child_group_killed: bool,
}

/// Kill the host and the child's process group recorded in `dir`, but only the
/// ones that are verifiably still the processes the registry recorded: the start
/// string matches, the host's command line names `step-host` and this directory,
/// and the child leads its own process group. A pid that is 0 or 1, our own pid
/// or our own process group is never signalled, and an entry without start
/// strings (written by an older host) is left alone.
pub fn reap_dir(dir: &Path) -> Reaped {
    use nix::sys::signal::{kill, killpg, Signal};
    use nix::unistd::{getpgrp, getpid, Pid};
    let mut out = Reaped {
        id: dir
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into()),
        host_killed: false,
        child_group_killed: false,
    };
    let (Ok(info), Ok(kv)) = (HostInfo::read(dir), read_kv(&dir.join("host.kv"))) else {
        return out;
    };
    let own = getpid().as_raw() as u32;
    let own_pg = getpgrp().as_raw() as u32;
    let start_ok = |key: &str, pid: u32| -> bool {
        match kv.get(key) {
            Some(want) if !want.is_empty() => proc_start(pid).as_deref() == Some(want.as_str()),
            _ => false,
        }
    };
    let child = info.child_pid;
    if child > 1
        && child != own
        && child != own_pg
        && start_ok("child_start", child)
        && proc_pgid(child) == Some(child)
    {
        out.child_group_killed = killpg(Pid::from_raw(child as i32), Signal::SIGKILL).is_ok();
    }
    let host = info.host_pid;
    let dir_s = dir.display().to_string();
    if host > 1
        && host != own
        && host != own_pg
        && start_ok("host_start", host)
        && proc_command(host).is_some_and(|c| c.contains("step-host") && c.contains(&dir_s))
    {
        out.host_killed = kill(Pid::from_raw(host as i32), Signal::SIGKILL).is_ok();
    }
    out
}

/// `reap_dir` for every host under `root`.
pub fn reap_verified(root: &Path) -> Vec<Reaped> {
    let mut out = vec![];
    if let Ok(entries) = fs::read_dir(root) {
        for e in entries.flatten() {
            let p = e.path();
            if p.join("host.kv").exists() {
                out.push(reap_dir(&p));
            }
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

#[cfg(test)]
mod limit_tests {
    use super::remaining_at;
    use std::time::Duration as D;
    #[test]
    fn remaining_limit_counts_from_host_start() {
        assert_eq!(remaining_at(D::from_secs(3), 1000, 1000), D::from_secs(3));
        assert_eq!(remaining_at(D::from_secs(3), 1000, 3000), D::from_secs(1));
        assert_eq!(remaining_at(D::from_secs(3), 1000, 9000), D::ZERO);
        assert_eq!(remaining_at(D::from_secs(3), 5000, 1000), D::from_secs(3));
    }
}
