// SPDX-License-Identifier: GPL-3.0-or-later

//! Compile holders that stopped making progress. A `compile` slot held by a
//! test binary that hangs (#37's `protocol` test, once) is a slot every waiting
//! task goes without until someone kills it by hand. Every so often the daemon
//! reads the CPU time of each holder's processes; a holder whose processes used
//! almost none for long enough reads as idle on the home page, and a task's
//! command that stays idle past `[cpu] compile_idle_kill_s` is stopped, which
//! frees its slot. A person's own command (`mason run`) is never stopped, only
//! shown as idle: they can see it and decide.

use crate::daemon::Daemon;
use std::collections::{HashMap, HashSet};

/// One process from `ps`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Proc {
    pub pid: u32,
    pub ppid: u32,
    pub pgid: u32,
    /// User and system CPU time used so far.
    pub cpu_ms: u64,
    /// Stopped (`ps` state `T`): by a signal, or by a tracer.
    pub stopped: bool,
}

/// Below this share of one core, averaged since the last reading, a holder is idle.
pub const IDLE_SHARE_PCT: u64 = 1;

/// How often the holders are read.
pub const EVERY: std::time::Duration = std::time::Duration::from_secs(30);

/// `ps`'s CPU time: `mm:ss.cc` (macOS, minutes past 60) or `[dd-]hh:mm:ss` (Linux).
pub fn parse_time(t: &str) -> Option<u64> {
    let (days, rest) = match t.split_once('-') {
        Some((d, r)) => (d.parse::<u64>().ok()?, r),
        None => (0, t),
    };
    let parts: Vec<&str> = rest.split(':').collect();
    let secs = |s: &str| -> Option<u64> {
        let (whole, frac) = s.split_once('.').unwrap_or((s, ""));
        let mut ms = whole.parse::<u64>().ok()? * 1000;
        if !frac.is_empty() {
            let f: String = frac.chars().chain("000".chars()).take(3).collect();
            ms += f.parse::<u64>().ok()?;
        }
        Some(ms)
    };
    let ms = match parts.as_slice() {
        [m, s] => m.parse::<u64>().ok()? * 60_000 + secs(s)?,
        [h, m, s] => {
            h.parse::<u64>().ok()? * 3_600_000 + m.parse::<u64>().ok()? * 60_000 + secs(s)?
        }
        _ => return None,
    };
    Some(days * 86_400_000 + ms)
}

/// `ps -axo pid=,ppid=,pgid=,time=,stat=`, one process a line (the state is optional).
pub fn parse_ps(text: &str) -> Vec<Proc> {
    text.lines()
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            Some(Proc {
                pid: f.next()?.parse().ok()?,
                ppid: f.next()?.parse().ok()?,
                pgid: f.next()?.parse().ok()?,
                cpu_ms: parse_time(f.next()?)?,
                stopped: f.next().is_some_and(|st| st.starts_with('T')),
            })
        })
        .collect()
}

pub fn read_ps() -> Vec<Proc> {
    std::process::Command::new("ps")
        .args(["-axo", "pid=,ppid=,pgid=,time=,stat="])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| parse_ps(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or_default()
}

/// `root`, everything under it, and its process group's members (a child
/// that moved itself under init is still in the group). Empty when `root` is gone.
pub fn tree_of(procs: &[Proc], root: u32) -> Vec<&Proc> {
    let Some(leader) = procs.iter().find(|p| p.pid == root) else {
        return vec![];
    };
    let mut seen: HashSet<u32> = HashSet::from([root]);
    let mut frontier = vec![root];
    while let Some(p) = frontier.pop() {
        for c in procs.iter().filter(|c| c.ppid == p) {
            if seen.insert(c.pid) {
                frontier.push(c.pid);
            }
        }
    }
    if leader.pgid == root {
        seen.extend(procs.iter().filter(|p| p.pgid == root).map(|p| p.pid));
    }
    procs.iter().filter(|p| seen.contains(&p.pid)).collect()
}

/// The CPU time of `root` and its tree (see [`tree_of`]). `None` when `root` is gone.
pub fn tree_cpu(procs: &[Proc], root: u32) -> Option<u64> {
    let t = tree_of(procs, root);
    (!t.is_empty()).then(|| t.iter().map(|p| p.cpu_ms).sum())
}

/// When each holder was last seen doing something.
#[derive(Default)]
pub struct Tracker {
    /// key -> (CPU ms at the last reading, when it was read, when last active)
    seen: HashMap<String, (u64, u64, u64)>,
}

impl Tracker {
    /// Note a reading; return how long the holder has been idle. A holder whose
    /// processes used at least [`IDLE_SHARE_PCT`]% of a core since the last
    /// reading is active. CPU time that went down (a process ended) is a new start.
    pub fn observe(&mut self, key: &str, cpu_ms: u64, now: u64) -> u64 {
        let e = self
            .seen
            .entry(key.to_string())
            .or_insert((cpu_ms, now, now));
        let (last_cpu, last_at, _) = *e;
        let elapsed = now.saturating_sub(last_at);
        if cpu_ms < last_cpu || cpu_ms.saturating_sub(last_cpu) * 100 >= elapsed * IDLE_SHARE_PCT {
            e.2 = now;
        }
        e.0 = cpu_ms;
        e.1 = now;
        now.saturating_sub(e.2)
    }

    /// Forget holders that are gone.
    pub fn keep(&mut self, live: &HashSet<String>) {
        self.seen.retain(|k, _| live.contains(k));
    }
}

/// A step host's id that is an agent session (`t95r799-1`), not a command.
fn is_agent_host(id: &str) -> bool {
    id.rsplit_once('-')
        .is_some_and(|(_, n)| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// Read every compile holder once: note how long each has been idle (the home
/// page shows it) and stop a task's command that has been idle too long.
pub fn check(d: &Daemon, tracker: &mut Tracker, now: u64) {
    let holders = d.broker.compile_holders();
    if holders.is_empty() {
        tracker.keep(&HashSet::new());
        d.broker.set_idle(HashMap::new());
        return;
    }
    let procs = read_ps();
    if procs.is_empty() {
        return;
    }
    let reg = mason_step_host::registry::Registry::new(d.cfg.hosts_dir());
    let hosts: Vec<_> = reg
        .scan()
        .unwrap_or_default()
        .into_iter()
        .filter(|h| h.state == mason_step_host::registry::HostState::Running)
        .filter(|h| !is_agent_host(&h.info.id))
        .collect();
    let kill_after_ms = u64::from(d.broker.compile_idle_kill_s()) * 1000;
    let mut idle = HashMap::new();
    let mut live = HashSet::new();
    for h in holders {
        // What runs on this holder's behalf: a person's `mason run` (the process
        // that took the slot), or the task's running command hosts.
        let roots: Vec<(String, u32)> = match h.task {
            Some(t) if !h.interactive => hosts
                .iter()
                .filter(|s| crate::sweep::parse_host_id(&s.info.id).is_some_and(|(tk, _)| tk == t))
                .filter(|s| s.info.child_pid > 0)
                .map(|s| (s.info.id.clone(), s.info.child_pid))
                .collect(),
            _ if h.pid > 0 && h.pid != std::process::id() => vec![(String::new(), h.pid)],
            _ => vec![],
        };
        let cpu: Option<u64> = roots
            .iter()
            .filter_map(|(_, pid)| tree_cpu(&procs, *pid))
            .reduce(|a, b| a + b);
        let Some(cpu) = cpu else { continue };
        let ids: Vec<&str> = roots.iter().map(|(i, _)| i.as_str()).collect();
        let key = format!("{}:{}", h.lease, ids.join(","));
        live.insert(key.clone());
        let idle_ms = tracker.observe(&key, cpu, now);
        idle.insert(h.lease, idle_ms);
        if kill_after_ms > 0 && idle_ms >= kill_after_ms && h.task.is_some() && !h.interactive {
            for (id, _) in roots.iter().filter(|(id, _)| !id.is_empty()) {
                // One watcher to a tree: the stall watcher judges this one.
                if d.watched
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .contains(id)
                {
                    continue;
                }
                let text = format!(
                    "Stopped {}'s command `{id}`: it used no CPU for {} while holding a compile slot",
                    h.label,
                    mason_scheduler::since(now, now - idle_ms)
                );
                tracing::warn!(host = %id, "{text}");
                d.stop_hosts(id);
                d.broker.note(now, text);
            }
        }
    }
    tracker.keep(&live);
    d.broker.set_idle(idle);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ps_times_in_both_formats() {
        assert_eq!(parse_time("0:00.03"), Some(30));
        assert_eq!(parse_time("845:46.55"), Some(845 * 60_000 + 46_550));
        assert_eq!(parse_time("01:02:03"), Some(3_723_000));
        assert_eq!(parse_time("2-00:00:01"), Some(2 * 86_400_000 + 1000));
        assert_eq!(parse_time("x"), None);
    }

    #[test]
    fn a_tree_counts_children_and_the_process_group() {
        let procs = parse_ps(
            "  10     1    10   0:01.00\n  11    10    10   0:02.00\n  12    11    10   0:03.00\n  \
             13     1    10   0:04.00\n  14     1    14   9:00.00\n",
        );
        // 13 left the tree for init but stayed in the group.
        assert_eq!(tree_cpu(&procs, 10), Some(10_000));
        // Not a group leader: only its own subtree.
        assert_eq!(tree_cpu(&procs, 11), Some(5_000));
        assert_eq!(tree_cpu(&procs, 99), None);
    }

    #[test]
    fn idle_is_time_since_the_last_reading_that_used_a_core_percent() {
        let mut t = Tracker::default();
        assert_eq!(t.observe("a", 1_000, 0), 0);
        // 30 s with 1 s of CPU: active.
        assert_eq!(t.observe("a", 2_000, 30_000), 0);
        // 30 s with 0.2 s of CPU (under 1%): idle since the last active reading.
        assert_eq!(t.observe("a", 2_200, 60_000), 30_000);
        assert_eq!(t.observe("a", 2_200, 90_000), 60_000);
        // Busy again.
        assert_eq!(t.observe("a", 9_000, 120_000), 0);
        // A process ended and the total fell: a fresh start, not idle.
        assert_eq!(t.observe("a", 100, 150_000), 0);
        t.keep(&HashSet::new());
        assert_eq!(
            t.observe("a", 100, 999_000),
            0,
            "forgotten, so first seen again"
        );
    }

    #[test]
    fn the_state_column_marks_stopped_processes() {
        let procs = parse_ps("  10     1    10   0:01.00 Ss\n  11    10    10   0:00.00 T\n  12    10    10   0:00.00 T+\n");
        assert!(!procs[0].stopped && procs[1].stopped && procs[2].stopped);
    }

    #[test]
    fn agent_hosts_are_told_apart_from_command_hosts() {
        assert!(is_agent_host("t95r799-1"));
        assert!(!is_agent_host("t95r799-check4"));
        assert!(!is_agent_host("t98r809-test-again1"));
        assert!(!is_agent_host("t98r809-test"));
    }
}
