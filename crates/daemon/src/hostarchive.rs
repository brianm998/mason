// SPDX-License-Identifier: GPL-3.0-or-later

//! Finished step hosts leave the registry (s98).
//!
//! `h/` is the directory every scan reads, so a directory for every host that ever ran made
//! every `/status`, sweep and task page slower the longer the daemon lived. Once a step run has
//! closed, its hosts have exited and a grace period has passed (so a daemon that comes back
//! later still finds them where it looks), the sweep moves `h/<id>/` and `h/<id>.in` to
//! `t/<task>/hosts/`. Readers of old journals look in both places. After `retention_days` the
//! moved directories are deleted; the task logs in `t/<task>/` stay. The per-session MCP
//! config directories in `a/` of sessions that have ended are removed too.
//!
//! A move or removal that fails is logged and tried again on the next pass; nothing here is
//! fatal. A host of an open run is never touched: `reattach` and `run_command_once` find it in
//! `h/`.

use crate::config::DaemonConfig;
use mason_core::*;
use mason_engine::World;
use mason_step_host::registry::{status_of, ExitRecord, HostInfo, HostState, HostStatus, Registry};
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::time::UNIX_EPOCH;

const DAY_MS: u64 = 86_400_000;

/// What one pass did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Host ids moved to `t/<task>/hosts/`.
    pub moved: Vec<String>,
    /// Host ids deleted from `t/<task>/hosts/` for being past retention.
    pub deleted: Vec<String>,
    /// Session directories removed from `a/`.
    pub sessions: Vec<String>,
    /// Moves and removals that failed (they are tried again next pass).
    pub failed: usize,
}

fn mtime_ms(p: &Path) -> u64 {
    fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as u64)
}

/// When a host stopped: the exit record's time, else the last sign of life it left.
fn finished_ms(dir: &Path) -> u64 {
    if let Ok(r) = ExitRecord::read(dir) {
        return r.ended_ms;
    }
    if let Ok(i) = HostInfo::read(dir) {
        let hb = fs::read_to_string(dir.join("hb"))
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .unwrap_or(0);
        return i.started_ms.max(i.heartbeat_ms).max(hb);
    }
    mtime_ms(dir)
}

/// The ids in a registry-shaped directory: a directory per host and `<id>.in` input files.
fn ids_in(root: &Path) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    for e in fs::read_dir(root).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        match name.strip_suffix(".in") {
            Some(id) => ids.insert(id.to_string()),
            None => ids.insert(name),
        };
    }
    ids
}

fn fail(what: &str, e: &std::io::Error, report: &mut Report) {
    eprintln!("host directories: {what}: {e}; will try again");
    report.failed += 1;
}

/// Every host of `task`, live and moved: the live registry first, then the archive, without
/// repeats (a host moved between the two reads is seen once). Sorted by id.
pub fn task_hosts(cfg: &DaemonConfig, task: u64) -> Vec<HostStatus> {
    let prefix = format!("t{task}r");
    let mine = |n: &str| n.starts_with(&prefix);
    let mut out = Registry::new(cfg.hosts_dir())
        .scan_where(mine)
        .unwrap_or_default();
    let seen: BTreeSet<String> = out.iter().map(|h| h.info.id.clone()).collect();
    for h in Registry::new(cfg.archived_hosts_dir(task))
        .scan_where(mine)
        .unwrap_or_default()
    {
        if !seen.contains(&h.info.id) {
            out.push(h);
        }
    }
    out.sort_by(|a, b| a.info.id.cmp(&b.info.id));
    out
}

/// One pass at time `now` (ms since the epoch).
pub fn pass(cfg: &DaemonConfig, w: &World, now: u64) -> Report {
    let mut rep = Report::default();
    let grace = cfg.host_grace.as_millis() as u64;
    move_closed(cfg, w, now, grace, &mut rep);
    if cfg.retention_days > 0 {
        expire(
            cfg,
            now,
            cfg.retention_days.saturating_mul(DAY_MS),
            &mut rep,
        );
    }
    end_sessions(cfg, w, now, grace, &mut rep);
    rep
}

fn move_closed(cfg: &DaemonConfig, w: &World, now: u64, grace: u64, rep: &mut Report) {
    let root = cfg.hosts_dir();
    for id in ids_in(&root) {
        let Some((task, run)) = crate::sweep::parse_host_id(&id) else {
            continue;
        };
        // Only a closed run's hosts move.
        let closed = w
            .step_runs
            .get(&StepRunId(run))
            .is_some_and(|r| r.task == TaskId(task) && r.outcome.is_some());
        if !closed {
            continue;
        }
        let dir = root.join(&id);
        let input = root.join(format!("{id}.in"));
        let stopped = if dir.is_dir() {
            if matches!(status_of(&dir).map(|s| s.state), Ok(HostState::Running)) {
                continue;
            }
            finished_ms(&dir)
        } else {
            mtime_ms(&input)
        };
        if now.saturating_sub(stopped) < grace {
            continue;
        }
        let dest = cfg.archived_hosts_dir(task);
        if let Err(e) = fs::create_dir_all(&dest) {
            fail(&format!("cannot make {}", dest.display()), &e, rep);
            continue;
        }
        let mut ok = true;
        if dir.is_dir() {
            if let Err(e) = fs::rename(&dir, dest.join(&id)) {
                fail(&format!("cannot move {}", dir.display()), &e, rep);
                ok = false;
            }
        }
        if ok && input.exists() {
            if let Err(e) = fs::rename(&input, dest.join(format!("{id}.in"))) {
                fail(&format!("cannot move {}", input.display()), &e, rep);
                ok = false;
            }
        }
        if ok {
            rep.moved.push(id);
        }
    }
}

fn expire(cfg: &DaemonConfig, now: u64, keep_ms: u64, rep: &mut Report) {
    let tasks = cfg.data_dir.join("t");
    for t in fs::read_dir(&tasks).into_iter().flatten().flatten() {
        let hosts = t.path().join("hosts");
        for id in ids_in(&hosts) {
            let dir = hosts.join(&id);
            let input = hosts.join(format!("{id}.in"));
            let stopped = if dir.is_dir() {
                finished_ms(&dir)
            } else {
                mtime_ms(&input)
            };
            if now.saturating_sub(stopped) <= keep_ms {
                continue;
            }
            let mut ok = true;
            if dir.exists() {
                if let Err(e) = fs::remove_dir_all(&dir) {
                    fail(&format!("cannot delete {}", dir.display()), &e, rep);
                    ok = false;
                }
            }
            if ok && input.exists() {
                if let Err(e) = fs::remove_file(&input) {
                    fail(&format!("cannot delete {}", input.display()), &e, rep);
                    ok = false;
                }
            }
            if ok {
                rep.deleted.push(id);
            }
        }
        let _ = fs::remove_dir(&hosts); // only if now empty
    }
}

/// `a/<pid>-<n>/` holds one session's MCP config, read when the agent starts. A directory is
/// of an ended session when it is older than the grace period and was made before the oldest
/// step run still open began (a session of that run, or a later one, is newer).
fn end_sessions(cfg: &DaemonConfig, w: &World, now: u64, grace: u64, rep: &mut Report) {
    let oldest_open = w
        .step_runs
        .values()
        .filter(|r| r.outcome.is_none())
        .map(|r| r.started_at.0)
        .min()
        .unwrap_or(u64::MAX);
    for e in fs::read_dir(cfg.agent_state_dir())
        .into_iter()
        .flatten()
        .flatten()
    {
        let name = e.file_name().to_string_lossy().to_string();
        let shaped = name
            .split_once('-')
            .is_some_and(|(a, b)| a.parse::<u32>().is_ok() && b.parse::<u32>().is_ok());
        let path = e.path();
        if !shaped || !path.is_dir() {
            continue;
        }
        let made = mtime_ms(&path);
        if now.saturating_sub(made) < grace || made >= oldest_open {
            continue;
        }
        match fs::remove_dir_all(&path) {
            Ok(()) => rep.sessions.push(name),
            Err(e) => fail(&format!("cannot remove {}", path.display()), &e, rep),
        }
    }
}
