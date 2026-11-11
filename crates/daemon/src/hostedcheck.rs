// SPDX-License-Identifier: GPL-3.0-or-later

//! A test command run under a step host, so that a daemon which restarts, or
//! takes over by `exec`, finds it again instead of leaving it orphaned.
//!
//! Used by the develop agent's `run_check` (host id `t<task>r<run>-check<n>`) and
//! by landing's re-test (`b<batch>-retest<n>`). Each run has a record next to its
//! log, `<id>.check`, holding a key (the tree it ran on, and the command) and
//! whether its result has been given to a caller. A call:
//!
//! - attaches to a run with the same key that is in flight (in this process, or
//!   found again after a restart);
//! - takes the result of a run with the same key that finished while the daemon
//!   was away and was never given to anyone;
//! - otherwise starts a new run on the next free number.
//!
//! The limit counts from the host's start, not from the call that collects it.

use crate::daemon::{now_ms, Daemon};
use mason_landing::git;
use mason_step_host::client::{collect_to_file, ClientError, HostClient};
use mason_step_host::launcher::{spawn_host, SpawnOptions};
use mason_step_host::protocol::{CURRENT, PREVIOUS};
use mason_step_host::registry::{read_kv, write_kv, HostInfo};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

pub struct CheckSpec {
    /// `t5r7-check`, `b3-retest`: the run's number is appended.
    pub id_prefix: String,
    /// Where the log, exit record and run record go.
    pub log_dir: PathBuf,
    pub cmd: String,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
    pub limit: Duration,
    /// What the command runs on, from [`tree_key`].
    pub tree: String,
}

#[derive(Clone, Debug)]
pub struct CheckOutcome {
    pub passed: bool,
    pub timed_out: bool,
    /// The last lines of the output.
    pub output: String,
    /// The id of the host that ran it.
    pub id: String,
}

type Shared = Result<CheckOutcome, String>;

#[derive(Default)]
struct Slot {
    result: Mutex<Option<Shared>>,
    cv: Condvar,
}

fn inflight() -> &'static Mutex<HashMap<PathBuf, Arc<Slot>>> {
    static M: std::sync::OnceLock<Mutex<HashMap<PathBuf, Arc<Slot>>>> = std::sync::OnceLock::new();
    M.get_or_init(Default::default)
}

/// What the tree a command will run on is: its committed tree, and, when the
/// worktree differs from it, a hash of the difference. A key no two states share
/// if git cannot say.
pub fn tree_key(wt: &Path) -> String {
    let unique = || format!("unknown-{}", now_ms());
    let Ok(tree) = git::run(wt, &["rev-parse", "HEAD^{tree}"]) else {
        return unique();
    };
    let tree = tree.trim().to_string();
    let Ok(status) = git::run(
        wt,
        &[
            "--no-optional-locks",
            "status",
            "--porcelain",
            "--untracked-files=all",
        ],
    ) else {
        return unique();
    };
    if status.trim().is_empty() {
        return tree;
    }
    let mut h = Sha256::new();
    h.update(status.as_bytes());
    let Ok(diff) = git::run(wt, &["--no-optional-locks", "diff", "HEAD"]) else {
        return unique();
    };
    h.update(diff.as_bytes());
    let Ok(untracked) = git::run(
        wt,
        &["ls-files", "--others", "--exclude-standard", "--full-name"],
    ) else {
        return unique();
    };
    let files: Vec<&str> = untracked.lines().filter(|l| !l.is_empty()).collect();
    for chunk in files.chunks(200) {
        let mut args = vec!["hash-object", "--no-filters", "--"];
        args.extend(chunk.iter().copied());
        match git::run(wt, &args) {
            Ok(o) => h.update(o.as_bytes()),
            Err(_) => return unique(),
        }
    }
    let sum: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
    format!("{tree}+{}", &sum[..16])
}

fn run_key(spec: &CheckSpec) -> String {
    let mut h = Sha256::new();
    h.update(spec.tree.as_bytes());
    h.update([0]);
    h.update(spec.cmd.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn record_path(spec: &CheckSpec, id: &str) -> PathBuf {
    spec.log_dir.join(format!("{id}.check"))
}

fn mark_delivered(spec: &CheckSpec, id: &str, key: &str) {
    let _ = write_kv(
        &record_path(spec, id),
        &[("key", key.to_string()), ("delivered", "1".into())],
    );
}

/// Is there already a run of `spec` whose result nobody has been given: one in
/// flight (in this process, or found again after a restart) or one that
/// finished while the daemon was away? Such a call attaches or collects, and
/// starts nothing, so it has nothing to queue for.
pub fn pending(d: &Daemon, spec: &CheckSpec) -> bool {
    let key = run_key(spec);
    (0u32..)
        .map(|n| format!("{}{n}", spec.id_prefix))
        .map_while(|id| read_kv(&record_path(spec, &id)).ok().map(|kv| (id, kv)))
        .any(|(id, kv)| {
            kv.get("key") == Some(&key)
                && kv.get("delivered").map(String::as_str) != Some("1")
                && (inflight()
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .contains_key(&d.cfg.hosts_dir().join(&id))
                    || matches!(
                        mason_step_host::registry::status_of(&d.cfg.hosts_dir().join(&id)),
                        Ok(s) if s.state != mason_step_host::registry::HostState::Lost
                    ))
        })
}

/// Run `spec` (or attach to, or take the result of, the run that already is it).
/// Blocks until there is a result.
pub fn run(d: &Arc<Daemon>, spec: &CheckSpec) -> Shared {
    let _ = std::fs::create_dir_all(&spec.log_dir);
    let key = run_key(spec);
    // Find or allocate, one caller at a time, so two calls never take one number.
    let (id, slot, collector) = {
        let mut map = inflight().lock().unwrap_or_else(|e| e.into_inner());
        let mut n = 0u32;
        loop {
            let id = format!("{}{n}", spec.id_prefix);
            let rec = record_path(spec, &id);
            let mapkey = d.cfg.hosts_dir().join(&id);
            match read_kv(&rec) {
                Err(_) => {
                    // A free number: this run is new.
                    for ext in ["log", "exit"] {
                        let _ = std::fs::remove_file(spec.log_dir.join(format!("{id}.{ext}")));
                    }
                    let _ = write_kv(&rec, &[("key", key.clone()), ("delivered", "0".into())]);
                    let slot = Arc::new(Slot::default());
                    map.insert(mapkey, slot.clone());
                    break (id, slot, true);
                }
                Ok(kv) => {
                    let same = kv.get("key") == Some(&key);
                    let delivered = kv.get("delivered").map(String::as_str) == Some("1");
                    if same && !delivered {
                        if let Some(slot) = map.get(&mapkey) {
                            break (id, slot.clone(), false);
                        }
                        // In flight or finished in a daemon that is gone: collect it.
                        let slot = Arc::new(Slot::default());
                        map.insert(mapkey, slot.clone());
                        break (id, slot, true);
                    }
                }
            }
            n += 1;
        }
    };
    if !collector {
        let mut g = slot.result.lock().unwrap_or_else(|e| e.into_inner());
        while g.is_none() {
            g = slot.cv.wait(g).unwrap_or_else(|e| e.into_inner());
        }
        return g.clone().unwrap_or_else(|| Err("no result".into()));
    }
    let res = collect(d, spec, &id);
    // Given to its callers now: a later call on the same tree runs again.
    mark_delivered(spec, &id, &key);
    {
        let mut g = slot.result.lock().unwrap_or_else(|e| e.into_inner());
        *g = Some(res.clone());
        slot.cv.notify_all();
    }
    inflight()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&d.cfg.hosts_dir().join(&id));
    res
}

fn collect(d: &Arc<Daemon>, spec: &CheckSpec, id: &str) -> Shared {
    let hosts = d.cfg.hosts_dir();
    let dir = hosts.join(id);
    if !dir.join("host.kv").exists() {
        let o = SpawnOptions {
            host_bin: d.cfg.host_bin.clone(),
            root: hosts.clone(),
            id: id.to_string(),
            cmd: vec!["sh".into(), "-c".into(), spec.cmd.clone()],
            cwd: Some(spec.cwd.clone()),
            env: spec.env.clone(),
            clear_env: false,
            stdin: None,
            linger: Duration::from_secs(600),
            heartbeat: d.cfg.heartbeat,
        };
        spawn_host(&o).map_err(|e| format!("cannot start the command: {e}"))?;
    }
    let started = HostInfo::read(&dir).map_or_else(|_| now_ms(), |i| i.started_ms);
    let deadline = started.saturating_add(spec.limit.as_millis() as u64);
    // Stop it when the limit, counted from the host's start, has passed.
    let done = Arc::new(AtomicBool::new(false));
    let fired = Arc::new(AtomicBool::new(false));
    {
        let (done, fired, sock) = (done.clone(), fired.clone(), dir.join("sock"));
        std::thread::spawn(move || loop {
            if done.load(Ordering::SeqCst) {
                return;
            }
            if now_ms() >= deadline {
                fired.store(true, Ordering::SeqCst);
                if let Ok(mut h) = HostClient::connect(&sock, PREVIOUS, CURRENT, "daemon") {
                    let _ = h.stop(1000);
                }
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        });
    }
    let log = spec.log_dir.join(format!("{id}.log"));
    let exit = spec.log_dir.join(format!("{id}.exit"));
    let rec = collect_to_file(&dir, &log, &exit);
    done.store(true, Ordering::SeqCst);
    let rec = match rec {
        Ok(r) => r,
        Err(ClientError::HostLost { .. }) => {
            return Err("the check was lost (its host died); run it again".into())
        }
        Err(e) => return Err(format!("cannot collect the check's result: {e}")),
    };
    let timed_out = fired.load(Ordering::SeqCst)
        || (rec.signal != 0 && rec.ended_ms > 0 && rec.ended_ms + 1500 >= deadline);
    Ok(CheckOutcome {
        passed: rec.code == 0 && rec.signal == 0 && !timed_out,
        timed_out,
        output: crate::jobs::tail(&std::fs::read_to_string(&log).unwrap_or_default(), 40),
        id: id.to_string(),
    })
}
