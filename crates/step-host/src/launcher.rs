// SPDX-License-Identifier: GPL-3.0-or-later

//! Starting a host from the daemon.

use crate::registry::HostInfo;
use std::collections::HashMap;
use std::fs;
use std::io::{self, Read, Seek, SeekFrom};
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

/// Whether a variable's value must stay off the host's command line.
fn is_secret(name: &str) -> bool {
    ["_TOKEN", "_API_KEY", "_SECRET"]
        .iter()
        .any(|end| name.ends_with(end))
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

type Launches = Mutex<Vec<(u64, PathBuf, Launching)>>;

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
        .map(|(_, _, l)| l.clone())
        .collect();
    v.sort_by_key(|l| l.since_ms);
    v
}

/// The launches waiting right now whose hosts root is `dir` or lies under it, oldest first.
/// A daemon asks about its own hosts directory, so launches of other daemons in the same
/// process (tests run many) are not its business.
pub fn launching_under(dir: &Path) -> Vec<Launching> {
    let mut v: Vec<Launching> = launches()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .filter(|(_, root, _)| root.starts_with(dir))
        .map(|(_, _, l)| l.clone())
        .collect();
    v.sort_by_key(|l| l.since_ms);
    v
}

/// Lists a launch while it lives; removes it however the launch ends.
struct LaunchGuard(u64);

impl LaunchGuard {
    fn new(id: &str, root: &Path) -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        launches().lock().unwrap_or_else(|e| e.into_inner()).push((
            n,
            root.to_path_buf(),
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
            .retain(|(n, _, _)| *n != self.0);
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

/// The file in a host's directory that takes the host's own standard error (not its
/// command's, which goes to the journal).
pub const HOST_ERR: &str = "host.err";

/// How much of [`HOST_ERR`] a failed launch carries back: this many bytes from its end,
/// and of those this many lines.
const ERR_TAIL_BYTES: u64 = 2000;
const ERR_TAIL_LINES: usize = 5;

/// The last lines this launch's host wrote to `path` (from byte `from` on), joined into one.
fn err_tail(path: &Path, from: u64) -> String {
    let Ok(mut f) = fs::File::open(path) else {
        return String::new();
    };
    let len = f.metadata().map_or(0, |m| m.len());
    let at = from.max(len.saturating_sub(ERR_TAIL_BYTES));
    let mut buf = vec![];
    if f.seek(SeekFrom::Start(at)).is_err() || f.read_to_end(&mut buf).is_err() {
        return String::new();
    }
    let text = String::from_utf8_lossy(&buf);
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    lines[lines.len().saturating_sub(ERR_TAIL_LINES)..].join(" | ")
}

/// Take away what a launch that came to nothing left behind when there is nothing in it to
/// read: an empty [`HOST_ERR`], and the directory if this launch made it and it is empty.
fn discard_unused(dir: &Path, made: bool) {
    let err = dir.join(HOST_ERR);
    if fs::metadata(&err).is_ok_and(|m| m.len() == 0) {
        let _ = fs::remove_file(&err);
    }
    if made {
        let _ = fs::remove_dir(dir);
    }
}

/// [`spawn_host`] with its own limit. A host that is not up by then is killed, so
/// that a launch reported as failed cannot come up later and run its command for a
/// step that has already been closed.
pub fn spawn_host_within(o: &SpawnOptions, limit: Duration) -> io::Result<HostInfo> {
    let dir = o.root.join(&o.id);
    // The host would refuse it too, but could only say so after it was started.
    crate::host::check_sock_path(&dir.join("sock"))
        .inspect_err(|e| eprintln!("step host {} cannot start: {e}", o.id))?;
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
        // `ps` shows every user a process's arguments but not its environment, so a
        // secret travels in the host's environment and only its name is an argument.
        if is_secret(k) {
            cmd.env(k, v).arg("--env-pass").arg(k);
        } else {
            cmd.arg("--env").arg(format!("{k}={v}"));
        }
    }
    cmd.arg("--").args(&o.cmd);
    let made = !dir.exists();
    fs::create_dir_all(&dir)?;
    let err_path = dir.join(HOST_ERR);
    let err_file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&err_path)?;
    // A second launch into the same directory reports only what its own host wrote.
    let err_from = err_file.metadata().map_or(0, |m| m.len());
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(err_file));
    mason_platform::process::detached(&mut cmd);
    let spawned = cmd.spawn();
    drop(cmd); // closes our handle on the error file
    let mut child = spawned.inspect_err(|_| discard_unused(&dir, made))?;
    let _listed = LaunchGuard::new(&o.id, &o.root);

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
                let status = match exited {
                    Ok(Some(st)) => {
                        note_exit(&dir, st);
                        st.to_string()
                    }
                    Ok(None) => "exit status unknown".to_string(),
                    Err(e) => format!("exit status unknown: {e}"),
                };
                // It may have written its record and finished very quickly.
                return HostInfo::read(&dir).map_err(|_| {
                    let said = match err_tail(&err_path, err_from) {
                        t if t.is_empty() => "it wrote no error".to_string(),
                        t => t,
                    };
                    let msg = format!("step host exited before it was up ({status}): {said}");
                    eprintln!("step host {} in {}: {msg}", o.id, dir.display());
                    io::Error::other(msg)
                });
            }
        }
        let waited = started.elapsed();
        if waited > limit {
            // Goes through `Child`, so it cannot reach a recycled pid. A process
            // frozen in its launch dies as soon as the launch lets go of it.
            let _ = child.kill();
            discard_unused(&dir, made);
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
