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
    /// The last green run's count for this suite and base (a real total).
    Expected,
    /// The suite's declared minimum number of tests (a real total).
    Minimum,
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
    pub fn expected(value: u64) -> Figure {
        Figure {
            value,
            source: Source::Expected,
        }
    }
    pub fn minimum(value: u64) -> Figure {
        Figure {
            value,
            source: Source::Minimum,
        }
    }
    /// True for a real total (reported, counted, expected or a minimum), not a typical one.
    pub fn is_real(&self) -> bool {
        matches!(
            self.source,
            Source::Reported | Source::Counted | Source::Expected | Source::Minimum
        )
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

/// The tokens one LLM session has used so far. A figure the stream did not report
/// is `None`, never 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Tokens {
    pub input: Option<u64>,
    pub output: Option<u64>,
    pub cache_read: Option<u64>,
    pub cache_write: Option<u64>,
    /// Cost in billionths of a dollar (kept whole so a record compares exactly).
    pub cost_nano_usd: Option<u64>,
}

impl Tokens {
    /// The sum of two, a figure staying absent only if neither has it.
    pub fn plus(&self, o: &Tokens) -> Tokens {
        fn add(a: Option<u64>, b: Option<u64>) -> Option<u64> {
            match (a, b) {
                (None, None) => None,
                (a, b) => Some(a.unwrap_or(0) + b.unwrap_or(0)),
            }
        }
        Tokens {
            input: add(self.input, o.input),
            output: add(self.output, o.output),
            cache_read: add(self.cache_read, o.cache_read),
            cache_write: add(self.cache_write, o.cache_write),
            cost_nano_usd: add(self.cost_nano_usd, o.cost_nano_usd),
        }
    }
    pub fn cost_nano(usd: f64) -> u64 {
        (usd.max(0.0) * 1e9).round() as u64
    }
}

/// How the start of a line decides whether it is counted: Claude Code's
/// `--include-partial-messages` writes one `stream_event` line per delta, and those
/// are not output the step produced.
const SKIPPED_LINE: &[u8] = b"{\"type\":\"stream_event\"";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum Kind {
    /// Not enough of the line yet to say.
    #[default]
    Undecided,
    Counted,
    Skipped,
}

/// Where one stream's scan of whole lines is.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct LineScan {
    /// Absolute offset the scan has reached.
    pos: u64,
    kind: Kind,
    /// Bytes of the unfinished line that count but wait for its kind.
    pending: u64,
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

/// The typical `(low, high, samples)` of past runs, kept in memory so it can be
/// shown again whenever there is no real total. It is not stored with the row
/// (the totals it produced are), so it never takes part in comparing two records.
#[derive(Clone, Copy, Debug, Default)]
pub struct Remembered(pub Option<(u64, u64, u32)>);

impl PartialEq for Remembered {
    fn eq(&self, _: &Remembered) -> bool {
        true
    }
}
impl Eq for Remembered {}

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
    /// True for an agent step: its figure follows [`Progress::choose`], and
    /// output lines never set `done`.
    pub agent: bool,
    /// The agent's own latest counts from `report_progress`: `(done, total, unit)`.
    pub reported: Option<(u64, u64, String)>,
    /// True once the agent has given a message of its own this run.
    pub reported_message: bool,
    /// Tool calls the agent has made this run.
    pub tool_calls: u64,
    /// The typical `(low, high, samples)` from past runs, kept so it can be
    /// shown again whenever there is no real total.
    pub history: Remembered,
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
    /// Per output stream, the tokens its LLM session has used (cumulative, so a
    /// journal read again replaces and never adds). One step run's figure is their sum.
    pub tokens: std::collections::BTreeMap<String, Tokens>,
    /// Per output stream, the line scan (not stored).
    pub scans: std::collections::BTreeMap<String, LineScan>,
    /// Streams written to a pseudo-terminal: their live log lines have terminal
    /// control sequences removed (the log file keeps them). Not stored.
    pub terminal: std::collections::BTreeSet<String>,
}

/// A line of terminal output without its control sequences: CSI (`ESC [ … final`),
/// OSC (`ESC ] … BEL` or `ESC \`), other two-byte escapes, every carriage return, and what a backspace erases.
pub fn strip_terminal(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '\u{1b}' => match it.next() {
                Some('[') => {
                    for n in it.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&n) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    while let Some(n) = it.next() {
                        if n == '\u{7}' {
                            break;
                        }
                        if n == '\u{1b}' {
                            it.next();
                            break;
                        }
                    }
                }
                _ => {}
            },
            '\r' => {}
            // A backspace erases the character before it: `script` starts its output with
            // `^D` and two backspaces when its stdin is at its end.
            '\u{8}' => {
                out.pop();
            }
            c => out.push(c),
        }
    }
    out
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
    ///
    /// Lines are judged whole: a `stream_event` line is not counted in `lines`,
    /// `bytes`, `done` or the log, however the chunks cut it. `marks` still
    /// advances by the real offsets. Bytes already counted are still scanned (in
    /// order), so a line cut by the mark is judged by its start.
    pub fn output(&mut self, stream: &str, offset: u64, bytes: &[u8], at_ms: u64) -> Change {
        let end = offset + bytes.len() as u64;
        let high = self.marks.get(stream).copied().unwrap_or(0);
        if bytes.is_empty() {
            return Change::None;
        }
        let (lines0, bytes0) = (self.output.lines, self.output.bytes);
        self.scan(stream, offset, bytes, high);
        if end <= high {
            return Change::None;
        }
        self.marks.insert(stream.to_string(), end);
        if (self.output.lines, self.output.bytes) == (lines0, bytes0) {
            return Change::None;
        }
        let since = if self.output.last_output_ms > 0 {
            self.output.last_output_ms
        } else {
            self.started_ms
        };
        self.max_gap_ms = self.max_gap_ms.max(at_ms.saturating_sub(since));
        self.output.last_output_ms = at_ms;
        self.updated_at_ms = at_ms;
        // Without a reported figure the count of lines is the count (never
        // for an agent: its figure is chosen by `choose`).
        // (A daemon-counted figure in another unit, like tests, is not replaced.)
        let pinned = self.done.source == Source::Counted && self.unit != "lines";
        if !self.agent && !matches!(self.done.source, Source::Reported) && !pinned {
            self.unit = "lines".to_string();
            self.done = Figure::counted(self.output.lines);
        }
        Change::Output
    }

    /// Cut the chunk into whole lines and count those that are output.
    fn scan(&mut self, stream: &str, offset: u64, bytes: &[u8], high: u64) {
        let mut sc = self.scans.remove(stream).unwrap_or_default();
        let mut buf = self.partial.remove(stream).unwrap_or_default();
        let mut i = 0usize;
        if offset > sc.pos {
            // A gap: the line is judged from here, as counted.
            sc = LineScan {
                pos: offset,
                kind: Kind::Counted,
                pending: 0,
            };
            buf.clear();
        } else {
            i = (sc.pos - offset) as usize;
        }
        while i < bytes.len() {
            let nl = bytes[i..].iter().position(|b| *b == b'\n');
            let stop = nl.map_or(bytes.len(), |n| i + n + 1);
            let seg = &bytes[i..stop];
            let seg_abs = offset + i as u64;
            sc.pending += (seg_abs + seg.len() as u64).saturating_sub(seg_abs.max(high));
            if sc.kind != Kind::Skipped {
                buf.extend_from_slice(seg);
            }
            if sc.kind == Kind::Undecided {
                if buf.starts_with(SKIPPED_LINE) {
                    sc.kind = Kind::Skipped;
                    buf.clear();
                } else if nl.is_some()
                    || buf.len() >= SKIPPED_LINE.len()
                    || !SKIPPED_LINE.starts_with(&buf)
                {
                    sc.kind = Kind::Counted;
                }
            }
            match sc.kind {
                Kind::Counted => {
                    self.output.bytes += std::mem::take(&mut sc.pending);
                }
                Kind::Skipped => sc.pending = 0,
                Kind::Undecided => {}
            }
            if nl.is_some() {
                if sc.kind == Kind::Counted && seg_abs + seg.len() as u64 > high {
                    self.output.lines += 1;
                    let text = String::from_utf8_lossy(&buf[..buf.len() - 1])
                        .trim_end_matches('\r')
                        .to_string();
                    let text = if self.terminal.contains(stream) {
                        let clean = strip_terminal(&text);
                        // The latest line is what the step is doing now (for xctest,
                        // the test that started), shown as the record's message.
                        if !self.agent && !self.reported_message && !clean.trim().is_empty() {
                            self.message = one_line(&clean);
                        }
                        clean
                    } else {
                        text
                    };
                    self.pending_log.push(text);
                }
                buf.clear();
                sc.kind = Kind::Undecided;
            } else if buf.len() > 8192 {
                // A line that never ends is cut, not grown without limit.
                buf.clear();
            }
            i = stop;
        }
        sc.pos = sc.pos.max(offset + bytes.len() as u64);
        let extra = self.pending_log.len().saturating_sub(LOG_KEEP);
        self.pending_log.drain(..extra);
        self.scans.insert(stream.to_string(), sc);
        self.partial.insert(stream.to_string(), buf);
    }

    /// The tokens stream `stream`'s session has used so far (cumulative: this
    /// replaces what the stream had). Output-only for pushing purposes.
    ///
    /// A stream's figures only grow, so a journal replayed after a restart, whose
    /// tally climbs again from nothing, never shows a figure lower than the one
    /// already known: each figure keeps the larger of the two.
    pub fn report_tokens(&mut self, stream: &str, t: Tokens, at_ms: u64) -> Change {
        let t = match self.tokens.get(stream) {
            Some(old) => Tokens {
                input: t.input.max(old.input),
                output: t.output.max(old.output),
                cache_read: t.cache_read.max(old.cache_read),
                cache_write: t.cache_write.max(old.cache_write),
                cost_nano_usd: t.cost_nano_usd.max(old.cost_nano_usd),
            },
            None => t,
        };
        if self.tokens.get(stream) == Some(&t) {
            return Change::None;
        }
        self.tokens.insert(stream.to_string(), t);
        self.updated_at_ms = at_ms;
        Change::Output
    }

    /// The step run's tokens: the sum of its streams, none if none reported.
    pub fn token_total(&self) -> Option<Tokens> {
        self.tokens.values().copied().reduce(|a, b| a.plus(&b))
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
        self.history = Remembered(range.map(|(lo, hi)| (lo, hi, samples as u32)));
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

    /// Mark this record as an agent's, with the typical tool-call count of past runs.
    pub fn begin_agent(&mut self, range: Option<(u64, u64)>, samples: usize) {
        self.agent = true;
        self.offer_history_total(range, samples);
        self.choose();
    }

    /// The agent called `report_progress`: its message always wins, and its
    /// counts (when given) are the figure. A changed total replaces the old.
    pub fn agent_report(
        &mut self,
        message: &str,
        counts: Option<(u64, u64)>,
        unit: &str,
        at_ms: u64,
    ) -> Change {
        let before = self.snapshot();
        self.agent = true;
        if let Some((d, t)) = counts {
            let unit = if unit.trim().is_empty() {
                "steps"
            } else {
                unit
            };
            self.reported = Some((d, t, unit.to_string()));
        }
        self.reported_message = true;
        self.message = message.to_string();
        self.choose();
        self.moved(before, at_ms)
    }

    /// The agent's todo list changed. Its message is the item in progress,
    /// unless the agent has given a message of its own.
    pub fn agent_plan(&mut self, items: Vec<TodoItem>, at_ms: u64) -> Change {
        let before = self.snapshot();
        self.agent = true;
        self.todo = items;
        if !self.reported_message {
            if let Some(t) = self.todo.iter().find(|t| t.status == "doing") {
                self.message = t.text.clone();
            }
        }
        self.choose();
        self.moved(before, at_ms)
    }

    /// The agent made a tool call. The summary is the message only while the
    /// agent has neither reported a message nor has an item in progress.
    /// Only the count changes at once-or-throttled pace: this is `Output`.
    pub fn agent_tool(&mut self, summary: &str, at_ms: u64) -> Change {
        let before = self.snapshot();
        self.agent = true;
        self.tool_calls += 1;
        let doing = self.todo.iter().any(|t| t.status == "doing");
        if !self.reported_message && !doing {
            self.message = summary.to_string();
        }
        self.choose();
        match self.moved(before, at_ms) {
            Change::None => Change::None,
            _ => Change::Output,
        }
    }

    fn snapshot(&self) -> (String, Figure, Figure, String, Vec<TodoItem>) {
        (
            self.unit.clone(),
            self.done,
            self.total,
            self.message.clone(),
            self.todo.clone(),
        )
    }

    fn moved(
        &mut self,
        before: (String, Figure, Figure, String, Vec<TodoItem>),
        at_ms: u64,
    ) -> Change {
        if before == self.snapshot() {
            return Change::None;
        }
        self.updated_at_ms = at_ms;
        Change::Urgent
    }

    /// The one rule for an agent's figure: the counts it reported, else its
    /// todo list ("plan steps": done of all), else its tool calls, with the
    /// typical figure from past runs as the total only when nothing real is
    /// known. Output lines never set `done`.
    pub fn choose(&mut self) {
        if let Some((d, t, unit)) = self.reported.clone() {
            self.unit = unit;
            self.done = Figure::reported(d);
            self.total = Figure::reported(t);
            self.total_low = 0;
            self.history_samples = 0;
        } else if !self.todo.is_empty() {
            self.unit = "plan steps".to_string();
            let done = self.todo.iter().filter(|t| t.status == "done").count() as u64;
            self.done = Figure::reported(done);
            self.total = Figure::reported(self.todo.len() as u64);
            self.total_low = 0;
            self.history_samples = 0;
        } else if self.tool_calls > 0 {
            self.unit = "tool calls".to_string();
            self.done = Figure::counted(self.tool_calls);
            self.apply_history();
        } else {
            self.apply_history();
        }
    }

    fn apply_history(&mut self) {
        match self.history.0 {
            Some((lo, hi, n)) => {
                self.total = Figure::history(hi);
                self.total_low = lo;
                self.history_samples = n;
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

/// The longest message line an agent's tool call may give.
pub const LINE_MAX: usize = 120;

/// One line of at most [`LINE_MAX`] characters with anything that looks like a
/// secret taken out: a long mixed token, a `ghp_`/`sk-` token, the value of an
/// assignment whose name says key, token, secret or password, and what follows
/// `Bearer`. (There was no shared log redactor to reuse; this is the same word
/// test the proposal redactor uses, applied to one line.)
pub fn one_line(text: &str) -> String {
    let mut out: Vec<String> = vec![];
    let mut after_bearer = false;
    for w in text.split_whitespace() {
        let t = w.trim_matches(|c: char| "`'\"(),;".contains(c));
        let secretish = t.len() >= 20
            && t.chars()
                .all(|c| c.is_ascii_alphanumeric() || "_-".contains(c))
            && t.chars().any(|c| c.is_ascii_digit())
            && t.chars().any(|c| c.is_ascii_alphabetic());
        let named = w.split_once('=').is_some_and(|(k, v)| {
            let k = k.to_ascii_lowercase();
            !v.is_empty()
                && ["key", "token", "secret", "password", "passwd"]
                    .iter()
                    .any(|n| k.contains(n))
        });
        if after_bearer || secretish || t.starts_with("ghp_") || t.starts_with("sk-") {
            out.push("[secret]".into());
        } else if named {
            let k = w.split_once('=').map_or("", |(k, _)| k);
            out.push(format!("{k}=[secret]"));
        } else {
            out.push(w.to_string());
        }
        after_bearer = w.eq_ignore_ascii_case("bearer");
    }
    let line = out.join(" ");
    if line.chars().count() <= LINE_MAX {
        return line;
    }
    let mut cut: String = line.chars().take(LINE_MAX - 1).collect();
    cut.push('…');
    cut
}

/// What one line of a progress file or one `step.progress` says. Every field is optional.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Report {
    pub done: Option<u64>,
    pub total: Option<u64>,
    pub unit: Option<String>,
    pub message: Option<String>,
}

impl Progress {
    /// Apply a report: done and total are the step's own figures; a count alone
    /// is output-grade, a total or message is urgent.
    pub fn apply_report(&mut self, r: &Report, at_ms: u64) -> Change {
        let mut change = Change::None;
        if let Some(u) = r.unit.as_deref().filter(|u| !u.is_empty()) {
            if self.unit != u {
                self.unit = u.to_string();
                self.updated_at_ms = at_ms;
                change = Change::Urgent;
            }
        }
        if let Some(d) = r.done {
            let unit = self.unit.clone();
            let c = self.report_done(&unit, d, at_ms);
            if c != Change::None && change == Change::None {
                change = Change::Output;
            }
        }
        for c in [
            r.total.map(|t| self.report_total(t, at_ms)),
            r.message.as_deref().map(|m| self.set_message(m, at_ms)),
        ]
        .into_iter()
        .flatten()
        {
            if c == Change::Urgent {
                change = Change::Urgent;
            }
        }
        change
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

    const EV: &str = "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_delta\"}}\n";

    /// The same stream with and without stream_event lines, cut at every byte.
    #[test]
    fn stream_event_lines_are_not_counted_wherever_the_chunks_end() {
        let real = "{\"type\":\"assistant\"}\nplain\n{\"type\":\"result\"}\n";
        let mixed = format!(
            "{EV}{{\"type\":\"assistant\"}}\n{EV}{EV}plain\n{EV}{{\"type\":\"result\"}}\n{EV}"
        );
        let mut a = Progress::new(1, "develop", 0);
        a.output("h", 0, real.as_bytes(), 1);
        for cut in 0..=mixed.len() {
            let mut b = Progress::new(1, "develop", 0);
            b.output("h", 0, &mixed.as_bytes()[..cut], 1);
            b.output("h", cut as u64, &mixed.as_bytes()[cut..], 2);
            assert_eq!(b.output.lines, a.output.lines, "cut {cut}");
            assert_eq!(b.output.bytes, a.output.bytes, "cut {cut}");
            assert_eq!(b.done, a.done, "cut {cut}");
            assert_eq!(b.pending_log, a.pending_log, "cut {cut}");
            assert_eq!(b.marks["h"], mixed.len() as u64, "marks are real offsets");
        }
    }

    #[test]
    fn a_replay_from_a_mark_inside_a_line_judges_the_line_by_its_start() {
        let mixed = format!("one\n{EV}two\n");
        let mut p = Progress::new(1, "develop", 0);
        // As restored after a restart: the mark sits inside the stream_event line.
        p.marks.insert("h".into(), 8);
        p.output.lines = 1;
        p.output.bytes = 4;
        p.output("h", 0, mixed.as_bytes(), 1);
        assert_eq!((p.output.lines, p.output.bytes), (2, 8));
        assert_eq!(p.pending_log, vec!["two"]);
    }

    #[test]
    fn tokens_are_per_stream_and_summed_and_replaced_not_added() {
        let mut p = Progress::new(1, "develop", 0);
        assert_eq!(p.token_total(), None);
        let t = |i, o| Tokens {
            input: Some(i),
            output: Some(o),
            ..Default::default()
        };
        assert_eq!(p.report_tokens("a", t(10, 5), 1), Change::Output);
        assert_eq!(
            p.report_tokens("a", t(10, 5), 2),
            Change::None,
            "same figure"
        );
        p.report_tokens("a", t(10, 8), 3);
        assert_eq!(
            p.report_tokens("a", t(2, 1), 3),
            Change::None,
            "a replay climbing again"
        );
        p.report_tokens("b", t(1, 1), 4);
        let sum = p.token_total().unwrap();
        assert_eq!(
            (sum.input, sum.output, sum.cache_read),
            (Some(11), Some(9), None)
        );
    }

    #[test]
    fn apply_report_grades_changes() {
        let mut p = Progress::new(1, "t", 0);
        let c = p.apply_report(
            &Report {
                done: Some(2),
                ..Report::default()
            },
            5,
        );
        assert_eq!(c, Change::Output);
        let c = p.apply_report(
            &Report {
                total: Some(9),
                ..Report::default()
            },
            6,
        );
        assert_eq!(c, Change::Urgent);
        assert_eq!(p.total, Figure::reported(9));
    }

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

    fn item(text: &str, status: &str) -> TodoItem {
        TodoItem {
            text: text.into(),
            status: status.into(),
        }
    }

    #[test]
    fn reported_counts_beat_the_todo_list_which_beats_tool_calls() {
        let mut p = Progress::new(1, "develop", 0);
        p.begin_agent(Some((10, 30)), 6);
        assert_eq!(p.total, Figure::history(30));
        p.agent_tool("Reading a.rs", 1);
        assert_eq!(
            (p.unit.as_str(), p.done, p.total),
            ("tool calls", Figure::counted(1), Figure::history(30))
        );
        assert_eq!(p.message, "Reading a.rs");
        p.agent_plan(
            vec![item("a", "done"), item("b", "doing"), item("c", "pending")],
            2,
        );
        assert_eq!(p.unit, "plan steps");
        assert_eq!(
            (p.done, p.total),
            (Figure::reported(1), Figure::reported(3))
        );
        assert_eq!(p.message, "b");
        p.agent_tool("Running cargo test", 3);
        assert_eq!(
            p.message, "b",
            "a tool summary does not replace the item in progress"
        );
        assert_eq!(p.tool_calls, 2);
        p.agent_report("2 of 4 files", Some((2, 4)), "files", 4);
        assert_eq!(
            (p.unit.as_str(), p.done, p.total),
            ("files", Figure::reported(2), Figure::reported(4))
        );
        p.agent_plan(
            vec![item("a", "done"), item("b", "done"), item("c", "doing")],
            5,
        );
        assert_eq!(
            p.unit, "files",
            "the plan does not displace reported counts"
        );
        assert_eq!(p.message, "2 of 4 files");
        p.agent_tool("Editing x", 6);
        assert_eq!(p.message, "2 of 4 files");
    }

    #[test]
    fn a_changing_reported_total_keeps_the_latest() {
        let mut p = Progress::new(1, "develop", 0);
        assert_eq!(p.agent_report("a", Some((1, 4)), "", 1), Change::Urgent);
        assert_eq!((p.unit.as_str(), p.total.value), ("steps", 4));
        p.agent_report("b", Some((2, 5)), "", 2);
        assert_eq!((p.done.value, p.total.value), (2, 5));
        assert_eq!(p.agent_report("b", Some((2, 5)), "", 3), Change::None);
    }

    #[test]
    fn a_silent_agent_shows_tool_calls_and_history_and_lines_never_set_done() {
        let mut p = Progress::new(1, "develop", 0);
        p.begin_agent(Some((8, 12)), 5);
        p.agent_tool("Running cargo test", 1);
        p.output(
            "", 0, b"a
b
", 2,
        );
        assert_eq!(
            (p.unit.as_str(), p.done),
            ("tool calls", Figure::counted(1))
        );
        assert_eq!(
            (p.total, p.total_low, p.percent()),
            (Figure::history(12), 8, None)
        );
        assert_eq!(p.output.lines, 2);
        // A real total displaces the typical one.
        p.agent_report("x", Some((1, 2)), "files", 3);
        assert!(p.total.is_real());
        // Without history there is no total at all.
        let mut q = Progress::new(2, "develop", 0);
        q.begin_agent(None, 0);
        q.agent_tool("Reading b", 1);
        assert_eq!(q.total, Figure::default());
    }

    #[test]
    fn a_tool_line_is_one_short_redacted_line() {
        let l = one_line(
            "Running curl -H 'Authorization: Bearer abc123' -u me\nTOKEN=hunter2 sk-abcdef ghp_xyz",
        );
        assert!(!l.contains('\n') && !l.contains("abc123") && !l.contains("hunter2"));
        assert!(l.contains("TOKEN=[secret]") && !l.contains("sk-abcdef") && !l.contains("ghp_xyz"));
        assert!(!one_line("Reading crates/daemon/src/views.rs").contains("secret"));
        let long = one_line(&"word ".repeat(100));
        assert_eq!(long.chars().count(), LINE_MAX);
    }

    #[test]
    fn terminal_streams_lose_control_sequences_in_the_live_log_only() {
        assert_eq!(
            strip_terminal("\u{1b}[1mTest Case\u{1b}[0m 'A.a' started.\r"),
            "Test Case 'A.a' started."
        );
        assert_eq!(
            strip_terminal("a\u{1b}]0;title\u{7}b\u{1b}]1;x\u{1b}\\c"),
            "abc"
        );
        let mut p = Progress::new(1, "t", 0);
        p.terminal.insert("s".into());
        let raw = b"\x1b[32mgreen\x1b[0m\r\n";
        p.output("s", 0, raw, 1);
        p.output("plain", 0, raw, 1);
        assert_eq!(p.pending_log[0], "green");
        assert_eq!(p.message, "green");
        assert!(p.pending_log[1].contains('\u{1b}'));
        assert_eq!(p.output.bytes, 2 * raw.len() as u64);
    }
}
