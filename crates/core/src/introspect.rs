// SPDX-License-Identifier: GPL-3.0-or-later

//! Introspection (Part 14), as pure functions: telling a permanent error from a transient
//! one, the rule-based detectors, the arithmetic of the digest, whether a landed fix moved
//! its metric, the difference between a baseline and a red base, the checks on what the
//! analyst agent writes, and which triage answers go to the owner for review.
//! No I/O and no clock: the daemon reads the recorded data and passes it in.

use crate::proposal::evidence_ok;
use std::collections::{BTreeMap, BTreeSet};

// ---- recorded attempts -------------------------------------------------------------------

/// One attempt at one step, as recorded when it ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attempt {
    pub task: u64,
    pub project: String,
    pub step: String,
    /// `green`, `red` or `fail`.
    pub outcome: String,
    pub reason: String,
    /// The reason with numbers, hashes and paths taken out, so equal failures compare equal.
    pub signature: String,
    /// How many times this step had run for the task, this one included (1 is the first try).
    pub retry_n: u32,
    pub at: u64,
}

impl Attempt {
    pub fn failed(&self) -> bool {
        self.outcome == "fail"
    }
    /// A second or later try of a step.
    pub fn is_retry(&self) -> bool {
        self.retry_n >= 2
    }
}

/// A failure reason reduced to what stays the same between occurrences: lower case, with
/// every run of digits or hex, and every path, replaced by `#` or `<path>`.
pub fn signature(reason: &str) -> String {
    let mut words: Vec<String> = vec![];
    for raw in reason.split_whitespace() {
        let w = raw.trim_matches(|c: char| ",;:.()\"'`".contains(c));
        let lower = w.to_lowercase();
        let is_path = lower.contains('/') && !lower.contains(' ');
        let hexish = lower.len() >= 7 && lower.chars().all(|c| c.is_ascii_hexdigit());
        let w = if is_path {
            "<path>".to_string()
        } else if hexish {
            "#".to_string()
        } else {
            let mut out = String::new();
            let mut digits = false;
            for c in lower.chars() {
                if c.is_ascii_digit() {
                    if !digits {
                        out.push('#');
                    }
                    digits = true;
                } else {
                    digits = false;
                    out.push(c);
                }
            }
            out
        };
        if !w.is_empty() {
            words.push(w);
        }
    }
    words.join(" ")
}

/// Words that say a failure will happen again however often it is tried.
const PERMANENT: [&str; 17] = [
    "not found",
    "no such file",
    "does not exist",
    "permission denied",
    "unauthorized",
    "forbidden",
    "authentication",
    "invalid api key",
    "unknown model",
    "not supported",
    "unsupported",
    "no account",
    "is not configured",
    "no command",
    "malformed",
    "invalid",
    "command not found",
];

/// Words that say it may pass by itself. A reason with both is transient: the safer reading,
/// because retrying a transient error is right and calling a permanent one transient only
/// misses a finding.
const TRANSIENT: [&str; 12] = [
    "timed out",
    "timeout",
    "ran out of time",
    "rate limit",
    "usage limit",
    "429",
    "503",
    "502",
    "connection reset",
    "temporarily",
    "try again",
    "overloaded",
];

/// Whether retrying this failure is pointless.
pub fn is_permanent(reason: &str) -> bool {
    let r = reason.to_lowercase();
    !TRANSIENT.iter().any(|t| r.contains(t)) && PERMANENT.iter().any(|p| r.contains(p))
}

/// The agent ended its turn without submitting.
pub fn is_no_result(reason: &str) -> bool {
    reason.contains("without submitting")
}

// ---- metrics ------------------------------------------------------------------------------

/// Every metric a finding may name.
pub const METRICS: [&str; 8] = [
    "retry_loop_rate",
    "no_result_rate",
    "repeat_failure_rate",
    "wait_share",
    "force_skip_rate",
    "landing_idle_ms",
    "expired_switches",
    "interventions_per_landed",
];

pub fn known_metric(m: &str) -> bool {
    METRICS.contains(&m)
}

/// A rate with the numbers behind it: no estimate, `num / den` over `n` samples.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Measure {
    pub num: f64,
    pub den: f64,
    /// How many records the rate is built from.
    pub n: u64,
}

impl Measure {
    pub fn rate(&self) -> f64 {
        if self.den <= 0.0 {
            0.0
        } else {
            self.num / self.den
        }
    }
}

/// What was recorded in a stretch of time: the input of every measure.
#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub attempts: Vec<Attempt>,
    pub landed: u64,
    pub force_skips: u64,
    pub interventions: u64,
    /// Milliseconds tasks spent waiting on something (a wait reason), and all active time.
    pub wait_ms: u64,
    pub active_ms: u64,
}

/// A metric's value over `stats`, if it can be measured over a window of recorded data.
/// `landing_idle_ms` and `expired_switches` are snapshots of now, not windows, so a fix to
/// them cannot be checked this way.
pub fn measure(metric: &str, s: &Stats) -> Option<Measure> {
    let n_att = s.attempts.len() as f64;
    let attempts = s.attempts.len() as u64;
    match metric {
        "retry_loop_rate" => Some(Measure {
            num: permanent_retries(&s.attempts) as f64,
            den: n_att,
            n: attempts,
        }),
        "no_result_rate" => Some(Measure {
            num: s
                .attempts
                .iter()
                .filter(|a| is_no_result(&a.reason))
                .count() as f64,
            den: n_att,
            n: attempts,
        }),
        "repeat_failure_rate" => {
            let mut by: BTreeMap<&str, u64> = BTreeMap::new();
            for a in s.attempts.iter().filter(|a| a.failed()) {
                *by.entry(a.signature.as_str()).or_default() += 1;
            }
            let repeated: u64 = by.values().filter(|n| **n >= 3).sum();
            Some(Measure {
                num: repeated as f64,
                den: n_att,
                n: attempts,
            })
        }
        "wait_share" => Some(Measure {
            num: s.wait_ms as f64,
            den: s.active_ms as f64,
            n: s.landed.max(attempts),
        }),
        "force_skip_rate" => Some(Measure {
            num: s.force_skips as f64,
            den: s.landed as f64,
            n: s.landed,
        }),
        "interventions_per_landed" => Some(Measure {
            num: s.interventions as f64,
            den: s.landed as f64,
            n: s.landed,
        }),
        _ => None,
    }
}

/// Failed retries of a permanent error.
pub fn permanent_retries(attempts: &[Attempt]) -> usize {
    attempts
        .iter()
        .filter(|a| a.failed() && a.is_retry() && is_permanent(&a.reason))
        .count()
}

// ---- detectors ----------------------------------------------------------------------------

/// The thresholds the detectors use. Decision 0048 explains the defaults.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Thresholds {
    /// Retries of one permanent error in one task before it is a loop.
    pub loop_per_task: usize,
    /// Retries of one permanent error across a project in the window.
    pub loop_per_project: usize,
    pub no_result_rate: f64,
    /// Fewest attempts a rate needs before a detector trusts it.
    pub min_samples: u64,
    pub wait_share: f64,
    /// The same failure signature this many times.
    pub same_signature: usize,
    pub force_skips: u64,
    pub landing_idle_ms: u64,
}

impl Default for Thresholds {
    fn default() -> Thresholds {
        Thresholds {
            loop_per_task: 3,
            loop_per_project: 5,
            no_result_rate: 0.3,
            min_samples: 5,
            wait_share: 0.6,
            same_signature: 3,
            force_skips: 3,
            landing_idle_ms: 10 * 60_000,
        }
    }
}

/// What a detector or the analyst found. It becomes a proposal with this evidence and metric.
#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    /// The rule that found it (`retry_loop`, ...), or `analyst`.
    pub rule: String,
    /// Part of the dedupe key: one finding per rule and subject while it is open.
    pub subject: String,
    pub project: String,
    pub title: String,
    pub body: String,
    pub evidence: String,
    /// The metric expected to move once this is fixed.
    pub metric: String,
    /// The metric as it stood when found.
    pub before: Option<Measure>,
}

impl Finding {
    /// A finding without evidence, or naming no known metric, is refused.
    pub fn check(&self) -> Result<(), String> {
        if self.title.trim().is_empty() || self.title.chars().count() > 120 {
            return Err("a finding needs a title of 1 to 120 characters".into());
        }
        if self.body.trim().is_empty() {
            return Err("a finding needs a body".into());
        }
        evidence_ok(&self.evidence)?;
        if !known_metric(&self.metric) {
            return Err(format!(
                "unknown metric {:?}: name one of {}",
                self.metric,
                METRICS.join(", ")
            ));
        }
        Ok(())
    }
}

/// Where a rule points a reader, so a proposal's evidence names a file.
fn pointer(rule: &str) -> &'static str {
    match rule {
        "retry_loop" | "repeat_failure" => "crates/daemon/src/jobs.rs",
        "no_result" => "crates/daemon/src/jobs.rs",
        "wait_share" => "crates/daemon/src/sched.rs",
        "landing_idle" => "crates/daemon/src/landing.rs",
        "force_skips" => "crates/daemon/src/suites.rs",
        _ => "crates/daemon/src/sweep.rs",
    }
}

fn list<T: ToString>(it: impl IntoIterator<Item = T>, max: usize) -> String {
    let all: Vec<String> = it.into_iter().map(|x| x.to_string()).collect();
    let mut s = all.iter().take(max).cloned().collect::<Vec<_>>().join(", ");
    if all.len() > max {
        s += &format!(" and {} more", all.len() - max);
    }
    s
}

/// What the daily detectors look at besides attempts.
#[derive(Clone, Debug, Default)]
pub struct Facts {
    /// Tasks waiting to land right now, and how long each project's landing queue has idled.
    pub landing_ready: u64,
    pub landing_idle_ms: u64,
    /// `suite -> times it was skipped by force` in the window.
    pub force_skips: BTreeMap<String, u64>,
    /// Switches (overrides, quarantine) still present after their expiry.
    pub expired_switches: Vec<String>,
}

/// Run every detector over a window of one project's recorded data. The order is fixed so
/// the same history always gives the same findings.
pub fn detect(project: &str, s: &Stats, f: &Facts, t: &Thresholds) -> Vec<Finding> {
    let mut out = vec![];
    let mk = |rule: &str,
              subject: String,
              title: String,
              body: String,
              evidence: String,
              metric: &str,
              before: Option<Measure>| Finding {
        rule: rule.into(),
        subject,
        project: project.into(),
        title,
        body,
        evidence: format!("{evidence}; look at {}", pointer(rule)),
        metric: metric.into(),
        before,
    };

    // A retry loop on a permanent error: the same permanent failure retried again and again.
    let mut per_task: BTreeMap<(u64, &str), Vec<&Attempt>> = BTreeMap::new();
    let mut per_sig: BTreeMap<&str, Vec<&Attempt>> = BTreeMap::new();
    for a in s
        .attempts
        .iter()
        .filter(|a| a.failed() && a.is_retry() && is_permanent(&a.reason))
    {
        per_task
            .entry((a.task, a.signature.as_str()))
            .or_default()
            .push(a);
        per_sig.entry(a.signature.as_str()).or_default().push(a);
    }
    let mut looped: BTreeSet<&str> = BTreeSet::new();
    for ((task, sig), rows) in &per_task {
        if rows.len() >= t.loop_per_task {
            looped.insert(sig);
            let _ = task;
        }
    }
    for (sig, rows) in &per_sig {
        if rows.len() >= t.loop_per_project {
            looped.insert(sig);
        }
    }
    for sig in looped {
        let rows = &per_sig[sig];
        let tasks: BTreeSet<u64> = rows.iter().map(|a| a.task).collect();
        let step = &rows[0].step;
        out.push(mk(
            "retry_loop",
            sig.to_string(),
            format!("Retry loop on a permanent error: {}", short(sig)),
            format!(
                "Step `{step}` was retried {} times after failing with {:?}, an error no retry can fix, in task(s) {}. Retrying a permanent error wastes a slot and a session each time; a step should fail at once, or ask, when the reason will not change.",
                rows.len(),
                rows[0].reason,
                list(tasks.iter().map(|t| format!("#{t}")), 6)
            ),
            format!(
                "step_attempts rows {} (retry_n {}) in the store",
                list(rows.iter().map(|a| format!("task {} step {}", a.task, a.step)), 4),
                list(rows.iter().map(|a| a.retry_n), 6)
            ),
            "retry_loop_rate",
            measure("retry_loop_rate", s),
        ));
    }

    // The same failure signature, at least `same_signature` times (not already a loop above).
    let mut by_sig: BTreeMap<&str, Vec<&Attempt>> = BTreeMap::new();
    for a in s.attempts.iter().filter(|a| a.failed()) {
        by_sig.entry(a.signature.as_str()).or_default().push(a);
    }
    for (sig, rows) in by_sig {
        let is_loop = is_permanent(&rows[0].reason)
            && rows.iter().filter(|a| a.is_retry()).count() >= t.loop_per_task;
        if rows.len() >= t.same_signature
            && !sig.is_empty()
            && !is_no_result(&rows[0].reason)
            && !is_loop
        {
            out.push(mk(
                "repeat_failure",
                sig.to_string(),
                format!("The same failure {} times: {}", rows.len(), short(sig)),
                format!(
                    "Step `{}` failed {} times with the same reason, {:?}. Repeating failures with one signature usually means a cause nobody has fixed.",
                    rows[0].step,
                    rows.len(),
                    rows[0].reason
                ),
                format!(
                    "step_attempts rows for tasks {} in the store",
                    list(rows.iter().map(|a| format!("#{}", a.task)), 6)
                ),
                "repeat_failure_rate",
                measure("repeat_failure_rate", s),
            ));
        }
    }

    // Step no-result rate.
    if let Some(m) = measure("no_result_rate", s) {
        if m.n >= t.min_samples && m.rate() > t.no_result_rate {
            out.push(mk(
                "no_result",
                String::new(),
                format!("Agents end their turn without a result in {:.0}% of attempts", m.rate() * 100.0),
                format!(
                    "{} of {} step attempts in the window ended with the agent not submitting; the threshold is {:.0}%.",
                    m.num, m.den, t.no_result_rate * 100.0
                ),
                format!("step_attempts rows with \"without submitting\" ({} of {})", m.num, m.den),
                "no_result_rate",
                Some(m),
            ));
        }
    }

    // Share of time spent waiting.
    if let Some(m) = measure("wait_share", s) {
        if s.active_ms > 0 && m.n >= t.min_samples && m.rate() > t.wait_share {
            out.push(mk(
                "wait_share",
                String::new(),
                format!("Tasks spend {:.0}% of their time waiting", m.rate() * 100.0),
                format!(
                    "{} ms of {} ms active time was spent waiting on a resource, a slot or the owner; the threshold is {:.0}%.",
                    s.wait_ms, s.active_ms, t.wait_share * 100.0
                ),
                format!("wait events in the event log: {} ms waiting of {} ms", s.wait_ms, s.active_ms),
                "wait_share",
                Some(m),
            ));
        }
    }

    // Ready tasks waiting while the landing queue idles.
    if f.landing_ready > 0 && f.landing_idle_ms >= t.landing_idle_ms {
        out.push(mk(
            "landing_idle",
            String::new(),
            "Tasks wait to land while the landing queue idles".to_string(),
            format!(
                "{} task(s) are ready to land and the landing queue has been idle for {} ms (threshold {} ms).",
                f.landing_ready, f.landing_idle_ms, t.landing_idle_ms
            ),
            format!("landing queue idle {} ms with {} ready", f.landing_idle_ms, f.landing_ready),
            "landing_idle_ms",
            Some(Measure {
                num: f.landing_idle_ms as f64,
                den: 1.0,
                n: 1,
            }),
        ));
    }

    // Frequent force-skips of one suite.
    for (suite, n) in &f.force_skips {
        if *n >= t.force_skips {
            out.push(mk(
                "force_skips",
                suite.clone(),
                format!("Suite {suite} is force-skipped again and again"),
                format!("Suite `{suite}` was skipped by force {n} times in the window (threshold {}). Either the suite is wrong for the work, or something makes it fail that should be fixed.", t.force_skips),
                format!("debts for suite:{suite} ({n} in the window) in the store"),
                "force_skip_rate",
                measure("force_skip_rate", s),
            ));
        }
    }

    // Switches that expired and are still there.
    if !f.expired_switches.is_empty() {
        out.push(mk(
            "expired_switches",
            String::new(),
            "Switches are past their expiry and still there".to_string(),
            format!(
                "These overrides or quarantines expired and have not been removed or renewed: {}.",
                list(&f.expired_switches, 6)
            ),
            format!("expired entries: {}", list(&f.expired_switches, 6)),
            "expired_switches",
            Some(Measure {
                num: f.expired_switches.len() as f64,
                den: 1.0,
                n: 1,
            }),
        ));
    }
    out
}

fn short(s: &str) -> String {
    let t: String = s.chars().take(70).collect();
    t.trim().to_string()
}

// ---- closing the loop ---------------------------------------------------------------------

/// What the cycle after a fix landed says about it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Verdict {
    /// Not enough recorded data since the fix landed to say.
    Pending {
        have: u64,
        need: u64,
    },
    Worked {
        before: f64,
        after: f64,
    },
    DidntHelp {
        before: f64,
        after: f64,
    },
    /// The metric has no windowed measure, so no verdict can be computed.
    Unmeasured,
}

/// The fraction by which a metric must fall for a fix to count as having worked.
pub const WORKED_DROP: f64 = 0.5;

/// A fix worked if, with at least `min_samples` recorded since it landed, the metric fell by
/// at least half. Anything else, with enough data, did not help.
pub fn verdict(before: Option<Measure>, after: Option<Measure>, min_samples: u64) -> Verdict {
    let (Some(b), Some(a)) = (before, after) else {
        return Verdict::Unmeasured;
    };
    if a.n < min_samples {
        return Verdict::Pending {
            have: a.n,
            need: min_samples,
        };
    }
    let (rb, ra) = (b.rate(), a.rate());
    if rb > 0.0 && ra <= rb * (1.0 - WORKED_DROP) {
        Verdict::Worked {
            before: rb,
            after: ra,
        }
    } else {
        Verdict::DidntHelp {
            before: rb,
            after: ra,
        }
    }
}

impl Verdict {
    pub fn sentence(&self, metric: &str) -> String {
        match self {
            Verdict::Pending { have, need } => {
                format!("waiting for data: {have} of {need} samples of {metric} since it landed")
            }
            Verdict::Worked { before, after } => {
                format!("this fix worked: {metric} went from {before:.3} to {after:.3}")
            }
            Verdict::DidntHelp { before, after } => {
                format!("this fix didn't help: {metric} went from {before:.3} to {after:.3}")
            }
            Verdict::Unmeasured => {
                format!("{metric} is a snapshot and has no window to compare; check it by hand")
            }
        }
    }
    pub fn name(&self) -> &'static str {
        match self {
            Verdict::Pending { .. } => "pending",
            Verdict::Worked { .. } => "worked",
            Verdict::DidntHelp { .. } => "didnt_help",
            Verdict::Unmeasured => "unmeasured",
        }
    }
}

// ---- the digest's arithmetic --------------------------------------------------------------

/// The median of recorded values, or `None` for none. For an even count, the mean of the
/// middle two.
pub fn median(mut v: Vec<u64>) -> Option<u64> {
    if v.is_empty() {
        return None;
    }
    v.sort_unstable();
    let m = v.len() / 2;
    Some(if v.len() % 2 == 1 {
        v[m]
    } else {
        (v[m - 1] + v[m]) / 2
    })
}

/// `a / b`, or `None` if `b` is zero (no division by zero, no made-up zero).
pub fn per(a: u64, b: u64) -> Option<f64> {
    (b > 0).then(|| a as f64 / b as f64)
}

// ---- base health --------------------------------------------------------------------------

/// The tests failing now that the baseline did not list: the only ones that file tasks.
pub fn new_failures(current: &[String], baseline: &[String]) -> Vec<String> {
    let known: BTreeSet<&str> = baseline.iter().map(String::as_str).collect();
    let mut seen = BTreeSet::new();
    current
        .iter()
        .filter(|t| !known.contains(t.as_str()) && seen.insert(t.as_str()))
        .cloned()
        .collect()
}

// ---- schedules ----------------------------------------------------------------------------

/// `daily`, `weekly`, `hourly` or a number with `m`, `h` or `d`, as milliseconds.
pub fn parse_every(s: &str) -> Option<u64> {
    const MIN: u64 = 60_000;
    match s.trim() {
        "hourly" => return Some(60 * MIN),
        "daily" | "nightly" => return Some(24 * 60 * MIN),
        "weekly" => return Some(7 * 24 * 60 * MIN),
        _ => {}
    }
    let t = s.trim();
    let (num, unit) = t.split_at(t.len().checked_sub(1)?);
    let n: u64 = num.parse().ok().filter(|n| *n >= 1)?;
    match unit {
        "s" => Some(n * 1000),
        "m" => Some(n * MIN),
        "h" => Some(n * 60 * MIN),
        "d" => Some(n * 24 * 60 * MIN),
        _ => None,
    }
}

/// Is a schedule due? One that never ran is due at once.
pub fn due(last_run: Option<u64>, now: u64, every_ms: u64) -> bool {
    last_run.is_none_or(|l| now.saturating_sub(l) >= every_ms)
}

// ---- the analyst's output -----------------------------------------------------------------

/// A finding the analyst wrote, before it is checked.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Draft {
    pub title: String,
    pub body: String,
    pub evidence: String,
    pub metric: String,
}

/// Turn the analyst's drafts into findings; every one that fails its check is refused, with
/// the reason, and nothing from it is kept.
pub fn accept_drafts(project: &str, drafts: &[Draft]) -> (Vec<Finding>, Vec<(String, String)>) {
    let (mut ok, mut refused) = (vec![], vec![]);
    for d in drafts {
        let f = Finding {
            rule: "analyst".into(),
            subject: crate::proposal::normalise(&d.title),
            project: project.into(),
            title: d.title.trim().into(),
            body: d.body.trim().into(),
            evidence: d.evidence.trim().into(),
            metric: d.metric.trim().into(),
            before: None,
        };
        match f.check() {
            Ok(()) => ok.push(f),
            Err(why) => refused.push((d.title.clone(), why)),
        }
    }
    (ok, refused)
}

// ---- triage -------------------------------------------------------------------------------

/// Whether the answer to question `id` is queued for the owner's review: a fixed share,
/// chosen by a hash of the id so the same question is always in or out, and every answer
/// the triage agent was unsure of. With `percent` of 100 all are reviewed.
pub fn sample_for_review(id: u64, percent: u32, unsure: bool) -> bool {
    if unsure {
        return true;
    }
    let h = id.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 33;
    (h % 100) < u64::from(percent.min(100))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn att(task: u64, outcome: &str, reason: &str, retry_n: u32, at: u64) -> Attempt {
        Attempt {
            task,
            project: "p".into(),
            step: "develop".into(),
            outcome: outcome.into(),
            reason: reason.into(),
            signature: signature(reason),
            retry_n,
            at,
        }
    }

    #[test]
    fn signatures_ignore_numbers_paths_and_hashes() {
        assert_eq!(
            signature("Cannot open /tmp/x/42/log.txt: no such file (code 2)"),
            signature("cannot open /var/y/7/log.txt: no such file (code 13)")
        );
        assert_ne!(signature("model not found"), signature("quota exceeded"));
        assert_eq!(signature("failed at deadbeef12"), "failed at #");
    }

    #[test]
    fn permanent_and_transient_errors_are_told_apart() {
        assert!(is_permanent("the agent failed: unknown model fable-9"));
        assert!(is_permanent("permission denied"));
        assert!(!is_permanent("request timed out: not found in cache"));
        assert!(!is_permanent("rate limit, try again"));
        assert!(!is_permanent("something odd"));
    }

    fn loop_stats(n: u32) -> Stats {
        Stats {
            attempts: (1..=n)
                .map(|i| {
                    att(
                        3,
                        "fail",
                        "the agent failed: unknown model m-x",
                        i,
                        u64::from(i),
                    )
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn a_retry_loop_on_a_permanent_error_is_found() {
        let found = detect(
            "p",
            &loop_stats(5),
            &Facts::default(),
            &Thresholds::default(),
        );
        let f = found
            .iter()
            .find(|f| f.rule == "retry_loop")
            .expect("found");
        assert_eq!(f.metric, "retry_loop_rate");
        assert!(f.check().is_ok(), "{:?}", f.check());
        // retry_n 2..=5 are retries: 4 of 5 attempts
        let b = f.before.unwrap();
        assert_eq!((b.num, b.den), (4.0, 5.0));
    }

    #[test]
    fn two_retries_are_not_a_loop_and_transient_errors_never_are() {
        let found = detect(
            "p",
            &loop_stats(3),
            &Facts::default(),
            &Thresholds::default(),
        );
        assert!(found.iter().all(|f| f.rule != "retry_loop"));
        let mut s = loop_stats(8);
        for a in &mut s.attempts {
            a.reason = "the step ran out of time".into();
            a.signature = signature(&a.reason);
        }
        let found = detect("p", &s, &Facts::default(), &Thresholds::default());
        assert!(found.iter().all(|f| f.rule != "retry_loop"));
    }

    #[test]
    fn every_part_14_example_has_a_rule() {
        let mut s = Stats {
            attempts: (0..10)
                .map(|i| {
                    att(
                        i,
                        "fail",
                        "the agent ended its turn 3 times without submitting",
                        1,
                        i,
                    )
                })
                .collect(),
            wait_ms: 900,
            active_ms: 1000,
            landed: 10,
            ..Default::default()
        };
        s.attempts
            .push(att(20, "fail", "build broke at link", 1, 1));
        s.attempts
            .push(att(21, "fail", "build broke at link", 1, 2));
        s.attempts
            .push(att(22, "fail", "build broke at link", 1, 3));
        let f = Facts {
            landing_ready: 2,
            landing_idle_ms: 20 * 60_000,
            force_skips: [("slow".to_string(), 4)].into(),
            expired_switches: vec!["quarantine api::flaky".into()],
        };
        let rules: BTreeSet<String> = detect("p", &s, &f, &Thresholds::default())
            .into_iter()
            .map(|f| f.rule)
            .collect();
        for r in [
            "no_result",
            "wait_share",
            "repeat_failure",
            "landing_idle",
            "force_skips",
            "expired_switches",
        ] {
            assert!(rules.contains(r), "missing {r}: {rules:?}");
        }
        let looped = detect(
            "p",
            &loop_stats(5),
            &Facts::default(),
            &Thresholds::default(),
        );
        assert!(looped.iter().any(|f| f.rule == "retry_loop"));
    }

    #[test]
    fn findings_need_evidence_and_a_known_metric() {
        let mut f = detect(
            "p",
            &loop_stats(5),
            &Facts::default(),
            &Thresholds::default(),
        )
        .remove(0);
        assert!(f.check().is_ok());
        f.evidence = "it feels slow somewhere".into();
        assert!(f.check().is_err());
        f.evidence = "step_attempts in jobs.rs:10 shows the loop".into();
        f.metric = "vibes".into();
        assert!(f.check().unwrap_err().contains("unknown metric"));
        f.metric = "wait_share".into();
        f.evidence = String::new();
        assert!(f.check().is_err());
    }

    #[test]
    fn the_verdict_needs_data_and_a_halving() {
        let m = |num, den, n| Some(Measure { num, den, n });
        let before = m(4.0, 5.0, 5);
        assert_eq!(
            verdict(before, m(0.0, 3.0, 3), 5),
            Verdict::Pending { have: 3, need: 5 }
        );
        assert!(matches!(
            verdict(before, m(0.0, 8.0, 8), 5),
            Verdict::Worked { .. }
        ));
        assert!(matches!(
            verdict(before, m(4.0, 8.0, 8), 5),
            Verdict::DidntHelp { .. }
        ));
        // exactly half counts; a metric that was already zero cannot have been fixed
        assert!(matches!(
            verdict(m(1.0, 2.0, 2), m(1.0, 4.0, 4), 4),
            Verdict::Worked { .. }
        ));
        assert!(matches!(
            verdict(m(0.0, 5.0, 5), m(0.0, 8.0, 8), 5),
            Verdict::DidntHelp { .. }
        ));
        assert_eq!(verdict(None, m(0.0, 8.0, 8), 5), Verdict::Unmeasured);
        assert!(Verdict::Worked {
            before: 0.8,
            after: 0.0
        }
        .sentence("retry_loop_rate")
        .contains("this fix worked"));
        assert!(Verdict::DidntHelp {
            before: 0.8,
            after: 0.8
        }
        .sentence("retry_loop_rate")
        .contains("this fix didn't help"));
    }

    #[test]
    fn digest_arithmetic_makes_nothing_up() {
        assert_eq!(median(vec![]), None);
        assert_eq!(median(vec![5, 1, 9]), Some(5));
        assert_eq!(median(vec![2, 4]), Some(3));
        assert_eq!(per(3, 0), None);
        assert_eq!(per(3, 2), Some(1.5));
    }

    #[test]
    fn only_new_failures_are_new() {
        let base: Vec<String> = vec!["a".into(), "b".into()];
        let now: Vec<String> = vec!["b".into(), "c".into(), "c".into(), "a".into()];
        assert_eq!(new_failures(&now, &base), vec!["c".to_string()]);
        assert!(new_failures(&base, &base).is_empty());
    }

    #[test]
    fn schedules_parse_and_come_due() {
        assert_eq!(parse_every("daily"), Some(86_400_000));
        assert_eq!(parse_every("6h"), Some(21_600_000));
        assert_eq!(parse_every("30m"), Some(1_800_000));
        assert_eq!(parse_every("0h"), None);
        assert_eq!(parse_every("soon"), None);
        assert!(due(None, 5, 100));
        assert!(!due(Some(10), 50, 100));
        assert!(due(Some(10), 110, 100));
    }

    #[test]
    fn the_analyst_is_held_to_the_same_rules() {
        let good = Draft {
            title: "Reviews wait for a slot".into(),
            body: "Review steps queue behind develop.".into(),
            evidence: "crates/daemon/src/sched.rs:120 ranks review below develop".into(),
            metric: "wait_share".into(),
        };
        let no_evidence = Draft {
            evidence: "I think so".into(),
            ..good.clone()
        };
        let bad_metric = Draft {
            metric: "happiness".into(),
            ..good.clone()
        };
        let (ok, refused) = accept_drafts("p", &[good, no_evidence, bad_metric]);
        assert_eq!(ok.len(), 1);
        assert_eq!(refused.len(), 2);
        assert!(refused[1].1.contains("unknown metric"));
    }

    #[test]
    fn triage_samples_are_a_stable_share_and_unsure_ones_always() {
        let n = (0..1000)
            .filter(|i| sample_for_review(*i, 20, false))
            .count();
        assert!((120..=280).contains(&n), "{n}");
        assert_eq!(
            sample_for_review(7, 20, false),
            sample_for_review(7, 20, false)
        );
        assert!(sample_for_review(7, 0, true));
        assert!(!sample_for_review(7, 0, false));
        assert!((0..50).all(|i| sample_for_review(i, 100, false)));
    }
}
