// SPDX-License-Identifier: GPL-3.0-or-later

//! What a developing task's `run_check` calls look like to the owner (pure: the daemon
//! gives the clock). Each call is one [`CheckEntry`] in the develop run's record: its scope,
//! the phase it is in (waiting for a slot, building, running, done), how long it waited and
//! ran, and its result. The latest tool summary of the agent sits beside them, so a quiet
//! agent is told apart from one that waits on a check.
//!
//! Counts are not kept here: they are the run's sub-count named `check <suite>`, fed by the
//! same `LiveCount` as the test step's. They are for display; the verdict is the full parse.

use crate::progress::Progress;

/// A tool call older than this, with no check running, reads as idle.
pub const IDLE_AFTER_MS: u64 = 120_000;

/// Where a check is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CheckPhase {
    #[default]
    Waiting,
    Building,
    Running,
    Done,
}

impl CheckPhase {
    pub fn name(self) -> &'static str {
        match self {
            CheckPhase::Waiting => "waiting",
            CheckPhase::Building => "building",
            CheckPhase::Running => "running",
            CheckPhase::Done => "done",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct CheckEntry {
    /// 1-based, in this develop run.
    pub n: u32,
    /// "full", "3 packages", "tests: a, b".
    pub scope: String,
    pub phase: CheckPhase,
    pub started_ms: u64,
    /// When the current phase began.
    pub phase_ms: u64,
    /// Time spent waiting for slots, over finished stretches.
    pub waited_ms: u64,
    /// Time spent building and running, over finished stretches.
    pub ran_ms: u64,
    /// The broker's wait text while waiting (empty otherwise).
    pub wait: String,
    /// The test executable cargo (or the parallel runner) is on, when known.
    pub current: String,
    /// The longest this suite usually runs, from history (0: none known).
    pub usual_ms: u64,
    /// "" while going; "green", "red" or "ended" once done.
    pub result: String,
    pub failed: u32,
    /// The first failing tests (at most [`KEEP_FAILING`]).
    pub failing: Vec<String>,
}

pub const KEEP_FAILING: usize = 3;

/// "45 s", "9 min", "1 h 12 min".
pub fn dur(ms: u64) -> String {
    let s = ms / 1000;
    if s < 60 {
        format!("{s} s")
    } else if s < 3600 {
        format!("{} min", (s + 30) / 60)
    } else {
        let m = (s + 30) / 60;
        format!("{} h {} min", m / 60, m % 60)
    }
}

impl CheckEntry {
    fn close_phase(&mut self, now: u64) {
        let spent = now.saturating_sub(self.phase_ms);
        match self.phase {
            CheckPhase::Waiting => self.waited_ms += spent,
            CheckPhase::Building | CheckPhase::Running => self.ran_ms += spent,
            CheckPhase::Done => {}
        }
        self.phase_ms = now;
    }

    /// Move to `phase` (not out of Done).
    pub fn enter(&mut self, phase: CheckPhase, now: u64) {
        if self.phase == CheckPhase::Done || self.phase == phase {
            return;
        }
        self.close_phase(now);
        self.phase = phase;
        if phase != CheckPhase::Waiting {
            self.wait.clear();
        }
    }

    pub fn finish(&mut self, result: &str, failing: &[String], now: u64) {
        if self.phase == CheckPhase::Done {
            return;
        }
        self.close_phase(now);
        self.phase = CheckPhase::Done;
        self.wait.clear();
        self.current.clear();
        self.result = result.to_string();
        self.failed = failing.len() as u32;
        self.failing = failing.iter().take(KEEP_FAILING).cloned().collect();
    }

    /// Time in the phase it is in now, plus what earlier stretches took.
    fn waited_now(&self, now: u64) -> u64 {
        self.waited_ms
            + if self.phase == CheckPhase::Waiting {
                now.saturating_sub(self.phase_ms)
            } else {
                0
            }
    }

    fn ran_now(&self, now: u64) -> u64 {
        self.ran_ms
            + if matches!(self.phase, CheckPhase::Building | CheckPhase::Running) {
                now.saturating_sub(self.phase_ms)
            } else {
                0
            }
    }

    fn failed_text(&self) -> String {
        let mut s = format!("{} failed", self.failed);
        if !self.failing.is_empty() {
            s += &format!(" ({})", self.failing.join(", "));
        }
        s
    }
}

impl Progress {
    /// The check entry of this call, started now (scope text as the agent asked).
    pub fn check_start(&mut self, scope: &str, now: u64) -> usize {
        let n = self.checks.len() as u32 + 1;
        self.checks.push(CheckEntry {
            n,
            scope: scope.to_string(),
            started_ms: now,
            phase_ms: now,
            ..CheckEntry::default()
        });
        self.updated_at_ms = now;
        self.checks.len() - 1
    }

    /// The sum of the counts of the sub-counts named `check …`: "212 of 340 tests".
    fn check_counts(&self) -> Option<(u64, Option<u64>, String)> {
        let subs: Vec<_> = self
            .subs
            .iter()
            .filter(|s| s.name.starts_with("check "))
            .collect();
        let first = subs.first()?;
        let done: u64 = subs.iter().map(|s| s.done.value).sum();
        let total = subs
            .iter()
            .all(|s| s.total.is_real())
            .then(|| subs.iter().map(|s| s.total.value).sum());
        Some((done, total, first.unit.clone()))
    }

    /// The check as one line for the task page, with its times.
    pub fn check_line(&self, i: usize, now: u64) -> String {
        let Some(c) = self.checks.get(i) else {
            return String::new();
        };
        let head = format!("check {}", c.n);
        match c.phase {
            CheckPhase::Waiting => {
                let w = dur(c.waited_now(now));
                if c.wait.is_empty() {
                    format!("{head}: waiting for a slot, {w}")
                } else {
                    format!("{head}: {}, {w}", c.wait)
                }
            }
            CheckPhase::Building => format!("{head}: building, {}", dur(c.ran_now(now))),
            CheckPhase::Running => {
                let mut s = format!("{head}: running");
                if let Some((d, t, u)) = self.check_counts() {
                    match t {
                        Some(t) if t > 0 => s += &format!(", {d} of {t} {u}"),
                        _ => s += &format!(", {d} {u}"),
                    }
                }
                if !c.current.is_empty() {
                    s += &format!(", in {}", c.current);
                }
                s += &format!(", {}", dur(c.ran_now(now)));
                if c.usual_ms > 0 {
                    s += &format!(" (usually up to {})", dur(c.usual_ms));
                }
                s
            }
            CheckPhase::Done => {
                let res = match c.result.as_str() {
                    "red" if c.failed > 0 => format!("red, {}", c.failed_text()),
                    r => r.to_string(),
                };
                format!(
                    "{head} ({}): waited {}, ran {}, {res}",
                    c.scope,
                    dur(c.waited_ms),
                    dur(c.ran_ms)
                )
            }
        }
    }

    /// The latest check, short, for a list row: "check: 212/340 tests, 9 min".
    pub fn check_short(&self, now: u64) -> String {
        let Some(c) = self.checks.last() else {
            return String::new();
        };
        match c.phase {
            CheckPhase::Waiting => {
                let w = if c.wait.is_empty() {
                    "waiting for a slot"
                } else {
                    c.wait.as_str()
                };
                format!("check: {w}")
            }
            CheckPhase::Building => format!("check: building, {}", dur(c.ran_now(now))),
            CheckPhase::Running => match self.check_counts() {
                Some((d, Some(t), u)) if t > 0 => {
                    format!("check: {d}/{t} {u}, {}", dur(c.ran_now(now)))
                }
                _ => format!("check: running, {}", dur(c.ran_now(now))),
            },
            CheckPhase::Done => match (c.result.as_str(), c.failed) {
                ("red", f) if f > 0 => format!("check {}: red, {f} failed", c.n),
                (r, _) => format!("check {}: {r}", c.n),
            },
        }
    }

    /// Is a check of this run not yet done?
    pub fn check_open(&self) -> Option<CheckPhase> {
        self.checks
            .last()
            .map(|c| c.phase)
            .filter(|p| *p != CheckPhase::Done)
    }

    /// What a developing task is doing, for rows and the task page: the open check first,
    /// then the agent's latest tool call with its age; plainly idle when neither is going.
    pub fn doing_text(&self, now: u64) -> String {
        let tool = if self.tool.is_empty() {
            String::new()
        } else {
            format!(
                "{} ({} ago)",
                self.tool,
                dur(now.saturating_sub(self.tool_ms))
            )
        };
        if self.check_open().is_some() {
            let c = self.check_short(now);
            return if tool.is_empty() {
                c
            } else {
                format!("{c}; last action {tool}")
            };
        }
        if self.tool.is_empty() {
            return String::new();
        }
        if now.saturating_sub(self.tool_ms) >= IDLE_AFTER_MS {
            format!("idle, no check running; last action {tool}")
        } else {
            tool
        }
    }

    /// Which of [`Phases`] this developing task counts in.
    pub fn phase_kind(&self, now: u64) -> PhaseKind {
        match self.check_open() {
            Some(CheckPhase::Waiting) => PhaseKind::Waiting,
            Some(_) => PhaseKind::Checking,
            None if self.tool_ms > 0 && now.saturating_sub(self.tool_ms) >= IDLE_AFTER_MS => {
                PhaseKind::Idle
            }
            None => PhaseKind::Editing,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhaseKind {
    Waiting,
    Checking,
    Editing,
    Idle,
}

/// "31 developing: 23 waiting for a test slot, 6 running checks, 2 editing" (empty for none).
pub fn phase_summary(kinds: &[PhaseKind]) -> String {
    if kinds.is_empty() {
        return String::new();
    }
    let n = |k: PhaseKind| kinds.iter().filter(|x| **x == k).count();
    let parts: Vec<String> = [
        (n(PhaseKind::Waiting), "waiting for a test slot"),
        (n(PhaseKind::Checking), "running checks"),
        (n(PhaseKind::Editing), "editing"),
        (n(PhaseKind::Idle), "idle"),
    ]
    .iter()
    .filter(|(c, _)| *c > 0)
    .map(|(c, w)| format!("{c} {w}"))
    .collect();
    format!("{} developing: {}", kinds.len(), parts.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::progress::{Figure, SubProgress};

    fn p() -> Progress {
        Progress::new(1, "develop", 0)
    }

    #[test]
    fn a_check_moves_through_its_phases_and_keeps_the_times() {
        let mut p = p();
        let i = p.check_start("full", 1000);
        p.checks[i].wait = "Waiting for test-suite slot: 18th in line".into();
        assert!(p.check_line(i, 61_000).contains("18th in line, 1 min"));
        assert_eq!(p.phase_kind(61_000), PhaseKind::Waiting);
        p.checks[i].enter(CheckPhase::Building, 61_000);
        assert!(p.checks[i].wait.is_empty());
        p.checks[i].enter(CheckPhase::Running, 121_000);
        p.subs.push(SubProgress {
            name: "check cargo".into(),
            unit: "tests".into(),
            done: Figure::counted(212),
            total: Figure::reported(340),
            ..SubProgress::default()
        });
        assert_eq!(p.check_short(181_000), "check: 212/340 tests, 2 min");
        assert_eq!(p.phase_kind(181_000), PhaseKind::Checking);
        p.checks[i].finish("red", &["a::b".into(), "c::d".into()], 181_000);
        assert_eq!(p.checks[i].waited_ms, 60_000);
        assert_eq!(p.checks[i].ran_ms, 120_000);
        assert_eq!(
            p.check_line(i, 999_999),
            "check 1 (full): waited 1 min, ran 2 min, red, 2 failed (a::b, c::d)"
        );
        // Done stays done.
        p.checks[i].enter(CheckPhase::Running, 300_000);
        assert_eq!(p.checks[i].phase, CheckPhase::Done);
    }

    #[test]
    fn a_quiet_agent_with_no_check_reads_as_idle() {
        let mut p = p();
        p.tool = "Editing a.rs".into();
        p.tool_ms = 1000;
        assert_eq!(p.doing_text(2000), "Editing a.rs (1 s ago)");
        assert_eq!(p.phase_kind(2000), PhaseKind::Editing);
        let late = 1000 + IDLE_AFTER_MS;
        assert!(p.doing_text(late).starts_with("idle, no check running"));
        assert_eq!(p.phase_kind(late), PhaseKind::Idle);
        p.check_start("full", late);
        assert!(p.doing_text(late).starts_with("check: waiting"));
    }

    #[test]
    fn the_home_summary_counts_by_phase() {
        use PhaseKind::*;
        let k: Vec<PhaseKind> = [(Waiting, 3), (Checking, 2), (Editing, 1)]
            .iter()
            .flat_map(|(k, n)| std::iter::repeat_n(*k, *n))
            .collect();
        assert_eq!(
            phase_summary(&k),
            "6 developing: 3 waiting for a test slot, 2 running checks, 1 editing"
        );
        assert_eq!(phase_summary(&[]), "");
    }
}
