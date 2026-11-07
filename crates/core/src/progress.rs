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
        self.output.last_output_ms = at_ms;
        self.marks.insert(stream.to_string(), end);
        self.updated_at_ms = at_ms;
        // Without a reported figure the count of lines is the count.
        if !matches!(self.done.source, Source::Reported) {
            self.unit = "lines".to_string();
            self.done = Figure::counted(self.output.lines);
        }
        Change::Output
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
    pub fn offer_history_total(&mut self, total: Option<u64>) {
        if self.total.is_real() {
            return;
        }
        self.total = total.map_or(Figure::default(), Figure::history);
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
        p.offer_history_total(Some(70));
        assert_eq!(p.total, Figure::history(70));
        assert_eq!(p.real_total(), None);
        p.report_total(5, 1);
        p.offer_history_total(Some(70));
        assert_eq!(p.total, Figure::reported(5));
        let mut q = Progress::new(2, "t", 0);
        q.offer_history_total(None);
        assert_eq!(q.total.source, Source::None);
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
