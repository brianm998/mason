// SPDX-License-Identifier: GPL-3.0-or-later

//! Starting a host from the daemon.

use crate::registry::HostInfo;
use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct SpawnOptions {
    pub host_bin: PathBuf,
    pub root: PathBuf,
    pub id: String,
    pub cmd: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(String, String)>,
    pub clear_env: bool,
    pub stdin: Option<PathBuf>,
    pub linger: Duration,
    pub heartbeat: Duration,
}

impl SpawnOptions {
    pub fn new(
        host_bin: impl Into<PathBuf>,
        root: impl Into<PathBuf>,
        id: &str,
        cmd: &[&str],
    ) -> Self {
        SpawnOptions {
            host_bin: host_bin.into(),
            root: root.into(),
            id: id.into(),
            cmd: cmd.iter().map(|s| s.to_string()).collect(),
            cwd: None,
            env: vec![],
            clear_env: false,
            stdin: None,
            linger: Duration::from_secs(600),
            heartbeat: Duration::from_secs(1),
        }
    }
}

type Seen = Mutex<HashMap<PathBuf, (Option<i32>, u64)>>;

fn seen() -> &'static Seen {
    static SEEN: OnceLock<Seen> = OnceLock::new();
    SEEN.get_or_init(Default::default)
}

/// What this process saw when the host it started in `dir` ended: the signal that
/// ended it (None when it exited normally) and the time. None when this process
/// did not start it or has not seen it end (for instance after a restart).
pub fn exit_signal(dir: &Path) -> Option<(Option<i32>, u64)> {
    seen()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(dir)
        .copied()
}

fn note_exit(dir: &Path, st: ExitStatus) {
    seen().lock().unwrap_or_else(|e| e.into_inner()).insert(
        dir.to_path_buf(),
        (
            mason_platform::process::exit_signal(&st),
            crate::registry::now_ms(),
        ),
    );
    reaping()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(dir);
}

/// Hosts this process started and is still waiting on (its reaper has not seen them end).
fn reaping() -> &'static Mutex<std::collections::HashSet<PathBuf>> {
    static R: OnceLock<Mutex<std::collections::HashSet<PathBuf>>> = OnceLock::new();
    R.get_or_init(Default::default)
}

/// Like `exit_signal`, but when this process started the host in `dir` and its reaper has
/// not yet seen it end, waits up to `limit` for it to. A host found dead is reaped within
/// moments, so a caller that noticed first still gets the signal, not "unknown".
pub fn exit_signal_within(dir: &Path, limit: Duration) -> Option<(Option<i32>, u64)> {
    let end = Instant::now() + limit;
    loop {
        if let Some(s) = exit_signal(dir) {
            return Some(s);
        }
        let pending = reaping()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(dir);
        if !pending || Instant::now() >= end {
            return None;
        }
        thread::sleep(Duration::from_millis(10));
    }
}

/// A launch in progress: the host id and when the wait began (ms since the epoch).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launching {
    pub id: String,
    pub since_ms: u64,
}

type Launches = Mutex<Vec<(u64, Launching)>>;

fn launches() -> &'static Launches {
    static L: OnceLock<Launches> = OnceLock::new();
    L.get_or_init(Default::default)
}

/// The launches this process is waiting on right now, oldest first.
pub fn launching() -> Vec<Launching> {
    let mut v: Vec<Launching> = launches()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .map(|(_, l)| l.clone())
        .collect();
    v.sort_by_key(|l| l.since_ms);
    v
}

/// Lists a launch while it lives; removes it however the launch ends.
struct LaunchGuard(u64);

impl LaunchGuard {
    fn new(id: &str) -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        launches().lock().unwrap_or_else(|e| e.into_inner()).push((
            n,
            Launching {
                id: id.to_string(),
                since_ms: crate::registry::now_ms(),
            },
        ));
        LaunchGuard(n)
    }
}

impl Drop for LaunchGuard {
    fn drop(&mut self) {
        launches()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|(n, _)| *n != self.0);
    }
}

/// How long [`spawn_host`] waits for a host to come up. A host starts in
/// milliseconds, but a launch can freeze before its first instruction for as long
/// as the operating system cannot vet new processes: on macOS, while `syspolicyd`
/// is down, which was seen to last 22 minutes. The launch then goes on by itself,
/// so waiting costs a task nothing and giving up costs it an attempt.
pub const START_LIMIT: Duration = Duration::from_secs(60 * 60);

/// After this long without a host, say so once in the daemon's log.
const SLOW_START: Duration = Duration::from_secs(60);

/// Start a host and return once it is up (its record is on disk and its socket
/// is listening). The host outlives this process.
pub fn spawn_host(o: &SpawnOptions) -> io::Result<HostInfo> {
    spawn_host_within(o, START_LIMIT)
}

/// [`spawn_host`] with its own limit. A host that is not up by then is killed, so
/// that a launch reported as failed cannot come up later and run its command for a
/// step that has already been closed.
pub fn spawn_host_within(o: &SpawnOptions, limit: Duration) -> io::Result<HostInfo> {
    let dir = o.root.join(&o.id);
    let mut cmd = Command::new(&o.host_bin);
    cmd.arg("--dir").arg(&dir).arg("--id").arg(&o.id);
    cmd.arg("--linger-ms").arg(o.linger.as_millis().to_string());
    cmd.arg("--hb-ms").arg(o.heartbeat.as_millis().to_string());
    if let Some(c) = &o.cwd {
        cmd.arg("--cwd").arg(c);
    }
    if o.clear_env {
        cmd.arg("--clear-env");
    }
    if let Some(p) = &o.stdin {
        cmd.arg("--stdin").arg(p);
    }
    for (k, v) in &o.env {
        cmd.arg("--env").arg(format!("{k}={v}"));
    }
    cmd.arg("--").args(&o.cmd);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = cmd.spawn()?;
    let _listed = LaunchGuard::new(&o.id);

    let started = Instant::now();
    let mut warned = false;
    let result = loop {
        if let Ok(info) = HostInfo::read(&dir) {
            if info.sock.exists() {
                break Ok(info);
            }
        }
        match child.try_wait() {
            Ok(None) => {}
            exited => {
                if let Ok(Some(st)) = exited {
                    note_exit(&dir, st);
                }
                // It may have written its record and finished very quickly.
                return HostInfo::read(&dir)
                    .map_err(|_| io::Error::other("step host exited before it was up"));
            }
        }
        let waited = started.elapsed();
        if waited > limit {
            // Goes through `Child`, so it cannot reach a recycled pid. A process
            // frozen in its launch dies as soon as the launch lets go of it.
            let _ = child.kill();
            break Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "step host did not start within {}s (killed; on macOS, check that \
                     syspolicyd is running)",
                    limit.as_secs()
                ),
            ));
        }
        if waited > SLOW_START && !warned {
            warned = true;
            eprintln!(
                "step host {} has not started after {}s; still waiting, up to {}s \
                 (on macOS, check that syspolicyd is running)",
                o.id,
                SLOW_START.as_secs(),
                limit.as_secs()
            );
        }
        thread::sleep(Duration::from_millis(5));
    };

    // Reap the host when it eventually exits, without blocking the caller.
    reaping()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(dir.clone());
    thread::spawn(move || {
        if let Ok(st) = child.wait() {
            note_exit(&dir, st);
        }
    });
    result
}
