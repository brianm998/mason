// SPDX-License-Identifier: GPL-3.0-or-later

//! `mason report` / `GET /report` (s404): throughput, send-backs, waits and interventions, read
//! once from the event log and the store tables. Everything is a count or a sum of recorded
//! rows and events; nothing is estimated. The event half (`Collector`) is fed one event at a
//! time so a test can drive it without a daemon; `build` joins it with the store rows.

use crate::daemon::{now_ms, Daemon};
use mason_core::introspect::{median, p90};
use mason_core::{EventKind, FailClass, Next, State, TaskId};
use mason_engine::{reduce, World};
use serde_json::{json, Value};
use std::collections::BTreeMap;

const DAY_MS: u64 = 86_400_000;

/// What the store tables add to the event log, for the window.
#[derive(Default)]
pub struct Rows {
    /// (kind, project) of each owner intervention in the window.
    pub interventions: Vec<(String, String)>,
    /// (task, at) of each lost step host in the window.
    pub host_losses: Vec<(u64, u64)>,
    /// (suite::test) of each flaky sighting in the window.
    pub flaky: Vec<String>,
    /// (project, scope) of each open debt.
    pub open_debts: Vec<(String, String)>,
}

#[derive(Default)]
pub struct Collector {
    w: World,
    from: u64,
    to: u64,
    /// task -> admitted at (first claim).
    claims: BTreeMap<u64, u64>,
    /// (day, project) -> landed
    landed: BTreeMap<(String, String), u64>,
    failed: BTreeMap<(String, String), u64>,
    lead_ms: BTreeMap<String, Vec<u64>>,
    /// (project, normalized reason) -> count
    send_backs: BTreeMap<(String, String), u64>,
    /// suite name -> send-backs naming it
    suites: BTreeMap<String, u64>,
    /// (reason, class) -> count
    failures: BTreeMap<(String, &'static str), u64>,
    /// class -> each wait's length in the window
    waits: BTreeMap<String, Vec<u64>>,
    /// task -> project name, as last seen
    projects: BTreeMap<u64, String>,
}

impl Collector {
    pub fn new(from: u64, to: u64) -> Self {
        Collector {
            w: World::new(),
            from,
            to,
            ..Default::default()
        }
    }

    fn project_of(&self, t: &TaskId) -> String {
        self.w
            .tasks
            .get(t)
            .and_then(|x| self.w.projects.get(&x.project))
            .map(|p| p.name.clone())
            .unwrap_or_default()
    }

    pub fn feed(&mut self, event: &mason_core::Event) {
        let at = event.at.0;
        let inw = at >= self.from && at < self.to;
        match &event.kind {
            EventKind::Admitted {
                task,
                entry: Some(_),
            } => {
                self.claims.entry(task.0).or_insert(at);
            }
            EventKind::StepSucceeded {
                task,
                next:
                    Some(Next {
                        state: State::Landed,
                        ..
                    }),
            }
            | EventKind::ClosedNoChange { task, .. } => {
                let p = self.project_of(task);
                if inw {
                    *self.landed.entry((day_of(at), p.clone())).or_default() += 1;
                    if let Some(c) = self.claims.get(&task.0) {
                        self.lead_ms
                            .entry(p)
                            .or_default()
                            .push(at.saturating_sub(*c));
                    }
                }
            }
            EventKind::SentBack { task, reason, .. } if inw => {
                let p = self.project_of(task);
                for s in suites_in(reason) {
                    *self.suites.entry(s).or_default() += 1;
                }
                *self.send_backs.entry((p, normalize(reason))).or_default() += 1;
            }
            EventKind::StepFailed {
                task,
                reason,
                class,
            } if inw => {
                let p = self.project_of(task);
                *self.failed.entry((day_of(at), p)).or_default() += 1;
                let c = match class {
                    Some(FailClass::Code) => "code",
                    Some(FailClass::Outside) => "outside",
                    None => "unclassified",
                };
                *self.failures.entry((normalize(reason), c)).or_default() += 1;
            }
            _ => {}
        }
        if let Ok((next, _)) = reduce(&self.w, event) {
            let ended: Vec<mason_core::Wait> = self
                .w
                .waits
                .iter()
                .filter(|(task, wait)| next.waits.get(task).is_none_or(|n| n.since != wait.since))
                .map(|(_, wait)| wait.clone())
                .collect();
            for wait in &ended {
                self.note_wait(wait, at);
            }
            for (id, t) in &next.tasks {
                if let Some(p) = next.projects.get(&t.project) {
                    self.projects.insert(id.0, p.name.clone());
                }
            }
            self.w = next;
        }
    }

    fn note_wait(&mut self, wait: &mason_core::Wait, end: u64) {
        let (lo, hi) = (wait.since.0.max(self.from), end.min(self.to));
        if hi > lo {
            self.waits
                .entry(wait.class_name())
                .or_default()
                .push(hi - lo);
        }
    }

    /// Join with the store rows. Waits still going count up to the end of the window.
    pub fn finish(mut self, rows: &Rows, project: Option<&str>) -> Value {
        let open: Vec<_> = self.w.waits.values().cloned().collect();
        let to = self.to;
        for wait in &open {
            self.note_wait(wait, to);
        }
        let in_p = |p: &str| project.is_none_or(|x| x == p);
        let per_day = |m: &BTreeMap<(String, String), u64>| -> Value {
            let mut days: BTreeMap<&str, BTreeMap<&str, u64>> = BTreeMap::new();
            for ((d, p), n) in m.iter().filter(|((_, p), _)| in_p(p)) {
                days.entry(d).or_default().insert(p, *n);
            }
            json!(days)
        };
        let total = |m: &BTreeMap<(String, String), u64>| -> u64 {
            m.iter().filter(|((_, p), _)| in_p(p)).map(|(_, n)| n).sum()
        };
        let leads: Vec<u64> = self
            .lead_ms
            .iter()
            .filter(|(p, _)| in_p(p))
            .flat_map(|(_, v)| v.iter().copied())
            .collect();
        let mut sb: BTreeMap<&str, u64> = BTreeMap::new();
        for ((_, r), n) in self.send_backs.iter().filter(|((p, _), _)| in_p(p)) {
            *sb.entry(r).or_default() += n;
        }
        let sb_total: u64 = sb.values().sum();
        let mut fails: Vec<Value> = self
            .failures
            .iter()
            .map(|((r, c), n)| json!({"reason": r, "class": c, "count": n}))
            .collect();
        fails.sort_by_key(|v| std::cmp::Reverse(v["count"].as_u64()));
        let mut waits: Vec<Value> = self
            .waits
            .iter()
            .map(|(c, v)| {
                json!({"class": c, "count": v.len(), "total_ms": v.iter().sum::<u64>(),
                       "median_ms": median(v.clone())})
            })
            .collect();
        waits.sort_by_key(|v| std::cmp::Reverse(v["total_ms"].as_u64()));
        let mut iv: BTreeMap<&str, u64> = BTreeMap::new();
        for (k, _) in rows.interventions.iter().filter(|(_, p)| in_p(p)) {
            *iv.entry(k).or_default() += 1;
        }
        let losses = rows
            .host_losses
            .iter()
            .filter(|(t, _)| self.projects.get(t).is_some_and(|p| in_p(p)) || (project.is_none()))
            .count();
        let mut flaky: BTreeMap<&str, u64> = BTreeMap::new();
        for f in &rows.flaky {
            *flaky.entry(f).or_default() += 1;
        }
        let debts: Vec<Value> = rows
            .open_debts
            .iter()
            .filter(|(p, _)| in_p(p))
            .map(|(p, s)| json!({"project": p, "scope": s}))
            .collect();
        json!({
            "from": self.from, "to": self.to, "project": project,
            "landed": {"total": total(&self.landed), "per_day": per_day(&self.landed)},
            "failed": {"total": total(&self.failed), "per_day": per_day(&self.failed)},
            "lead_time_ms": {"median": median(leads.clone()), "p90": p90(leads.clone()),
                             "samples": leads.len()},
            "send_backs": {"total": sb_total, "by_reason": sb, "by_suite": self.suites},
            "failures": fails,
            "waits": waits,
            "interventions": {"total": iv.values().sum::<u64>(), "by_kind": iv},
            "host_losses": losses,
            "flaky": {"sightings": rows.flaky.len(), "by_test": flaky},
            "open_debts": debts,
        })
    }
}

/// `YYYY-MM-DD` (UTC) of a millisecond time.
pub fn day_of(ms: u64) -> String {
    let z = (ms / DAY_MS) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// The names in backticks in a reason: the suites that sent a task back.
fn suites_in(reason: &str) -> Vec<String> {
    reason
        .split('`')
        .skip(1)
        .step_by(2)
        .filter(|s| !s.is_empty() && !s.contains(' '))
        .map(String::from)
        .collect()
}

/// A reason with what varies taken out (names in backticks, numbers, ids), so equal causes
/// count together.
pub fn normalize(reason: &str) -> String {
    let mut out = String::new();
    let mut tick = false;
    let mut num = false;
    for ch in reason.chars().take_while(|c| *c != '\n') {
        if ch == '`' {
            if tick {
                out.push_str("_`");
            } else {
                out.push('`');
            }
            tick = !tick;
            continue;
        }
        if tick {
            continue;
        }
        if ch.is_ascii_digit() {
            if !num {
                out.push('N');
            }
            num = true;
            continue;
        }
        num = false;
        out.push(ch.to_ascii_lowercase());
    }
    let t = out.trim();
    t.chars().take(100).collect()
}

/// The report for the last `days` days, read once.
pub fn report(d: &Daemon, project: Option<&str>, days: u64) -> Value {
    let to = now_ms();
    let from = to.saturating_sub(days.max(1) * DAY_MS);
    let mut c = Collector::new(from, to);
    let mut after = 0u64;
    while let Ok(batch) = d.store.read_since(after, 5000) {
        if batch.is_empty() {
            break;
        }
        for se in batch {
            after = se.seq;
            c.feed(&se.event);
        }
    }
    let rows = Rows {
        interventions: d
            .store
            .interventions(from)
            .unwrap_or_default()
            .into_iter()
            .filter(|r| r.at < to && r.who != "pipeline" && r.kind != "refused_answer")
            .map(|r| (r.kind, r.project))
            .collect(),
        host_losses: d
            .store
            .host_losses_since(from)
            .unwrap_or_default()
            .into_iter()
            .filter(|r| r.lost_at_ms < to)
            .map(|r| (r.task, r.lost_at_ms))
            .collect(),
        flaky: d
            .store
            .flaky_since(from)
            .unwrap_or_default()
            .into_iter()
            .filter(|(_, _, at)| *at < to)
            .map(|(s, t, _)| format!("{s}::{t}"))
            .collect(),
        open_debts: d
            .store
            .debts(None, true)
            .unwrap_or_default()
            .into_iter()
            .map(|x| (x.project, x.scope))
            .collect(),
    };
    c.finish(&rows, project)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mason_core::{Entry, Priority, ProjectId, Route, Timestamp, TrustSource};

    fn ev(at: u64, kind: EventKind) -> mason_core::Event {
        mason_core::Event {
            at: Timestamp(at),
            kind,
        }
    }

    fn filed(at: u64, p: u64) -> mason_core::Event {
        ev(
            at,
            EventKind::TaskFiled {
                project: ProjectId(p),
                title: "t".into(),
                body: String::new(),
                priority: Priority::Normal,
                trust: TrustSource::Owner,
                explored: true,
                proposed: false,
                blockers: vec![],
                not_before: None,
            },
        )
    }

    fn admit(at: u64, t: u64) -> mason_core::Event {
        ev(
            at,
            EventKind::Admitted {
                task: TaskId(t),
                entry: Some(Entry {
                    def: "d".into(),
                    step: "develop".into(),
                    state: State::Developing,
                    route: Route::Explore,
                }),
            },
        )
    }

    fn step(at: u64, t: u64, to: State) -> mason_core::Event {
        ev(
            at,
            EventKind::StepSucceeded {
                task: TaskId(t),
                next: Some(Next {
                    step: String::new(),
                    state: to,
                }),
            },
        )
    }

    #[test]
    fn days_are_utc_dates() {
        assert_eq!(day_of(0), "1970-01-01");
        assert_eq!(day_of(1_763_078_400_000), "2025-11-14");
        assert_eq!(day_of(1_763_078_400_000 - 1), "2025-11-13");
    }

    #[test]
    fn reasons_are_normalized() {
        assert_eq!(
            normalize("suite `a-b` is red: 12 tests failed\nmore"),
            "suite `_` is red: N tests failed"
        );
        assert_eq!(suites_in("suite `x` and `y z` and `w`"), vec!["x", "w"]);
    }

    #[test]
    fn every_section_counts_a_small_synthetic_log() {
        let day1 = DAY_MS;
        let mut c = Collector::new(0, 3 * DAY_MS);
        let wstart = |at, t, class: &str| {
            ev(
                at,
                EventKind::WaitStarted {
                    task: TaskId(t),
                    on: "x".into(),
                    class: Some(class.into()),
                },
            )
        };
        let wend = |at, t| ev(at, EventKind::WaitEnded { task: TaskId(t) });
        let sent = |at, t, r: &str| {
            ev(
                at,
                EventKind::SentBack {
                    task: TaskId(t),
                    reason: r.into(),
                    to_step: "develop".into(),
                },
            )
        };
        let events = vec![
            ev(
                0,
                EventKind::ProjectRegistered {
                    project: ProjectId(1),
                    name: "p".into(),
                },
            ),
            ev(
                0,
                EventKind::ProjectRegistered {
                    project: ProjectId(2),
                    name: "q".into(),
                },
            ),
            filed(0, 1), // task 0 in p
            filed(0, 1), // task 1 in p
            filed(0, 2), // task 2 in q
            admit(1_000, 0),
            admit(1_000, 1),
            admit(1_000, 2),
            step(2_000, 0, State::Testing),
            step(2_500, 2, State::Testing),
            wstart(3_000, 0, "test-suite"),
            wend(13_000, 0),
            sent(14_000, 0, "suite `cargo-workspace` is red: 3 tests failed"),
            step(15_000, 0, State::Testing),
            wstart(16_000, 0, "test-suite"),
            wend(36_000, 0),
            sent(37_000, 0, "suite `cargo-workspace` is red: 5 tests failed"),
            step(38_000, 0, State::Testing),
            sent(39_000, 0, "merge conflict with the base"),
            step(40_000, 0, State::Testing),
            step(41_000, 0, State::Landed), // lead 40 s
            wstart(day1 + 1_000, 2, "owner"),
            wend(day1 + 4_000, 2),
            step(day1 + 5_000, 2, State::Landed), // lead 1 day + 4 s, day 2, q
            ev(
                day1 + 6_000,
                EventKind::StepFailed {
                    task: TaskId(1),
                    reason: "the step host was lost 3 times".into(),
                    class: Some(FailClass::Outside),
                },
            ),
        ];
        for e in events {
            c.feed(&e);
        }
        let rows = Rows {
            interventions: vec![
                ("answered".into(), "p".into()),
                ("answered".into(), "q".into()),
                ("retry".into(), "p".into()),
            ],
            host_losses: vec![(1, day1 + 6_000), (2, 100)],
            flaky: vec!["s::t".into(), "s::t".into(), "s::u".into()],
            open_debts: vec![("p".into(), "suite:x".into())],
        };
        let v = c.finish(&rows, None);
        assert_eq!(v["landed"]["total"], 2, "{v:#}");
        assert_eq!(v["landed"]["per_day"]["1970-01-01"]["p"], 1);
        assert_eq!(v["landed"]["per_day"]["1970-01-02"]["q"], 1);
        assert_eq!(v["failed"]["total"], 1);
        assert_eq!(v["failed"]["per_day"]["1970-01-02"]["p"], 1);
        assert_eq!(v["lead_time_ms"]["samples"], 2);
        assert_eq!(v["lead_time_ms"]["p90"], day1 + 4_000);
        assert_eq!(v["send_backs"]["total"], 3);
        assert_eq!(
            v["send_backs"]["by_reason"]["suite `_` is red: N tests failed"],
            2
        );
        assert_eq!(
            v["send_backs"]["by_reason"]["merge conflict with the base"],
            1
        );
        assert_eq!(v["send_backs"]["by_suite"]["cargo-workspace"], 2);
        assert_eq!(v["failures"][0]["count"], 1);
        assert_eq!(v["failures"][0]["class"], "outside");
        let w = v["waits"].as_array().unwrap();
        let ts = w.iter().find(|x| x["class"] == "test-suite").unwrap();
        assert_eq!(ts["count"], 2);
        assert_eq!(ts["total_ms"], 30_000);
        assert_eq!(ts["median_ms"], 15_000);
        let ow = w.iter().find(|x| x["class"] == "owner").unwrap();
        assert_eq!(ow["count"], 1);
        assert_eq!(v["interventions"]["total"], 3);
        assert_eq!(v["interventions"]["by_kind"]["answered"], 2);
        assert_eq!(v["host_losses"], 2);
        assert_eq!(v["flaky"]["sightings"], 3);
        assert_eq!(v["flaky"]["by_test"]["s::t"], 2);
        assert_eq!(v["open_debts"].as_array().unwrap().len(), 1);
    }
}
