// SPDX-License-Identifier: GPL-3.0-or-later

//! The stall watcher: a suite, a command step or a `mason run` child whose
//! process tree has gone silent. Processes under a test run are sometimes left
//! stopped (state `T`), and the run then sits until its time limit with its slot
//! held. Two rules, each off at zero:
//!
//! * silent for `cont_after` with a stopped process in the tree: SIGCONT every
//!   stopped process (counted);
//! * silent for `kill_after`: the tree is killed, unless a member of it is
//!   waiting on a MASON lease (a nested `mason run`), which is waiting its
//!   turn, not stuck.
//!
//! Silent means no new output bytes and no CPU movement anywhere in the tree
//! since the last progress. The decision ([`tick`]) is pure; [`Watch`] reads
//! `ps`, sends the signals and tells the daemon.

use crate::daemon::{now_ms, Daemon};
use crate::hung::{read_ps, tree_of, Proc};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The two project keys, in milliseconds. Zero turns a rule off.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Thresholds {
    pub cont_after_ms: u64,
    pub kill_after_ms: u64,
}

impl Thresholds {
    pub fn new(cont_after: Duration, kill_after: Duration) -> Thresholds {
        Thresholds {
            cont_after_ms: cont_after.as_millis() as u64,
            kill_after_ms: kill_after.as_millis() as u64,
        }
    }
    pub fn off(&self) -> bool {
        self.cont_after_ms == 0 && self.kill_after_ms == 0
    }
}

/// What the watcher remembers between readings.
#[derive(Clone, Debug)]
pub struct State {
    last_progress: u64,
    cpu: Option<u64>,
    bytes: u64,
    last_cont: Option<u64>,
}

impl State {
    pub fn new(now: u64) -> State {
        State {
            last_progress: now,
            cpu: None,
            bytes: 0,
            last_cont: None,
        }
    }
    /// Milliseconds with no progress, as of the last reading.
    pub fn silent_ms(&self, now: u64) -> u64 {
        now.saturating_sub(self.last_progress)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    /// SIGCONT these processes.
    Cont(Vec<u32>),
    /// Kill the tree; it has been silent this many ms.
    Kill(u64),
    /// Silent long enough to kill, but a member waits on a lease: spared.
    Spared,
}

/// One reading of the tree under `root`. `bytes` is the output so far;
/// `waiting` the pids of lease requests still waiting for their turn.
pub fn tick(
    st: &mut State,
    th: &Thresholds,
    now: u64,
    procs: &[Proc],
    root: u32,
    bytes: u64,
    waiting: &HashSet<u32>,
) -> Option<Action> {
    let tree = tree_of(procs, root);
    if tree.is_empty() {
        return None;
    }
    let cpu: u64 = tree.iter().map(|p| p.cpu_ms).sum();
    // CPU that went down is a process that ended: that is movement too.
    if st.cpu != Some(cpu) || bytes != st.bytes {
        st.last_progress = now;
    }
    st.cpu = Some(cpu);
    st.bytes = bytes;
    let silent = st.silent_ms(now);
    let stopped: Vec<u32> = tree.iter().filter(|p| p.stopped).map(|p| p.pid).collect();
    if th.cont_after_ms > 0
        && silent >= th.cont_after_ms
        && !stopped.is_empty()
        && st
            .last_cont
            .is_none_or(|c| now.saturating_sub(c) >= th.cont_after_ms)
    {
        st.last_cont = Some(now);
        return Some(Action::Cont(stopped));
    }
    if th.kill_after_ms > 0 && silent >= th.kill_after_ms {
        return Some(if tree.iter().any(|p| waiting.contains(&p.pid)) {
            Action::Spared
        } else {
            Action::Kill(silent)
        });
    }
    None
}

/// "no output or CPU for 30 min" (seconds when under a minute).
pub fn stalled_text(silent_ms: u64) -> String {
    let s = silent_ms / 1000;
    let n = if s >= 60 {
        format!("{} min", s / 60)
    } else {
        format!("{s} s")
    };
    format!("stalled: no output or CPU for {n}")
}

/// SIGCONT one process. False when it could not be signalled.
pub fn cont(pid: u32) -> bool {
    mason_platform::process::signal_process(pid, mason_platform::process::Signal::Cont).is_ok()
}

/// SIGKILL a process group (a `mason run` child's tree, which leads its own).
pub fn kill_group(pgid: u32) {
    let _ = mason_platform::process::signal_group(pgid, mason_platform::process::Signal::Kill);
}

/// A watcher on one step host's tree. Dropping it stops the watching.
pub struct Watch {
    done: Arc<AtomicBool>,
    stalled: Arc<Mutex<Option<String>>>,
    d: Arc<Daemon>,
    host: String,
}

impl Watch {
    /// Watch the child of step host `host_id` (its pid is read from the host's own
    /// record) with `log` as its output. `note` is told what was done. `None` when
    /// both rules are off.
    pub fn start(
        d: &Arc<Daemon>,
        host_id: &str,
        log: PathBuf,
        th: Thresholds,
        note: impl Fn(String) + Send + 'static,
    ) -> Option<Watch> {
        if th.off() {
            return None;
        }
        let done = Arc::new(AtomicBool::new(false));
        let stalled = Arc::new(Mutex::new(None));
        if th.kill_after_ms > 0 {
            d.watched
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(host_id.to_string());
        }
        let (d2, done2, stalled2, id) = (
            d.clone(),
            done.clone(),
            stalled.clone(),
            host_id.to_string(),
        );
        std::thread::spawn(move || {
            let dir = d2.cfg.hosts_dir().join(&id);
            let mut st = State::new(now_ms());
            while !done2.load(Ordering::SeqCst) {
                std::thread::sleep(d2.cfg.watch_tick);
                if done2.load(Ordering::SeqCst) {
                    return;
                }
                let root = match mason_step_host::registry::HostInfo::read(&dir) {
                    Ok(i) if i.child_pid > 1 => i.child_pid,
                    _ => continue,
                };
                let procs = read_ps();
                let bytes = std::fs::metadata(&log).map_or(0, |m| m.len());
                let now = now_ms();
                let waiting = d2.broker.waiting_pids();
                match tick(&mut st, &th, now, &procs, root, bytes, &waiting) {
                    Some(Action::Cont(pids)) => {
                        let n = pids.iter().filter(|p| cont(**p)).count();
                        if n > 0 {
                            d2.stall_conts.fetch_add(1, Ordering::SeqCst);
                            let text = format!(
                                "no output or CPU for {}; sent SIGCONT to {n} stopped process(es)",
                                mason_scheduler::since(now, now - st.silent_ms(now))
                            );
                            tracing::warn!(host = %id, "{text}");
                            note(text);
                        }
                    }
                    Some(Action::Kill(silent)) => {
                        let text = stalled_text(silent);
                        tracing::warn!(host = %id, "{text}; stopping its tree");
                        *stalled2.lock().unwrap_or_else(|e| e.into_inner()) = Some(text.clone());
                        d2.stall_kills.fetch_add(1, Ordering::SeqCst);
                        note(text);
                        d2.stop_log
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .record(&id, std::time::Instant::now());
                        if let Ok(mut h) = mason_step_host::client::HostClient::connect(
                            &dir.join("sock"),
                            mason_step_host::protocol::PREVIOUS,
                            mason_step_host::protocol::CURRENT,
                            "daemon",
                        ) {
                            let _ = h.stop(1000);
                        }
                        return;
                    }
                    Some(Action::Spared) => {
                        d2.stall_spared.fetch_add(1, Ordering::SeqCst);
                    }
                    None => {}
                }
            }
        });
        Some(Watch {
            done,
            stalled,
            d: d.clone(),
            host: host_id.to_string(),
        })
    }

    /// Why the watcher killed the tree, if it did.
    pub fn stalled(&self) -> Option<String> {
        self.stalled
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.done.store(true, Ordering::SeqCst);
        self.d
            .watched
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.host);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(pid: u32, ppid: u32, cpu_ms: u64, stopped: bool) -> Proc {
        Proc {
            pid,
            ppid,
            pgid: 10,
            cpu_ms,
            stopped,
        }
    }
    const TH: Thresholds = Thresholds {
        cont_after_ms: 100,
        kill_after_ms: 300,
    };

    #[test]
    fn a_stopped_descendant_is_continued_after_silence_and_not_before() {
        let procs = [p(10, 1, 5, false), p(11, 10, 0, true)];
        let mut st = State::new(0);
        let none = HashSet::new();
        assert_eq!(tick(&mut st, &TH, 0, &procs, 10, 0, &none), None);
        assert_eq!(tick(&mut st, &TH, 99, &procs, 10, 0, &none), None);
        assert_eq!(
            tick(&mut st, &TH, 100, &procs, 10, 0, &none),
            Some(Action::Cont(vec![11]))
        );
        // Not again until another cont_after has passed.
        assert_eq!(tick(&mut st, &TH, 150, &procs, 10, 0, &none), None);
        assert_eq!(
            tick(&mut st, &TH, 200, &procs, 10, 0, &none),
            Some(Action::Cont(vec![11]))
        );
    }

    #[test]
    fn output_or_cpu_movement_is_progress() {
        let none = HashSet::new();
        let mut st = State::new(0);
        let a = [p(10, 1, 5, false), p(11, 10, 0, true)];
        let b = [p(10, 1, 6, false), p(11, 10, 0, true)];
        let th = Thresholds {
            cont_after_ms: 0,
            kill_after_ms: 100,
        };
        assert_eq!(tick(&mut st, &th, 90, &a, 10, 0, &none), None);
        // CPU moved at 150: the silence starts over.
        assert_eq!(tick(&mut st, &th, 150, &b, 10, 0, &none), None);
        assert_eq!(tick(&mut st, &th, 240, &b, 10, 0, &none), None);
        // New output at 250.
        assert_eq!(tick(&mut st, &th, 250, &b, 10, 7, &none), None);
        assert_eq!(tick(&mut st, &th, 340, &b, 10, 7, &none), None);
        assert!(tick(&mut st, &th, 350, &b, 10, 7, &none).is_some());
    }

    #[test]
    fn a_silent_tree_is_killed_unless_a_member_waits_on_a_lease() {
        let procs = [p(10, 1, 5, false), p(11, 10, 0, false)];
        let mut st = State::new(0);
        let waiting = HashSet::from([11]);
        tick(&mut st, &TH, 0, &procs, 10, 0, &waiting);
        assert_eq!(
            tick(&mut st, &TH, 1000, &procs, 10, 0, &waiting),
            Some(Action::Spared)
        );
        assert_eq!(
            tick(&mut st, &TH, 1000, &procs, 10, 0, &HashSet::new()),
            Some(Action::Kill(1000))
        );
    }

    #[test]
    fn a_waiting_tree_is_still_continued_when_stopped() {
        let procs = [p(10, 1, 5, false), p(11, 10, 0, true)];
        let mut st = State::new(0);
        let waiting = HashSet::from([11]);
        tick(&mut st, &TH, 0, &procs, 10, 0, &waiting);
        assert_eq!(
            tick(&mut st, &TH, 1000, &procs, 10, 0, &waiting),
            Some(Action::Cont(vec![11]))
        );
    }

    #[test]
    fn zero_turns_a_rule_off() {
        let th = Thresholds::default();
        assert!(th.off());
        let procs = [p(10, 1, 5, false), p(11, 10, 0, true)];
        let mut st = State::new(0);
        tick(&mut st, &th, 0, &procs, 10, 0, &HashSet::new());
        assert_eq!(
            tick(&mut st, &th, 1_000_000, &procs, 10, 0, &HashSet::new()),
            None
        );
        let only_kill = Thresholds {
            cont_after_ms: 0,
            kill_after_ms: 300,
        };
        assert_eq!(
            tick(
                &mut st,
                &only_kill,
                1_000_000,
                &procs,
                10,
                0,
                &HashSet::new()
            ),
            Some(Action::Kill(1_000_000))
        );
    }

    #[test]
    fn the_reason_names_minutes() {
        assert_eq!(
            stalled_text(30 * 60_000),
            "stalled: no output or CPU for 30 min"
        );
        assert_eq!(stalled_text(2_000), "stalled: no output or CPU for 2 s");
    }
}
