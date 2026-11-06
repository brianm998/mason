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
        let mut out = vec![];
        let entries = match fs::read_dir(&self.root) {
            Ok(e) => e,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(out),
            Err(e) => return Err(e),
        };
        for e in entries {
            let p = e?.path();
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
