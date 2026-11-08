// SPDX-License-Identifier: GPL-3.0-or-later

//! The daemon's half of live step progress: one [`Progress`] record per
//! running step run, fed with the output of its step host as it arrives.
//!
//! Records live in memory, are written to the store every few seconds (so a
//! restart shows the same figures and carries on from there), and are
//! dropped when the run ends. A run that succeeds leaves its final counts in
//! `step_history`, which gives the typical figures offered as `history`
//! totals. Changes are pushed on [`Daemon::progress_bus`], coalesced by a
//! per-run [`Throttle`]: one a second for output alone, at once for anything
//! else. A ticker flushes the sends that came due and does the writing.

use crate::daemon::{now_ms, Daemon};
use mason_core::progress::{
    middle_half, stale_after, Change, Progress, Push, Source, SubProgress, Throttle, TodoItem,
};
use mason_core::*;
use mason_protocol::{CellState, ProgressCell, ProgressSource, ProgressSub, TodoItem as PbTodo};
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How often the ticker looks for sends that came due.
pub const TICK: Duration = Duration::from_millis(250);
/// A live record is written to the store at most this often.
pub const PERSIST_MS: u64 = 3000;

/// A change of one run's record, as the sockets get it.
#[derive(Clone, Debug)]
pub struct Update {
    pub task: u64,
    pub project: u64,
    pub run: u64,
    pub cell: ProgressCell,
    pub now_ms: u64,
    /// Whole output lines since the last push (at most 40).
    pub log: Vec<String>,
}

struct Live {
    task: TaskId,
    project: ProjectId,
    p: Progress,
    th: Throttle,
    persisted_ms: u64,
    dirty: bool,
}

/// The records of the runs in progress.
#[derive(Default)]
pub struct Book {
    live: Mutex<HashMap<u64, Live>>,
}

impl Book {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<u64, Live>> {
        self.live.lock().unwrap_or_else(|e| e.into_inner())
    }
}

fn source(s: Source) -> ProgressSource {
    match s {
        Source::None => ProgressSource::SourceNone,
        Source::Reported => ProgressSource::SourceReported,
        Source::Counted => ProgressSource::SourceCounted,
        Source::History => ProgressSource::SourceHistory,
        Source::Expected => ProgressSource::SourceExpected,
        Source::Minimum => ProgressSource::SourceMinimum,
    }
}

/// The step's own count as the sum of its sub-counts (a test step's suites): the
/// total only when every sub-count has one.
pub fn rollup(p: &mut Progress) {
    // A landing batch counts its own phases; its sub-counts (suites, checks) are beside them.
    if p.subs.is_empty() || p.unit == "phases" {
        return;
    }
    let unit = p.subs[0].unit.clone();
    p.unit = unit;
    p.done = mason_core::progress::Figure::counted(p.subs.iter().map(|s| s.done.value).sum());
    if p.subs.iter().all(|s| s.total.is_real()) {
        let source = if p.subs.iter().any(|s| s.total.source == Source::Minimum) {
            Source::Minimum
        } else if p.subs.iter().any(|s| s.total.source == Source::Reported) {
            Source::Reported
        } else {
            Source::Expected
        };
        p.total = mason_core::progress::Figure {
            value: p.subs.iter().map(|s| s.total.value).sum(),
            source,
        };
    } else {
        p.total = mason_core::progress::Figure::default();
    }
}

/// What one line of a progress file or `step.progress` says: a JSON object with any of
/// `done`, `total`, `unit`, `message`; any other non-blank line is the message. None
/// for a blank line.
pub fn parse_report_line(line: &str) -> Option<mason_core::progress::Report> {
    let t = line.trim();
    if t.is_empty() {
        return None;
    }
    if t.starts_with('{') {
        if let Some(r) = serde_json::from_str::<serde_json::Value>(t)
            .ok()
            .and_then(|v| report_of(&v))
        {
            return Some(r);
        }
    }
    Some(mason_core::progress::Report {
        message: Some(t.to_string()),
        ..Default::default()
    })
}

/// The params of a `step.progress` notification, or None when malformed or empty.
pub fn parse_step_progress(v: &serde_json::Value) -> Option<mason_core::progress::Report> {
    let r = report_of(v)?;
    (r.done.is_some() || r.total.is_some() || r.unit.is_some() || r.message.is_some()).then_some(r)
}

fn report_of(v: &serde_json::Value) -> Option<mason_core::progress::Report> {
    let o = v.as_object()?;
    let num = |k: &str| match o.get(k) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(x) => x.as_u64().map(Some).ok_or(()),
    };
    let text = |k: &str| match o.get(k) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(x) => x.as_str().map(|s| Some(s.to_string())).ok_or(()),
    };
    Some(mason_core::progress::Report {
        done: num("done").ok()?,
        total: num("total").ok()?,
        unit: text("unit").ok()?,
        message: text("message").ok()?,
    })
}

/// Follows a progress file while a step runs: every whole line appended is applied
/// by `apply`. Stops (after one last read) when dropped.
pub struct FileTail {
    stop: Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl FileTail {
    pub fn start(
        path: std::path::PathBuf,
        apply: impl Fn(mason_core::progress::Report) + Send + 'static,
    ) -> FileTail {
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = stop.clone();
        let thread = std::thread::spawn(move || {
            let mut off = 0u64;
            let mut tail: Vec<u8> = vec![];
            loop {
                let last = flag.load(std::sync::atomic::Ordering::SeqCst);
                if let Ok(mut f) = std::fs::File::open(&path) {
                    if f.metadata().map_or(0, |m| m.len()) < off {
                        off = 0; // truncated: start again
                        tail.clear();
                    }
                    if f.seek(SeekFrom::Start(off)).is_ok() {
                        let mut buf = vec![];
                        if let Ok(n) = f.read_to_end(&mut buf) {
                            off += n as u64;
                            tail.extend_from_slice(&buf);
                        }
                    }
                }
                while let Some(i) = tail.iter().position(|b| *b == b'\n') {
                    let line: Vec<u8> = tail.drain(..=i).collect();
                    if let Some(r) = parse_report_line(&String::from_utf8_lossy(&line)) {
                        apply(r);
                    }
                }
                if last {
                    // An unfinished last line counts once the writer is done.
                    if let Some(r) = parse_report_line(&String::from_utf8_lossy(&tail)) {
                        apply(r);
                    }
                    return;
                }
                std::thread::sleep(TICK);
            }
        });
        FileTail {
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for FileTail {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// The record as a view cell.
pub fn cell_of(p: &Progress, state: CellState, label: &str) -> ProgressCell {
    let n = |v: u64| v.min(u32::MAX as u64) as u32;
    ProgressCell {
        step: p.step.clone(),
        state: state as i32,
        label: label.to_string(),
        done: n(p.done.value),
        total: n(p.total.value),
        unit: p.unit.clone(),
        done_source: source(p.done.source) as i32,
        total_source: source(p.total.source) as i32,
        message: p.message.clone(),
        lines: p.output.lines,
        bytes: p.output.bytes,
        last_output_ms: p.output.last_output_ms,
        started_ms: p.started_ms,
        updated_at_ms: p.updated_at_ms,
        run: p.run,
        subs: p
            .subs
            .iter()
            .map(|s| ProgressSub {
                name: s.name.clone(),
                unit: s.unit.clone(),
                done: n(s.done.value),
                total: n(s.total.value),
                done_source: source(s.done.source) as i32,
                total_source: source(s.total.source) as i32,
            })
            .collect(),
        todo: p
            .todo
            .iter()
            .map(|t| PbTodo {
                text: t.text.clone(),
                status: t.status.clone(),
            })
            .collect(),
        total_low: n(p.total_low),
        history_samples: p.history_samples,
        stale_after_ms: p.stale_after_ms,
    }
}

impl Daemon {
    fn progress_update(&self, l: &mut Live, state: CellState, now: u64) -> Update {
        Update {
            task: l.task.0,
            project: l.project.0,
            run: l.p.run,
            cell: cell_of(&l.p, state, "running"),
            now_ms: now,
            log: std::mem::take(&mut l.p.pending_log),
        }
    }

    fn progress_publish(&self, u: Update) {
        let _ = self.progress_bus.send(u);
    }

    /// A step run starts (or starts again after a restart): open its record,
    /// from the store if it left one, and offer the typical total.
    pub fn progress_begin(&self, task: TaskId, run: StepRunId) {
        let Ok(w) = self.world() else { return };
        let (Some(t), Some(r)) = (w.tasks.get(&task), w.step_runs.get(&run)) else {
            return;
        };
        let stored = self
            .store
            .progress_rows(Some(task.0))
            .unwrap_or_default()
            .into_iter()
            .find(|(_, p)| p.run == run.0)
            .map(|(_, p)| p);
        let resumed = stored.is_some();
        let mut p = stored.unwrap_or_else(|| Progress::new(run.0, &r.name, r.started_at.0));
        let project = self.project(t.project).map_or("", |c| c.id.as_str());
        let samples: Vec<u64> = self
            .store
            .history_samples(project, &r.name)
            .unwrap_or_default()
            .into_iter()
            .map(|(done, _)| done)
            .collect();
        p.offer_history_total(
            middle_half(&samples).filter(|(_, hi)| *hi > 0),
            samples.len(),
        );
        let gaps = self
            .store
            .history_gaps(project, &r.name)
            .unwrap_or_default();
        p.stale_after_ms = stale_after(&gaps);
        let now = now_ms();
        let l = Live {
            task,
            project: t.project,
            p,
            th: Throttle::new(now),
            persisted_ms: if resumed { now } else { 0 },
            dirty: !resumed,
        };
        self.progress.lock().insert(run.0, l);
    }

    /// Demo only: put a ready-made record in the book, as if its step were running.
    /// Nothing is pushed or stored; the views carry it.
    pub fn progress_seed(&self, task: TaskId, project: ProjectId, p: Progress) {
        let l = Live {
            task,
            project,
            p,
            th: Throttle::new(now_ms()),
            persisted_ms: now_ms(),
            dirty: false,
        };
        self.progress.lock().insert(l.p.run, l);
    }

    /// Output of a run's step host, at an absolute offset into the stream.
    pub fn progress_observe(&self, run: StepRunId, stream: &str, offset: u64, bytes: &[u8]) {
        self.progress_observe_at(run, stream, offset, bytes, now_ms());
    }

    fn progress_observe_at(
        &self,
        run: StepRunId,
        stream: &str,
        offset: u64,
        bytes: &[u8],
        at: u64,
    ) {
        let now = now_ms();
        let update = {
            let mut book = self.progress.lock();
            let Some(l) = book.get_mut(&run.0) else {
                return;
            };
            if l.p.output(stream, offset, bytes, at) == Change::None {
                return;
            }
            l.dirty = true;
            match l.th.note(now, false) {
                Push::Now => Some(self.progress_update(l, CellState::Part, now)),
                Push::At(_) | Push::Wait => None,
            }
        };
        if let Some(u) = update {
            self.progress_publish(u);
        }
    }

    /// A step run that is resumed after a restart: count what its log already
    /// holds beyond what the stored record had counted.
    pub fn progress_catch_up(&self, run: StepRunId, stream: &str, log: &Path) {
        let Ok(mut f) = std::fs::File::open(log) else {
            return;
        };
        let at = f
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or_else(now_ms, |d| d.as_millis() as u64);
        let mut off = self
            .progress
            .lock()
            .get(&run.0)
            .and_then(|l| l.p.marks.get(stream).copied())
            .unwrap_or(0);
        if f.seek(SeekFrom::Start(off)).is_err() {
            return;
        }
        let mut buf = vec![0u8; 64 * 1024];
        while let Ok(n) = f.read(&mut buf) {
            if n == 0 {
                break;
            }
            self.progress_observe_at(run, stream, off, &buf[..n], at);
            off += n as u64;
        }
    }

    /// A step says how far it is, what it is doing, or how many there are:
    /// pushed at once when something changed.
    pub fn progress_report(&self, run: StepRunId, f: impl FnOnce(&mut Progress, u64) -> Change) {
        let now = now_ms();
        let update = {
            let mut book = self.progress.lock();
            let Some(l) = book.get_mut(&run.0) else {
                return;
            };
            let change = f(&mut l.p, now);
            if change == Change::None {
                return;
            }
            l.dirty = true;
            match l.th.note(now, change == Change::Urgent) {
                Push::Now => Some(self.progress_update(l, CellState::Part, now)),
                Push::At(_) | Push::Wait => None,
            }
        };
        if let Some(u) = update {
            self.progress_publish(u);
        }
    }

    /// Replace the sub-counts of a run (a step with several counts).
    pub fn progress_subs(&self, run: StepRunId, subs: Vec<SubProgress>) {
        self.progress_report(run, |p, now| {
            if p.subs == subs {
                return Change::None;
            }
            p.subs = subs;
            p.updated_at_ms = now;
            Change::Urgent
        });
    }

    /// Change one named sub-count of a run (created if missing), then bring the step's own
    /// figures up to the sum of its sub-counts. A count that only moved is output-grade;
    /// a changed total or message is sent at once.
    pub fn progress_sub(
        &self,
        run: StepRunId,
        name: &str,
        message: Option<String>,
        f: impl FnOnce(&mut SubProgress),
    ) {
        self.progress_report(run, |p, now| {
            let i = match p.subs.iter().position(|s| s.name == name) {
                Some(i) => i,
                None => {
                    p.subs.push(SubProgress {
                        name: name.to_string(),
                        ..SubProgress::default()
                    });
                    p.subs.len() - 1
                }
            };
            let before = p.clone();
            f(&mut p.subs[i]);
            rollup(p);
            let mut change = if p.subs != before.subs {
                if p.subs[i].total != before.subs.get(i).map(|s| s.total).unwrap_or_default()
                    || before.subs.get(i).is_none()
                {
                    Change::Urgent
                } else {
                    Change::Output
                }
            } else {
                Change::None
            };
            if let Some(m) = message {
                if p.set_message(&m, now) == Change::Urgent {
                    change = Change::Urgent;
                }
            }
            if change != Change::None {
                p.updated_at_ms = now;
            }
            change
        });
    }

    /// Replace the agent's todo list of a run.
    pub fn progress_todo(&self, run: StepRunId, todo: Vec<TodoItem>) {
        self.progress_report(run, |p, now| {
            if p.todo == todo {
                return Change::None;
            }
            p.todo = todo;
            p.updated_at_ms = now;
            Change::Urgent
        });
    }

    /// The live record of a run, if it is in progress.
    pub fn progress_of(&self, run: u64) -> Option<Progress> {
        self.progress.lock().get(&run).map(|l| l.p.clone())
    }

    /// The live records of a task's runs.
    pub fn progress_of_task(&self, task: TaskId) -> Vec<Progress> {
        let mut v: Vec<Progress> = self
            .progress
            .lock()
            .values()
            .filter(|l| l.task == task)
            .map(|l| l.p.clone())
            .collect();
        v.sort_by_key(|p| p.run);
        v
    }

    /// The job for a run is over. A success keeps its final counts for the
    /// typical figures; any outcome drops the live record. A run that is
    /// still open (the daemon is stopping) keeps its stored one.
    pub fn progress_finish(&self, run: StepRunId) {
        let Some(mut l) = self.progress.lock().remove(&run.0) else {
            return;
        };
        let now = now_ms();
        let outcome = self
            .world()
            .ok()
            .and_then(|w| w.step_runs.get(&run).map(|r| r.outcome));
        if outcome == Some(None) {
            // Still open: leave what is stored, brought up to date.
            let _ = self.store.put_progress(l.task.0, &l.p);
            return;
        }
        let succeeded = outcome == Some(Some(StepOutcome::Succeeded));
        if succeeded {
            let project = self.project(l.project).map_or("", |c| c.id.as_str());
            let _ = self.store.record_history(
                project,
                l.task.0,
                run.0,
                &l.p.step,
                l.p.done.value,
                l.p.output.lines,
                l.p.max_gap_ms,
                now,
            );
        }
        let _ = self.store.drop_progress(run.0);
        let state = if succeeded {
            CellState::Filled
        } else {
            CellState::Empty
        };
        let u = self.progress_update(&mut l, state, now);
        self.progress_publish(u);
    }

    /// Flush the sends that came due and write the records that are old.
    pub fn progress_tick(&self) {
        let now = now_ms();
        let mut updates = vec![];
        let mut writes: Vec<(u64, Progress)> = vec![];
        {
            let mut book = self.progress.lock();
            for l in book.values_mut() {
                if l.th.take_due(now) {
                    updates.push(self.progress_update(l, CellState::Part, now));
                }
                if l.dirty && now.saturating_sub(l.persisted_ms) >= PERSIST_MS {
                    l.dirty = false;
                    l.persisted_ms = now;
                    writes.push((l.task.0, l.p.clone()));
                }
            }
        }
        for u in updates {
            self.progress_publish(u);
        }
        for (task, p) in writes {
            let _ = self.store.put_progress(task, &p);
        }
    }

    /// Drop stored records of runs that are not open (a crash can leave them).
    pub(crate) fn progress_cleanup(&self) {
        let Ok(w) = self.world() else { return };
        let open: Vec<u64> = w
            .step_runs
            .values()
            .filter(|r| r.outcome.is_none())
            .map(|r| r.id.0)
            .collect();
        let _ = self.store.drop_progress_except(open);
    }

    pub(crate) fn spawn_progress_ticker(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        let _ = std::thread::Builder::new()
            .name("progress-ticker".into())
            .spawn(move || loop {
                std::thread::sleep(TICK);
                let Some(d) = weak.upgrade() else { break };
                d.progress_tick();
            });
    }
}
