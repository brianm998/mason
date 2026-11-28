// SPDX-License-Identifier: GPL-3.0-or-later
//! Runs that wait for a node (#1260): which node a nodes-only run asks the broker for, why a run
//! has none to go to, and the sentence the task page shows for it.
//!
//! A nodes-only run (`place::wait_nid`) used to hold nothing while no node had room and polled
//! the nodes itself, so it had no place in any line and the page said only what the placement
//! had decided. Now it asks the broker for a unit of the node [`pick`] names, in the same ranked
//! line as every other request for that node. A run for which no node can ever have room (none
//! on, none with the toolchain, the project not allowed) holds no unit, because a unit held for
//! a node it cannot use would starve the runs that can; it waits in the node executor and says
//! why with [`Why`].

use std::collections::BTreeMap;
use std::sync::Mutex;

/// One paired node as a run sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cand {
    pub nid: String,
    pub name: String,
    /// Accepting work.
    pub usable: bool,
    /// Has the toolchains the run needs.
    pub covers: bool,
    /// Jobs it says it has free.
    pub free: u32,
}

/// Why a run has no node to go to now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Why {
    /// `[nodes] enabled` is off.
    Disabled,
    /// The project may not use nodes (`projects`, `exclude`).
    NotAllowed,
    /// No node is paired.
    NoNodes,
    /// Every node is away or not taking work (the names).
    Off(Vec<String>),
    /// No node taking work has what the suite needs.
    NoToolchain,
    /// A node could take it, and every job slot there is in use.
    Full,
}

impl Why {
    pub fn text(&self) -> String {
        match self {
            Why::Disabled => "nodes are switched off ([nodes] enabled)".into(),
            Why::NotAllowed => {
                "this project may not run on nodes (project may not run elsewhere)".into()
            }
            Why::NoNodes => "node off: no node is paired".into(),
            Why::Off(n) => format!("node off: {} not taking work", n.join(", ")),
            Why::NoToolchain => {
                "toolchain missing: no node taking work has what the suite needs".into()
            }
            Why::Full => "node full: every job slot is in use".into(),
        }
    }
}

/// Why nothing is running, or `None` when a node has a job free for the run. Pure.
pub fn diagnose(enabled: bool, allowed: bool, cands: &[Cand]) -> Option<Why> {
    if !enabled {
        return Some(Why::Disabled);
    }
    if !allowed {
        return Some(Why::NotAllowed);
    }
    if cands.is_empty() {
        return Some(Why::NoNodes);
    }
    let usable: Vec<&Cand> = cands.iter().filter(|c| c.usable).collect();
    if usable.is_empty() {
        return Some(Why::Off(cands.iter().map(|c| c.name.clone()).collect()));
    }
    let covering: Vec<&Cand> = usable.into_iter().filter(|c| c.covers).collect();
    if covering.is_empty() {
        return Some(Why::NoToolchain);
    }
    if covering.iter().all(|c| c.free == 0) {
        return Some(Why::Full);
    }
    None
}

/// The node a nodes-only run asks the broker for: one taking work with the toolchain, the one
/// with most jobs free, then the lowest id so every call names the same node. `None` when no
/// node could ever take it: it then holds no unit and waits in the executor. Pure.
pub fn pick(cands: &[Cand]) -> Option<&Cand> {
    cands
        .iter()
        .filter(|c| c.usable && c.covers)
        .min_by(|a, b| b.free.cmp(&a.free).then(a.nid.cmp(&b.nid)))
}

/// The node `project`'s run of `suite_name` / `command` asks the broker for, if any.
pub fn pick_for(
    d: &crate::daemon::Daemon,
    project: &str,
    suite_name: &str,
    command: &str,
) -> Option<String> {
    let (enabled, allowed, cands) = crate::nodes::candidates_for(d, project, suite_name, command);
    if !enabled || !allowed {
        return None;
    }
    pick(&cands).map(|c| c.nid.clone())
}

// ---- who is running on a node, for holders and estimates ----

/// A run the node executor has in hand.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub task: u64,
    pub project: String,
    pub suite: String,
    pub since: u64,
}

static RUNS: Mutex<BTreeMap<String, Entry>> = Mutex::new(BTreeMap::new());

/// Takes the run out of the list when it goes out of scope.
pub struct Guard(String);

impl Drop for Guard {
    fn drop(&mut self) {
        RUNS.lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.0);
    }
}

/// The task a run id (`t12r3-check`) belongs to.
pub fn task_of_run(id: &str) -> Option<u64> {
    let rest = id.strip_prefix('t')?;
    let (n, _) = rest.split_once('r')?;
    n.parse().ok()
}

/// Note that run `id` is in the node executor, until the guard drops.
pub fn register(id: &str, project: &str, suite: &str, now: u64) -> Guard {
    RUNS.lock().unwrap_or_else(|e| e.into_inner()).insert(
        id.to_string(),
        Entry {
            task: task_of_run(id).unwrap_or(0),
            project: project.into(),
            suite: suite.into(),
            since: now,
        },
    );
    Guard(id.to_string())
}

/// The runs in the node executor now.
pub fn running() -> Vec<Entry> {
    RUNS.lock()
        .unwrap_or_else(|e| e.into_inner())
        .values()
        .cloned()
        .collect()
}

// ---- the sentence ----

/// One holder of a slot: how it reads, how long it has held it, and what is left of its usual run
/// time (`None`: no history).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Holder {
    pub label: String,
    pub since: u64,
    pub remaining_ms: Option<u64>,
}

/// What is left of a usual run time of `usual_ms` after `elapsed_ms`; `None` without history.
pub fn remaining(usual_ms: Option<u64>, elapsed_ms: u64) -> Option<u64> {
    usual_ms.map(|u| u.saturating_sub(elapsed_ms))
}

/// How long until the run at `position` (1 is next) gets a slot, from what is left of the
/// holders' usual run times and `each_ms`, how long a run ahead of it is expected to hold a slot.
/// The run at place `p` takes the `p`-th slot to free; past the holders, slots freed by runs
/// ahead are counted `each_ms` apart. `None` when any holder has no history (or none holds), or
/// when runs ahead outnumber the holders and `each_ms` is unknown. An estimate from past runs,
/// never a promise. Pure.
pub fn estimate_wait(
    remaining_ms: &[Option<u64>],
    position: usize,
    each_ms: Option<u64>,
) -> Option<u64> {
    let mut left: Vec<u64> = remaining_ms.iter().copied().collect::<Option<Vec<u64>>>()?;
    if left.is_empty() {
        return None;
    }
    left.sort_unstable();
    let n = left.len();
    let p = position.max(1);
    if p <= n {
        return Some(left[p - 1]);
    }
    let rounds = ((p - 1) / n) as u64;
    Some(left[(p - 1) % n] + rounds * each_ms?)
}

fn minutes(ms: u64) -> String {
    mason_scheduler::since(ms, 0)
}

/// The words for one part that is not running. `why` first, then who holds the slots and for how
/// long, the place in line and the expected wait. `now` is given so a test fixes the clock.
pub fn describe(
    why: &Why,
    holders: &[Holder],
    position: Option<usize>,
    wait_ms: Option<u64>,
    now: u64,
) -> String {
    let mut parts = vec![why.text()];
    if !holders.is_empty() {
        let shown: Vec<String> = holders
            .iter()
            .take(2)
            .map(|h| format!("{} for {}", h.label, mason_scheduler::since(now, h.since)))
            .collect();
        let mut s = format!("held by {}", shown.join(" and "));
        if holders.len() > 2 {
            s.push_str(&format!(" and {} more", holders.len() - 2));
        }
        parts.push(s);
    }
    if let Some(p) = position {
        parts.push(format!("{} in line", mason_scheduler::ordinal(p)));
    }
    parts.push(match wait_ms {
        Some(ms) => format!(
            "about {} to go, going by past run times (an estimate, not a promise)",
            minutes(ms)
        ),
        None => "no estimate".into(),
    });
    format!(
        "{}: {}",
        mason_core::health::NODE_WAIT_PREFIX,
        parts.join("; ")
    )
}

/// The holders of node slots now, from the runs in the executor, with what is left of their
/// usual times.
pub fn holders(d: &crate::daemon::Daemon, now: u64) -> Vec<Holder> {
    running()
        .into_iter()
        .map(|e| {
            let usual = d
                .store
                .suite_estimate(&e.project, &e.suite, "run")
                .ok()
                .flatten();
            Holder {
                label: if e.task > 0 {
                    format!("#{} {}", e.task, e.suite)
                } else {
                    e.suite.clone()
                },
                since: e.since,
                remaining_ms: remaining(usual, now.saturating_sub(e.since)),
            }
        })
        .collect()
}

/// The expected hold of one run ahead in line: the longest of the holders' usual times, when
/// every holder has history.
fn each_ms(d: &crate::daemon::Daemon) -> Option<u64> {
    let all = running();
    if all.is_empty() {
        return None;
    }
    all.iter()
        .map(|e| {
            d.store
                .suite_estimate(&e.project, &e.suite, "run")
                .ok()
                .flatten()
        })
        .collect::<Option<Vec<u64>>>()
        .and_then(|v| v.into_iter().max())
}

/// The sentence for a task that waits in the broker for a node's unit, from the broker's wait
/// (`info.resource` is a `node:` resource): the reason, who holds the slots and for how long,
/// the place in line and the expected wait. `None` for any other wait. A holder is looked up in
/// the executor's runs by its task, for its suite's past run times.
pub fn line_for(
    d: &crate::daemon::Daemon,
    info: &mason_protocol::WaitInfo,
    now: u64,
) -> Option<String> {
    crate::nodes::nid_of_resource(&info.resource)?;
    let runs = running();
    let hs: Vec<Holder> = info
        .holders
        .iter()
        .map(|p| {
            let e = runs.iter().find(|e| e.task > 0 && e.task == p.task);
            let usual = e.and_then(|e| {
                d.store
                    .suite_estimate(&e.project, &e.suite, "run")
                    .ok()
                    .flatten()
            });
            Holder {
                label: match e {
                    Some(e) => format!("#{} {}", e.task, e.suite),
                    None => p.label.clone(),
                },
                since: p.since_ms,
                remaining_ms: remaining(usual, now.saturating_sub(p.since_ms)),
            }
        })
        .collect();
    let position = (info.position as usize).max(1);
    let wait = estimate_wait(
        &hs.iter().map(|h| h.remaining_ms).collect::<Vec<_>>(),
        position,
        each_ms(d),
    );
    Some(describe(&Why::Full, &hs, Some(position), wait, now))
}

/// The words for each run of `task` that waits in the node executor with no unit held, and why.
static WAITS: Mutex<BTreeMap<String, (Why, u64)>> = Mutex::new(BTreeMap::new());

/// Say run `id` waits, and why (`None`: it does not).
pub fn set_waiting(id: &str, why: Option<(Why, u64)>) {
    let mut m = WAITS.lock().unwrap_or_else(|e| e.into_inner());
    match why {
        Some(w) => {
            m.insert(id.to_string(), w);
        }
        None => {
            m.remove(id);
        }
    }
}

/// The sentences of the runs of `task` waiting without a unit.
pub fn waiting_of(d: &crate::daemon::Daemon, task: u64, now: u64) -> Vec<String> {
    let mine: Vec<(Why, u64)> = WAITS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .filter(|(id, _)| task_of_run(id) == Some(task))
        .map(|(_, w)| w.clone())
        .collect();
    mine.into_iter()
        .map(|(why, _)| describe(&why, &holders(d, now), None, None, now))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(nid: &str, usable: bool, covers: bool, free: u32) -> Cand {
        Cand {
            nid: nid.into(),
            name: format!("node-{nid}"),
            usable,
            covers,
            free,
        }
    }

    #[test]
    fn the_reasons_are_told_apart() {
        assert_eq!(diagnose(false, true, &[]), Some(Why::Disabled));
        assert_eq!(
            diagnose(true, false, &[cand("a", true, true, 1)]),
            Some(Why::NotAllowed)
        );
        assert_eq!(diagnose(true, true, &[]), Some(Why::NoNodes));
        assert_eq!(
            diagnose(true, true, &[cand("a", false, true, 0)]),
            Some(Why::Off(vec!["node-a".into()]))
        );
        assert_eq!(
            diagnose(true, true, &[cand("a", true, false, 3)]),
            Some(Why::NoToolchain)
        );
        assert_eq!(
            diagnose(true, true, &[cand("a", true, true, 0)]),
            Some(Why::Full)
        );
        assert_eq!(diagnose(true, true, &[cand("a", true, true, 1)]), None);
    }

    #[test]
    fn the_pick_is_the_node_with_most_room_then_the_lowest_id() {
        let cs = [
            cand("b", true, true, 2),
            cand("a", true, true, 2),
            cand("c", true, true, 1),
            cand("z", true, false, 9),
            cand("y", false, true, 9),
        ];
        assert_eq!(pick(&cs).map(|c| c.nid.as_str()), Some("a"));
        assert_eq!(pick(&[cand("a", true, false, 1)]), None);
        // Full nodes are still picked: the run waits in line for them.
        assert_eq!(
            pick(&[cand("a", true, true, 0)]).map(|c| c.nid.as_str()),
            Some("a")
        );
    }

    #[test]
    fn the_estimate_comes_from_the_holders_and_those_ahead() {
        let r = [Some(60_000), Some(10_000), Some(30_000)];
        // Next in line gets the first slot to free.
        assert_eq!(estimate_wait(&r, 1, Some(100)), Some(10_000));
        assert_eq!(estimate_wait(&r, 3, Some(100)), Some(60_000));
        // Past the holders, one more round of a usual run.
        assert_eq!(estimate_wait(&r, 4, Some(5_000)), Some(15_000));
        assert_eq!(estimate_wait(&r, 4, None), None);
        // No history for one holder, or none holding: no estimate.
        assert_eq!(estimate_wait(&[Some(1), None], 1, Some(1)), None);
        assert_eq!(estimate_wait(&[], 1, Some(1)), None);
        assert_eq!(remaining(Some(10), 25), Some(0));
        assert_eq!(remaining(None, 25), None);
    }

    #[test]
    fn the_sentence_gives_reason_holders_place_and_estimate() {
        let hs = [Holder {
            label: "#1136 cargo-workspace".into(),
            since: 0,
            remaining_ms: Some(120_000),
        }];
        let s = describe(&Why::Full, &hs, Some(2), Some(120_000), 240_000);
        assert!(s.contains("node full"), "{s}");
        assert!(s.contains("held by #1136 cargo-workspace for 4 min"), "{s}");
        assert!(s.contains("2nd in line"), "{s}");
        assert!(s.contains("about 2 min to go"), "{s}");
        assert!(s.contains("not a promise"), "{s}");
        let none = describe(&Why::NoToolchain, &[], None, None, 0);
        assert!(none.contains("toolchain missing"), "{none}");
        assert!(none.ends_with("no estimate"), "{none}");
    }

    #[test]
    fn a_run_id_names_its_task() {
        assert_eq!(task_of_run("t1029r4-check"), Some(1029));
        assert_eq!(task_of_run("x1r2"), None);
    }

    #[test]
    fn registered_runs_are_listed_until_the_guard_drops() {
        let g = register("t99001r1-unit", "p", "unit", 5);
        assert!(running()
            .iter()
            .any(|e| e.task == 99001 && e.suite == "unit"));
        drop(g);
        assert!(!running().iter().any(|e| e.task == 99001));
    }

    /// A scratch broker with one node of `jobs` units, like the pool `nodes::set_capacity` makes.
    fn node_broker(dir: &std::path::Path, jobs: u32) -> std::sync::Arc<crate::sched::Broker> {
        let b = crate::sched::Broker::new(
            dir.join("locks"),
            dir.join("limits.kv"),
            &crate::sched::ResourceCfg::default(),
            2,
            crate::cpu::FakeCpu::new(0),
            &[],
        )
        .unwrap();
        b.set_pool_units(&crate::nodes::resource("n1"), jobs);
        b
    }

    /// Ask for the node's unit on a thread. `(name, sentence of the wait, lease)` is sent once
    /// granted; `queued` is told when the request is waiting.
    fn ask(
        b: &std::sync::Arc<crate::sched::Broker>,
        task: u64,
        name: &'static str,
        seq: u64,
        out: std::sync::mpsc::Sender<(&'static str, String, crate::sched::Lease)>,
        queued: std::sync::mpsc::Sender<()>,
    ) {
        let b = b.clone();
        std::thread::spawn(move || {
            let mut said = String::new();
            let lease = b
                .acquire(
                    mason_core::TaskId(task),
                    &format!("#{task}"),
                    mason_scheduler::Rank::new(false, 3, 2, seq, seq, false),
                    mason_scheduler::Needs::of(&[(crate::nodes::resource("n1").as_str(), 1)]),
                    &|| true,
                    &mut |t| {
                        said = t.to_string();
                        let _ = queued.send(());
                    },
                )
                .unwrap();
            let _ = out.send((name, said, lease));
        });
    }

    /// The node is full: a nodes-only suite is listed in line with its place and the holder, and
    /// when the slot frees it goes by rank, so work that came after it does not pass it.
    #[test]
    fn a_suite_waiting_for_a_full_node_is_in_line_and_goes_before_later_work() {
        let dir = tempfile::tempdir().unwrap();
        let b = node_broker(dir.path(), 1);
        let holder = b
            .acquire(
                mason_core::TaskId(1),
                "#1",
                mason_scheduler::Rank::new(false, 3, 2, 1, 1, false),
                mason_scheduler::Needs::of(&[(crate::nodes::resource("n1").as_str(), 1)]),
                &|| true,
                &mut |_| {},
            )
            .unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let (qtx, qrx) = std::sync::mpsc::channel();
        let wait = std::time::Duration::from_secs(60);
        ask(&b, 2, "suite", 2, tx.clone(), qtx.clone());
        qrx.recv_timeout(wait).unwrap();
        ask(&b, 3, "later check", 3, tx.clone(), qtx.clone());
        qrx.recv_timeout(wait).unwrap();
        drop(holder);
        let mut order = vec![];
        let mut texts = vec![];
        for _ in 0..2 {
            let (name, said, lease) = rx.recv_timeout(wait).unwrap();
            order.push(name);
            texts.push(said);
            drop(lease);
        }
        assert_eq!(order, ["suite", "later check"]);
        assert!(texts[0].contains("1st in line"), "{texts:?}");
        assert!(texts[0].contains("held by #1"), "{texts:?}");
        assert!(texts[1].contains("2nd in line"), "{texts:?}");
    }

    /// The pool never reads more units than the node has jobs (the `7/7` on a six-job node).
    #[test]
    fn the_pool_is_never_more_than_the_nodes_own_jobs() {
        use crate::nodes::capacity_units;
        // Six leased, five running at the node, one not started: free 1 + held 6 was 7.
        assert_eq!(capacity_units(1, 6, 6, true), 6);
        // Nothing held: what is free.
        assert_eq!(capacity_units(4, 0, 6, true), 4);
        // Another coordinator's jobs take slots: free is what is left.
        assert_eq!(capacity_units(2, 1, 6, true), 3);
        // A node that does not say its max is not capped; one that is off keeps what is held.
        assert_eq!(capacity_units(1, 6, 0, true), 7);
        assert_eq!(capacity_units(3, 2, 6, false), 2);
        // Held units are never taken away by a lower max.
        assert_eq!(capacity_units(0, 8, 6, true), 8);
    }
}
