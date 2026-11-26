// SPDX-License-Identifier: GPL-3.0-or-later

//! What each waiting request is held back by, as data (s527): the structured wait of every
//! task, the chain behind it, the leases that belong to no task, and the one line for the home
//! page when most of the waiting work is held by the same request or the same gate.
//!
//! Pure: it reads one scheduler plan and the ledger's snapshot and builds the view. The
//! sentence of every wait comes from the same [`mason_scheduler::Wait`], so the two cannot
//! disagree.

use mason_protocol::{QueueHold, WaitInfo, WaitLink, WaitParty};
use mason_scheduler::{explain, label_parts, since, Needs, Snapshot, Wait, WaitCause};
use std::collections::{BTreeMap, HashMap, HashSet};

/// The longest chain shown: this task waits for X, X waits for Y, ...
const CHAIN_LIMIT: usize = 6;

/// A group of waiting tasks needs more than this share of all of them to be "most".
const MIN_HELD: usize = 2;

/// What the broker knows about the requests that is not in the ledger.
pub struct Inputs<'a> {
    pub now: u64,
    pub snap: &'a Snapshot,
    pub waits: &'a BTreeMap<u64, Wait>,
    /// Request id to the task it is for (a task's step, or a lease opened for a task).
    pub task_of: &'a HashMap<u64, u64>,
    /// The requests that are tasks' own steps, by request id.
    pub step_tasks: &'a HashMap<u64, u64>,
    /// Requests that are leases (a `mason run`, another pipeline's work, a slot held outside
    /// the queue), with where each came from.
    pub leases: &'a HashMap<u64, String>,
}

/// Everything the views need about waiting, published together.
#[derive(Clone, Debug, Default)]
pub struct WaitsView {
    /// By task id, for each task whose step waits on the scheduler.
    pub tasks: BTreeMap<u64, WaitInfo>,
    /// Leases that belong to no task, held or waiting, oldest first.
    pub leases: Vec<WaitParty>,
    /// What most waiting work is held by, when one thing holds it.
    pub hold: Option<QueueHold>,
}

/// What a request asks for, as the monitor shows it.
pub fn needs_text(n: &Needs) -> String {
    let mut v: Vec<String> = vec![];
    if n.exclusive {
        v.push("exclusive".into());
    }
    v.extend(n.amounts.keys().cloned());
    v.join(", ")
}

struct Maker<'a> {
    i: &'a Inputs<'a>,
}

impl Maker<'_> {
    fn party(
        &self,
        id: u64,
        label: &str,
        since_ms: u64,
        needs: &Needs,
        holding: bool,
    ) -> WaitParty {
        let (desc, command) = label_parts(label);
        let task = self.i.task_of.get(&id).copied().unwrap_or(0);
        let origin = self.i.leases.get(&id);
        let lease = task == 0 && origin.is_some();
        WaitParty {
            label: desc,
            task,
            since_ms,
            lease,
            origin: if lease {
                origin.cloned().unwrap_or_default()
            } else {
                String::new()
            },
            command,
            needs: needs_text(needs),
            exclusive: needs.exclusive,
            holding,
        }
    }

    fn holder(&self, id: u64) -> Option<WaitParty> {
        let h = self.i.snap.holders.iter().find(|h| h.id == id)?;
        Some(self.party(h.id, &h.label, h.since, &h.needs, true))
    }

    fn waiter(&self, id: u64) -> Option<WaitParty> {
        let r = self.i.snap.waiting.iter().find(|r| r.id == id)?;
        Some(self.party(r.id, &r.label, r.since, &r.needs, false))
    }

    /// Who holds `w` back, as ids: its blocker, else its first holder.
    fn first_cause(&self, w: &Wait) -> Option<u64> {
        w.blocker
            .as_ref()
            .map(|b| b.id)
            .or_else(|| w.held_by.first().map(|h| h.id))
    }

    fn chain(&self, w: &Wait) -> Vec<WaitLink> {
        let mut out = vec![];
        let mut seen = HashSet::new();
        let mut next = self.first_cause(w);
        while let Some(id) = next {
            if out.len() >= CHAIN_LIMIT || !seen.insert(id) {
                break;
            }
            let own = self.i.waits.get(&id);
            let Some(party) = self.holder(id).or_else(|| self.waiter(id)) else {
                break;
            };
            out.push(WaitLink {
                party: Some(party),
                cause: own.map(|w| w.reason_code().to_string()).unwrap_or_default(),
                sentence: own.map(|w| explain(w, self.i.now)).unwrap_or_default(),
            });
            next = own.and_then(|w| self.first_cause(w));
        }
        out
    }

    fn info(&self, w: &Wait) -> WaitInfo {
        let blocker = w.blocker.as_ref().and_then(|b| self.waiter(b.id));
        let holders: Vec<WaitParty> = w.held_by.iter().filter_map(|h| self.holder(h.id)).collect();
        let blocked_by = blocker
            .as_ref()
            .or(holders.first())
            .map(|p| p.label.clone())
            .unwrap_or_default();
        WaitInfo {
            cause: w.reason_code().to_string(),
            resource: w.class(),
            position: w.position as u32,
            holders,
            blocker,
            chain: self.chain(w),
            sentence: explain(w, self.i.now),
            blocked_by,
        }
    }
}

/// How a gate holds the queue, in words with the numbers in them.
pub fn gate_line(c: &WaitCause) -> Option<String> {
    Some(match c {
        WaitCause::Cpu { busy_pct, max_pct } => {
            format!("CPU at {busy_pct}% (a compile starts below {max_pct}%)")
        }
        WaitCause::Settling { .. } => "the last compile is settling into the CPU reading".into(),
        WaitCause::DiskStall { slow_ms, max_ms } => format!(
            "build disk slow ({:.1} s to answer, limit {:.1} s)",
            *slow_ms as f64 / 1000.0,
            *max_ms as f64 / 1000.0
        ),
        WaitCause::Thermal {
            speed_pct,
            min_pct,
            hold,
        } => format!("too hot ({})", thermal_short(*speed_pct, *min_pct, *hold)),
        WaitCause::Policy { busy_pct, max_pct } => {
            format!("syspolicyd at {busy_pct}% (limit {max_pct}%)")
        }
        WaitCause::Disk { free_mb, floor_mb } => format!(
            "only {} GB of disk free (floor {} GB)",
            free_mb / 1024,
            floor_mb / 1024
        ),
        WaitCause::Memory { free_mb, floor_mb } => {
            format!("only {free_mb} MB of memory free ({floor_mb} MB needed)")
        }
        _ => return None,
    })
}

fn tasks_word(n: usize) -> String {
    if n == 1 {
        "1 task waits".into()
    } else {
        format!("{n} tasks wait")
    }
}

/// "#510's compile slot", "`cargo test` to finish": what a request waits for.
fn waits_for(holders: &[WaitParty]) -> String {
    let one = |p: &WaitParty| {
        if p.task > 0 {
            let what = match p.needs.split(", ").find(|n| *n != "exclusive") {
                Some(n) => format!("{n} slot"),
                None => "the machine".into(),
            };
            format!("#{}'s {what}", p.task)
        } else {
            format!("`{}` to finish", p.label)
        }
    };
    let shown: Vec<String> = holders.iter().take(2).map(one).collect();
    let more = holders.len().saturating_sub(2);
    let mut s = shown.join(" and ");
    if more > 0 {
        s.push_str(&format!(" and {more} more"));
    }
    s
}

/// The request holding the queue, in words: "an exclusive `mason run` (starpipe heavy, from
/// starpipe in /x/star, waiting 9 min)".
fn party_line(p: &WaitParty, now: u64) -> String {
    let age = since(now, p.since_ms);
    if p.lease {
        let mut inside = vec![p.label.clone()];
        if !p.origin.is_empty() {
            inside.push(format!("from {}", p.origin));
        }
        inside.push(format!("waiting {age}"));
        format!(
            "{} `mason run` ({})",
            if p.exclusive { "an exclusive" } else { "a" },
            inside.join(", ")
        )
    } else {
        format!(
            "{}'s {}request (waiting {age})",
            p.label,
            if p.exclusive { "exclusive " } else { "" }
        )
    }
}

/// The view of one plan.
pub fn build(i: &Inputs) -> WaitsView {
    let m = Maker { i };
    let mut view = WaitsView::default();
    for (rid, w) in i.waits {
        if let Some(t) = i.step_tasks.get(rid) {
            view.tasks.insert(*t, m.info(w));
        }
    }
    // Leases that belong to no task, held ones and waiting ones.
    let mut leases: Vec<WaitParty> = i
        .snap
        .holders
        .iter()
        .filter(|h| i.leases.contains_key(&h.id) && !i.task_of.contains_key(&h.id))
        .map(|h| m.party(h.id, &h.label, h.since, &h.needs, true))
        .chain(
            i.snap
                .waiting
                .iter()
                .filter(|r| i.leases.contains_key(&r.id) && !i.task_of.contains_key(&r.id))
                .map(|r| m.party(r.id, &r.label, r.since, &r.needs, false)),
        )
        .collect();
    leases.sort_by_key(|p| p.since_ms);
    view.leases = leases;
    view.hold = hold(i, &m);
    view
}

fn hold(i: &Inputs, m: &Maker) -> Option<QueueHold> {
    // The waiting tasks, with the request ids behind them.
    let tasks: Vec<(&u64, &Wait)> = i
        .waits
        .iter()
        .filter(|(rid, _)| i.step_tasks.contains_key(rid))
        .collect();
    let total = tasks.len();
    let most = |n: usize| n >= MIN_HELD && n * 2 > total;
    // Behind one exclusive request.
    let mut by_blocker: BTreeMap<u64, usize> = BTreeMap::new();
    for (_, w) in &tasks {
        if w.reason_code() == "behind-exclusive" {
            if let Some(b) = &w.blocker {
                *by_blocker.entry(b.id).or_insert(0) += 1;
            }
        }
    }
    if let Some((bid, n)) = by_blocker
        .iter()
        .max_by_key(|(id, n)| (**n, std::cmp::Reverse(**id)))
    {
        if most(*n) {
            let party = m.waiter(*bid)?;
            let holders: Vec<WaitParty> = i
                .waits
                .get(bid)
                .map(|w| w.held_by.iter().filter_map(|h| m.holder(h.id)).collect())
                .unwrap_or_default();
            let mut line = format!("Queue held: {} ", party_line(&party, i.now));
            if holders.is_empty() {
                line.push_str("is waiting for the machine");
            } else {
                line.push_str(&format!("is waiting for {}", waits_for(&holders)));
            }
            line.push_str(&format!("; {} behind it.", tasks_word(*n)));
            return Some(QueueHold {
                kind: "exclusive-request".into(),
                cause: "behind-exclusive".into(),
                summary: line,
                waiting: *n as u32,
                tasks: holders.iter().map(|h| h.task).filter(|t| *t > 0).collect(),
                party: Some(party),
                holders,
            });
        }
    }
    // Held by one gate.
    let mut by_gate: BTreeMap<&'static str, (usize, &WaitCause)> = BTreeMap::new();
    for (_, w) in &tasks {
        if gate_line(&w.cause).is_some() {
            let code = w.reason_code();
            by_gate.entry(code).or_insert((0, &w.cause)).0 += 1;
        }
    }
    let (code, (n, cause)) = by_gate
        .iter()
        .max_by_key(|(code, (n, _))| (*n, std::cmp::Reverse(**code)))?;
    if !most(*n) {
        return None;
    }
    Some(QueueHold {
        kind: "gate".into(),
        cause: (*code).into(),
        summary: format!(
            "Queue held: {}; {} behind it.",
            gate_line(cause)?,
            tasks_word(*n)
        ),
        waiting: *n as u32,
        ..Default::default()
    })
}
