// SPDX-License-Identifier: GPL-3.0-or-later

//! The live progress record of one step run (pure: the daemon feeds it bytes
//! and reads the clock; nothing here does).
//!
//! A record says what is being counted (`unit`), how many are done and how
//! many are expected (each with the source of the figure), the step's latest
//! one-line message, the output so far (lines, bytes, time of the last) and
//! when anything in it last moved. A `history` total is the typical figure
//! from past successful runs and is never a real total.

/// Where a figure came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Source {
    /// There is no figure.
    #[default]
    None,
    /// The step said so.
    Reported,
    /// The daemon counted it.
    Counted,
    /// The typical figure from this project's past successful runs.
    History,
}

/// A number and where it came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Figure {
    pub value: u64,
    pub source: Source,
}

impl Figure {
    pub fn reported(value: u64) -> Figure {
        Figure {
            value,
            source: Source::Reported,
        }
    }
    pub fn counted(value: u64) -> Figure {
        Figure {
            value,
            source: Source::Counted,
        }
    }
    pub fn history(value: u64) -> Figure {
        Figure {
            value,
            source: Source::History,
        }
    }
    /// True for a figure that says something.
    pub fn is_some(&self) -> bool {
        self.source != Source::None
    }
    /// True for a real total (reported or counted), not a typical one.
    pub fn is_real(&self) -> bool {
        matches!(self.source, Source::Reported | Source::Counted)
    }
}

/// What the step has written so far.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Output {
    pub lines: u64,
    pub bytes: u64,
    /// Absolute time of the last output, 0 for none yet.
    pub last_output_ms: u64,
}

/// One of several counts in a step (a suite, a landing phase).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct SubProgress {
    pub name: String,
    pub unit: String,
    pub done: Figure,
    pub total: Figure,
}

/// One item of an agent's todo list.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct TodoItem {
    pub text: String,
    /// "pending", "doing" or "done".
    pub status: String,
}

/// The most log lines kept for one push.
pub const LOG_KEEP: usize = 40;

/// The progress of one step run.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Progress {
    pub run: u64,
    pub step: String,
    pub unit: String,
    pub done: Figure,
    pub total: Figure,
    pub message: String,
    pub output: Output,
    pub started_ms: u64,
    /// The last time anything in the record moved.
    pub updated_at_ms: u64,
    pub subs: Vec<SubProgress>,
    /// The agent's todo list, if it keeps one.
    pub todo: Vec<TodoItem>,
    /// With a `history` total, the low end of the typical range (`total` is the high end).
    pub total_low: u64,
    /// How many past runs the typical figures come from.
    pub history_samples: u32,
    /// The longest quiet gap between two outputs of this run so far.
    pub max_gap_ms: u64,
    /// The usual quiet time before this step counts as silent, from past runs (0: none known).
    pub stale_after_ms: u64,
    /// Whole lines seen since the last push, at most [`LOG_KEEP`] (not stored).
    pub pending_log: Vec<String>,
    /// The unfinished last line of each stream (not stored).
    pub partial: std::collections::BTreeMap<String, Vec<u8>>,
    /// Per output stream (a step host's journal), how far into it the bytes are
    /// already counted. A step can have several (an agent's resumed sessions).
    pub marks: std::collections::BTreeMap<String, u64>,
}

/// A change of the record, and whether a monitor should hear of it at once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    None,
    /// Output only: may wait for the throttle.
    Output,
    /// `done`, `total` or `message` moved: send now.
    Urgent,
}

impl Progress {
    pub fn new(run: u64, step: &str, started_ms: u64) -> Progress {
        Progress {
            run,
            step: step.to_string(),
            started_ms,
            updated_at_ms: started_ms,
            ..Progress::default()
        }
    }

    /// Output observed from absolute `offset` of the stream `stream`. Only the
    /// bytes beyond what was already counted are counted, so a journal that is
    /// read again (after a restart, or from disk then live) adds nothing twice.
    pub fn output(&mut self, stream: &str, offset: u64, bytes: &[u8], at_ms: u64) -> Change {
        let end = offset + bytes.len() as u64;
        let high = self.marks.get(stream).copied().unwrap_or(0);
        if end <= high || bytes.is_empty() {
            return Change::None;
        }
        let skip = high.saturating_sub(offset) as usize;
        // A gap (offset beyond the high water) is counted from where it starts.
        let fresh = &bytes[skip.min(bytes.len())..];
        self.output.lines += fresh.iter().filter(|b| **b == b'\n').count() as u64;
        self.output.bytes += fresh.len() as u64;
        let since = if self.output.last_output_ms > 0 {
            self.output.last_output_ms
        } else {
            self.started_ms
        };
        self.max_gap_ms = self.max_gap_ms.max(at_ms.saturating_sub(since));
        self.output.last_output_ms = at_ms;
        self.take_lines(stream, fresh);
        self.marks.insert(stream.to_string(), end);
        self.updated_at_ms = at_ms;
        // Without a reported figure the count of lines is the count.
        if !matches!(self.done.source, Source::Reported) {
            self.unit = "lines".to_string();
            self.done = Figure::counted(self.output.lines);
        }
        Change::Output
    }

    /// Cut `fresh` into whole lines: the unfinished end waits for the next bytes.
    fn take_lines(&mut self, stream: &str, fresh: &[u8]) {
        let buf = self.partial.entry(stream.to_string()).or_default();
        buf.extend_from_slice(fresh);
        while let Some(i) = buf.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = buf.drain(..=i).collect();
            let text = String::from_utf8_lossy(&line[..line.len() - 1])
                .trim_end_matches('\r')
                .to_string();
            self.pending_log.push(text);
        }
        // A line that never ends is cut, not grown without limit.
        if buf.len() > 8192 {
            buf.clear();
        }
        let extra = self.pending_log.len().saturating_sub(LOG_KEEP);
        self.pending_log.drain(..extra);
    }

    /// The step reports how many it has done (and in what unit, if given).
    pub fn report_done(&mut self, unit: &str, done: u64, at_ms: u64) -> Change {
        let was = (self.unit.clone(), self.done);
        if !unit.is_empty() {
            self.unit = unit.to_string();
        }
        self.done = Figure::reported(done);
        if (self.unit.clone(), self.done) == was {
            return Change::None;
        }
        self.updated_at_ms = at_ms;
        Change::Urgent
    }

    pub fn report_total(&mut self, total: u64, at_ms: u64) -> Change {
        let t = Figure::reported(total);
        if self.total == t {
            return Change::None;
        }
        self.total = t;
        self.updated_at_ms = at_ms;
        Change::Urgent
    }

    pub fn set_message(&mut self, message: &str, at_ms: u64) -> Change {
        if self.message == message {
            return Change::None;
        }
        self.message = message.to_string();
        self.updated_at_ms = at_ms;
        Change::Urgent
    }

    /// Offer the typical figure as a total, if no real total is known.
    /// `range` is the typical `(low, high)`; `samples` is how many runs it comes from.
    pub fn offer_history_total(&mut self, range: Option<(u64, u64)>, samples: usize) {
        if self.total.is_real() {
            return;
        }
        match range {
            Some((lo, hi)) => {
                self.total = Figure::history(hi);
                self.total_low = lo;
                self.history_samples = samples as u32;
            }
            None => {
                self.total = Figure::default();
                self.total_low = 0;
                self.history_samples = 0;
            }
        }
    }

    /// The whole percent done, only from a real total: rounded down, at most 100.
    pub fn percent(&self) -> Option<u32> {
        percent(self.done.value, self.real_total()?)
    }

    /// The total to show as a real one, if there is one.
    pub fn real_total(&self) -> Option<u64> {
        self.total.is_real().then_some(self.total.value)
    }
}

/// The samples needed before a typical figure is offered.
pub const MIN_SAMPLES: usize = 5;

/// The middle half of the samples as `(low, high)`: the figures a quarter and
/// three quarters of the way up the sorted list. None under five samples.
pub fn middle_half(samples: &[u64]) -> Option<(u64, u64)> {
    if samples.len() < MIN_SAMPLES {
        return None;
    }
    let mut s = samples.to_vec();
    s.sort_unstable();
    Some((s[s.len() / 4], s[s.len() * 3 / 4]))
}

/// The total offered from past successful runs: the high end of the middle
/// half, or none under five samples.
pub fn history_total(samples: &[u64]) -> Option<u64> {
    middle_half(samples).map(|(_, hi)| hi)
}

/// The whole percent `done` is of `total`: rounded down, at most 100, none for a total of 0.
/// Callers pass only real totals; a typical figure never gets a percentage.
pub fn percent(done: u64, total: u64) -> Option<u32> {
    if total == 0 {
        return None;
    }
    Some((done.min(total) as u128 * 100 / total as u128) as u32)
}

/// How long a step may be quiet before it is shown as silent, with too few past runs to say.
pub const DEFAULT_STALE_MS: u64 = 300_000;
/// The least quiet time ever called silent.
pub const MIN_STALE_MS: u64 = 30_000;

/// The quiet time after which a step is shown as silent: the high end of the middle half
/// of the longest gaps in past successful runs, or [`DEFAULT_STALE_MS`] with under five.
pub fn stale_after(gaps: &[u64]) -> u64 {
    middle_half(gaps).map_or(DEFAULT_STALE_MS, |(_, hi)| hi.max(MIN_STALE_MS))
}

/// True when nothing has moved for longer than `after_ms`.
pub fn is_stale(now_ms: u64, moved_ms: u64, after_ms: u64) -> bool {
    moved_ms > 0 && now_ms.saturating_sub(moved_ms) > after_ms
}

/// How often an output-only change is sent, at most.
pub const THROTTLE_MS: u64 = 1000;

/// What to do with a change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Push {
    /// Push it now.
    Now,
    /// Push it at this time (once; later changes are folded in).
    At(u64),
    /// A send is already due.
    Wait,
}

/// Coalesces changes of one run: at most one send a second for output alone,
/// at once for anything else.
#[derive(Clone, Copy, Debug)]
pub struct Throttle {
    last_sent: u64,
    due: Option<u64>,
}

impl Throttle {
    /// `now` is when the record was first sent.
    pub fn new(now: u64) -> Throttle {
        Throttle {
            last_sent: now,
            due: None,
        }
    }

    pub fn note(&mut self, now: u64, urgent: bool) -> Push {
        if urgent || now >= self.last_sent + THROTTLE_MS {
            self.last_sent = now;
            self.due = None;
            return Push::Now;
        }
        if self.due.is_some() {
            return Push::Wait;
        }
        let at = self.last_sent + THROTTLE_MS;
        self.due = Some(at);
        Push::At(at)
    }

    /// True once when a waiting send has come due.
    pub fn take_due(&mut self, now: u64) -> bool {
        match self.due {
            Some(at) if now >= at => {
                self.due = None;
                self.last_sent = now;
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_lines_and_bytes() {
        let mut p = Progress::new(1, "build", 100);
        assert_eq!(p.output("", 0, b"a\nb\nc", 200), Change::Output);
        assert_eq!((p.output.lines, p.output.bytes), (2, 5));
        assert_eq!(p.output.last_output_ms, 200);
        assert_eq!(p.unit, "lines");
        assert_eq!(p.done, Figure::counted(2));
        assert_eq!(p.output("", 5, b"\n", 300), Change::Output);
        assert_eq!(p.output.lines, 3);
    }

    #[test]
    fn reobserved_bytes_are_not_counted_twice() {
        let mut p = Progress::new(1, "build", 0);
        p.output("", 0, b"one\ntwo\n", 10);
        assert_eq!(p.output("", 0, b"one\ntwo\n", 20), Change::None);
        assert_eq!(p.output("", 4, b"two\nthree\n", 30), Change::Output);
        assert_eq!((p.output.lines, p.output.bytes), (3, 14));
        assert_eq!(p.output.last_output_ms, 30);
    }

    #[test]
    fn streams_are_counted_apart() {
        let mut p = Progress::new(1, "agent", 0);
        p.output("a-1", 0, b"x\ny\n", 5);
        p.output("a-2", 0, b"z\n", 6);
        assert_eq!(p.output.lines, 3);
        assert_eq!(p.output("a-1", 0, b"x\ny\n", 7), Change::None);
        assert_eq!(p.output.lines, 3);
    }

    #[test]
    fn updated_at_stays_without_output() {
        let mut p = Progress::new(1, "build", 100);
        p.output("", 0, b"x\n", 150);
        assert_eq!(p.output("", 0, b"x\n", 9000), Change::None);
        assert_eq!(p.updated_at_ms, 150);
    }

    #[test]
    fn reported_figures_win_over_counted() {
        let mut p = Progress::new(1, "t", 0);
        assert_eq!(p.report_done("tests", 3, 5), Change::Urgent);
        assert_eq!(p.report_done("tests", 3, 6), Change::None);
        p.output("", 0, b"a\nb\n", 10);
        assert_eq!(p.done, Figure::reported(3));
        assert_eq!(p.unit, "tests");
        assert_eq!(p.report_total(9, 11), Change::Urgent);
        assert_eq!(p.set_message("running x", 12), Change::Urgent);
        assert_eq!(p.set_message("running x", 13), Change::None);
        assert_eq!(p.updated_at_ms, 12);
    }

    #[test]
    fn middle_half_needs_five() {
        assert_eq!(middle_half(&[1, 2, 3, 4]), None);
        assert_eq!(middle_half(&[50, 10, 30, 20, 40]), Some((20, 40)));
        assert_eq!(history_total(&[1, 2, 3, 4]), None);
        assert_eq!(history_total(&[10, 20, 30, 40, 50, 60, 70, 80]), Some(70));
    }

    #[test]
    fn history_total_is_never_real() {
        let mut p = Progress::new(1, "t", 0);
        p.offer_history_total(Some((40, 70)), 8);
        assert_eq!(p.total, Figure::history(70));
        assert_eq!((p.total_low, p.history_samples, p.percent()), (40, 8, None));
        assert_eq!(p.real_total(), None);
        p.report_total(5, 1);
        p.offer_history_total(Some((40, 70)), 8);
        assert_eq!(p.total, Figure::reported(5));
        let mut q = Progress::new(2, "t", 0);
        q.offer_history_total(None, 0);
        assert_eq!(q.total.source, Source::None);
    }

    #[test]
    fn percent_is_floored_capped_and_real_only() {
        assert_eq!(percent(212, 340), Some(62));
        assert_eq!(percent(3, 7), Some(42));
        assert_eq!(percent(1, 1000), Some(0));
        assert_eq!(percent(999, 1000), Some(99));
        assert_eq!(percent(400, 340), Some(100));
        assert_eq!(percent(5, 0), None);
        let mut p = Progress::new(1, "t", 0);
        p.report_done("tests", 212, 1);
        assert_eq!(p.percent(), None);
        p.report_total(340, 2);
        assert_eq!(p.percent(), Some(62));
    }

    #[test]
    fn stale_rule() {
        assert_eq!(stale_after(&[1, 2, 3, 4]), DEFAULT_STALE_MS);
        assert_eq!(stale_after(&[100; 8]), MIN_STALE_MS);
        let gaps = [
            60_000, 90_000, 120_000, 150_000, 180_000, 240_000, 300_000, 600_000,
        ];
        assert_eq!(stale_after(&gaps), 300_000);
        assert!(!is_stale(1_000_000, 1_000_000 - 300_000, 300_000));
        assert!(is_stale(1_000_000, 1_000_000 - 300_001, 300_000));
        assert!(!is_stale(1_000_000, 0, 300_000));
    }

    #[test]
    fn gaps_and_log_lines() {
        let mut p = Progress::new(1, "t", 1000);
        p.output("", 0, b"one\ntw", 1500);
        p.output("", 6, b"o\r\nthree\n", 4500);
        assert_eq!(p.max_gap_ms, 3000);
        assert_eq!(p.pending_log, vec!["one", "two", "three"]);
        let big: Vec<u8> = (0..100)
            .flat_map(|i| format!("l{i}\n").into_bytes())
            .collect();
        p.output("b", 0, &big, 4600);
        assert_eq!(p.pending_log.len(), LOG_KEEP);
        assert_eq!(p.pending_log.last().unwrap(), "l99");
    }

    #[test]
    fn a_burst_is_one_update() {
        let mut p = Progress::new(1, "t", 0);
        let mut th = Throttle::new(0);
        let (mut now_n, mut at_n, mut wait_n) = (0, 0, 0);
        for i in 0..10_000u64 {
            let c = p.output("", i * 2, b"x\n", 1);
            assert_eq!(c, Change::Output);
            match th.note(1, c == Change::Urgent) {
                Push::Now => now_n += 1,
                Push::At(t) => {
                    assert_eq!(t, THROTTLE_MS);
                    at_n += 1
                }
                Push::Wait => wait_n += 1,
            }
        }
        assert_eq!((now_n, at_n, wait_n), (0, 1, 9999));
        assert!(!th.take_due(500));
        assert!(th.take_due(1000));
        assert!(!th.take_due(1001));
        assert_eq!(p.output.lines, 10_000);
    }

    #[test]
    fn urgent_goes_now_and_limits_the_rate() {
        let mut th = Throttle::new(0);
        assert_eq!(th.note(10, true), Push::Now);
        assert_eq!(th.note(20, false), Push::At(1010));
        assert_eq!(th.note(1010, false), Push::Now);
        assert_eq!(th.note(1500, false), Push::At(2010));
        assert_eq!(th.note(1600, true), Push::Now);
        assert!(!th.take_due(2010));
    }
}
