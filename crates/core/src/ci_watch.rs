// SPDX-License-Identifier: GPL-3.0-or-later

//! The rules of the base-branch CI watch (decision 0149), with no I/O: runs and jobs
//! in, what to file out. The clock is a parameter, so the long-red rule is tested with
//! an injected one.
//!
//! The rules, ported from starpipe's `watch_develop` (not its code):
//!
//! - **Seeding.** The first time a workflow is seen, its newest run that ended `success`
//!   or `failure` sets each job's baseline. Nothing is filed; a job already red is the
//!   known-red ledger.
//! - **Passed over.** A run that ended otherwise (cancelled by a newer push, ...) says
//!   nothing about the code. A job that is now `skipped` or `neutral` is no longer red.
//! - **Infrastructure is not a regression.** `cancelled` and `startup_failure` are never
//!   filed. A job that hit its GitHub time limit is not infrastructure on the base branch:
//!   the daemon reads it as `timed_out` (a `cancelled` job inside a finished run whose
//!   annotation says it exceeded the maximum execution time; decision 0660), and `agreement`
//!   distinct runs that timed out with no finished result between file it once, as its own
//!   condition ("exceeds its time limit"). A success or failure clears the count.
//! - **The gate acts at once.** A gate job going red after green is filed (urgent) on that
//!   one run, and holds the freeze until the job is green again.
//! - **Other jobs need agreement.** The first red after green is pending; `agreement`
//!   distinct runs red with no green between file it. A green clears it as a flake.
//! - **Long reds.** A job red for `red_ticket_after` with no open ticket naming it is
//!   filed, and looked at again only after another threshold, and then only for a run
//!   newer than the one its last ticket covered (and, if that ticket landed a change, of
//!   a commit at or after the landing). The same run is never filed twice.
//! - **Dedupe and caps.** Nothing is filed while an open task or issue names the job; with
//!   `max_open_auto` auto-filed tasks open, a red is filed `proposed`.
//! - **Excluded workflows** keep their baseline but are never filed and never freeze.

use std::collections::{BTreeMap, BTreeSet};

/// What the definition's `[watch.ci]` says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rules {
    /// Workflows to watch (run name or workflow file name); empty means all.
    pub workflows: Vec<String>,
    /// Workflows kept for their baseline but never filed.
    pub exclude: Vec<String>,
    /// `workflow` (every job in it) or `workflow/job`.
    pub gate: Option<String>,
    /// Distinct red runs, with no green between, before a non-gate job is filed.
    pub agreement: u32,
    pub red_ticket_after_ms: u64,
    pub max_open_auto: u32,
    pub freeze_on_gate: bool,
    /// `auto_approve = "confirmed"` (decision 0798): a red seen on `agreement` finished runs
    /// that ran the job is filed approved; every other red is filed `proposed`.
    pub auto_approve: bool,
    /// With `auto_approve`, how many such tickets are open at once; beyond it they stay proposed.
    pub auto_approve_max: u32,
}

impl Rules {
    pub fn watches(&self, workflow: &str) -> bool {
        self.workflows.is_empty() || self.workflows.iter().any(|w| w == workflow)
    }
    pub fn excluded(&self, workflow: &str) -> bool {
        self.exclude.iter().any(|w| w == workflow)
    }
    pub fn is_gate(&self, workflow: &str, job: &str) -> bool {
        match &self.gate {
            None => false,
            Some(g) => match g.split_once('/') {
                Some((w, j)) => w == workflow && j == job,
                None => g == workflow,
            },
        }
    }
}

/// The key a job goes by in titles and dedupe: `workflow/job`.
pub fn key(workflow: &str, job: &str) -> String {
    format!("{workflow}/{job}")
}

/// The shard prefix of a matrix job named `name (os, N)`: `name (os, `. `None` for a job that
/// does not end in a shard number.
fn shard_prefix(job: &str) -> Option<&str> {
    let inner = job.strip_suffix(')')?;
    let (head, n) = inner.rsplit_once(", ")?;
    if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(&job[..head.len() + 2])
}

/// The key shared by the shards of one matrix job: `CI/exit-scripts (ubuntu-latest, *)` for
/// `CI/exit-scripts (ubuntu-latest, 5)`; any other job's own key.
pub fn group_key(workflow: &str, job: &str) -> String {
    match shard_prefix(job) {
        Some(p) => format!("{workflow}/{p}*)"),
        None => key(workflow, job),
    }
}

/// Whether `text` names the job: its own key or its shard group's.
fn names_job(text: &str, workflow: &str, job: &str) -> bool {
    if text.contains(&key(workflow, job)) {
        return true;
    }
    match shard_prefix(job) {
        Some(p) => text.contains(&format!("{workflow}/{p}")),
        None => false,
    }
}

/// What failed in a job's log: the `FAILED: scripts/exit/sNN.sh` lines of the exit-script
/// runner, the `test x ... FAILED` names and the `error: test failed, to rerun pass '...'`
/// lines of cargo. Sorted and without repeats; empty for any other log.
pub fn signatures(log: &str) -> Vec<String> {
    let mut v: BTreeSet<String> = BTreeSet::new();
    for l in log.lines() {
        if let Some(i) = l.find("FAILED: scripts/exit/") {
            let rest = &l[i + "FAILED: ".len()..];
            if let Some(w) = rest.split_whitespace().next() {
                v.insert(w.to_string());
            }
        } else if let Some(i) = l.find(" ... FAILED") {
            let head = &l[..i];
            if let Some(j) = head.rfind("test ") {
                let name = head[j + 5..].trim();
                if !name.is_empty() && !name.contains(char::is_whitespace) {
                    v.insert(name.to_string());
                }
            }
        } else if let Some(i) = l.find("error: test failed, to rerun pass '") {
            let rest = &l[i + "error: test failed, to rerun pass '".len()..];
            if let Some(e) = rest.find('\'') {
                let a = rest[..e].trim();
                if !a.is_empty() {
                    v.insert(a.to_string());
                }
            }
        }
    }
    v.into_iter().collect()
}

/// A signature as a person says it: `scripts/exit/s486.sh` is `s486`.
pub fn short(sig: &str) -> &str {
    sig.strip_prefix("scripts/exit/")
        .and_then(|s| s.strip_suffix(".sh"))
        .unwrap_or(sig)
}

/// Whether `text` names the failure `sig`: the full signature or its short form, as a whole
/// word (`s48` is not in `s486`).
pub fn mentions(text: &str, sig: &str) -> bool {
    let word = |needle: &str| {
        let ok = |c: Option<char>| c.is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
        text.match_indices(needle).any(|(i, m)| {
            ok(text[..i].chars().next_back()) && ok(text[i + m.len()..].chars().next())
        })
    };
    word(sig) || word(short(sig))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobRec {
    pub id: u64,
    pub name: String,
    /// `None`: not finished.
    pub conclusion: Option<String>,
    /// What failed, read from the log of a failed job (see `signatures`).
    pub signatures: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunRec {
    pub id: u64,
    pub workflow: String,
    pub sha: String,
    pub url: String,
    pub created_ms: u64,
    /// `None`: not finished.
    pub conclusion: Option<String>,
    /// A full run (dispatched, s800) rather than a push run: only it has the full-only jobs.
    pub full: bool,
    pub jobs: Vec<JobRec>,
}

/// Where a red was seen.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct RunRef {
    pub run: u64,
    pub job: u64,
    pub sha: String,
    pub url: String,
    /// When the run was created (ms).
    pub at_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct JobState {
    pub red: bool,
    /// Already red when first seen: the known-red ledger. Never agreed into a ticket.
    pub known: bool,
    pub red_since: u64,
    /// Distinct runs it was red in, since the last green.
    pub pending: Vec<u64>,
    /// The latest run it was red in.
    pub last_red: RunRef,
    /// What failed in its latest red run (scripts, tests), when the log said.
    pub signatures: Vec<String>,
    /// The task filed (or found open) for it.
    pub ticket: Option<u64>,
    /// When it was last filed or looked at for the long-red rule.
    pub checked: Option<u64>,
    /// The gate went red and the freeze holds until it is green.
    pub freeze: bool,
    /// The newest run a ticket was filed for or found for (0: none known). The long-red
    /// rule files again only for a run newer than this.
    pub covered: u64,
    /// When that ticket landed with a change, once seen: a newer run must be of a commit
    /// at or after it.
    pub landed_ms: Option<u64>,
    /// Distinct runs it timed out in, since the last success or failure.
    pub timeouts: Vec<u64>,
    /// The latest run it timed out in.
    pub last_timeout: RunRef,
    /// The timeout was filed (or found named): nothing more until a result clears it.
    pub timeout_filed: bool,
    /// Finished runs of its workflow, in a row, that had no job of this name. At
    /// `agreement + 2` the job is forgotten (removed or renamed).
    pub absent: u32,
    /// The kind of run it last appeared in: a full run (true) or a push run. A job is absent
    /// only from a run of its own kind: a push run lacking the full-only jobs says nothing
    /// about them, and a full run lacking the fast jobs says nothing about those (s800).
    pub full: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct WorkflowState {
    pub last_run: u64,
    pub jobs: BTreeMap<String, JobState>,
}

/// Something filed, for the status.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Filed {
    pub key: String,
    pub task: Option<u64>,
    pub at: u64,
    pub urgent: bool,
    pub proposed: bool,
    /// Filed approved by `auto_approve`, so the owner can see and cancel it.
    pub auto: bool,
    pub url: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct State {
    /// Only workflows that were seeded.
    pub workflows: BTreeMap<String, WorkflowState>,
    pub filed: Vec<Filed>,
}

/// What a filing is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    Gate,
    Agreed,
    LongRed,
    /// The job hit its time limit in `agreement` runs.
    Timeout,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct File {
    pub workflow: String,
    pub job: String,
    pub urgent: bool,
    /// Past the cap: filed `proposed`.
    pub proposed: bool,
    /// Filed approved because the red was confirmed (`auto_approve`). The daemon turns it
    /// back into `proposed` when the ticket names a trust file.
    pub auto: bool,
    pub why: Why,
    pub at: RunRef,
    /// What failed (scripts, tests), merged over the shards filed together.
    pub signatures: Vec<String>,
    /// Other shards of the same matrix job filed under this one.
    pub also: Vec<String>,
    /// Tasks that landed within the lookback naming one of the failures.
    pub repeats: Vec<Repeat>,
}

/// A landed task that named a failure which is red again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Repeat {
    pub task: u64,
    pub title: String,
    pub landed_ms: u64,
    /// The failures of the file it named.
    pub signatures: Vec<String>,
}

impl File {
    pub fn key(&self) -> String {
        key(&self.workflow, &self.job)
    }
    /// The key shared by the shards of one matrix job.
    pub fn group(&self) -> String {
        group_key(&self.workflow, &self.job)
    }
}

/// What the daemon knows that the rules do not: who already names a job, how many
/// auto-filed tasks are open, and what time it is.
pub struct Ctx<'a> {
    pub rules: &'a Rules,
    pub now_ms: u64,
    /// `workflow/job` to the open task or issue naming it (`None`: an issue, no task).
    pub named: &'a BTreeMap<String, Option<u64>>,
    pub open_auto: u32,
    /// How many tickets filed by `auto_approve` are still open.
    pub open_approved: u32,
    /// Tasks that landed or closed within the lookback.
    pub recent: &'a [RecentTask],
    /// Tasks that landed with a change, to when (ms).
    pub landed: &'a BTreeMap<u64, u64>,
    /// A failure signature to the open task or issue naming it (`None`: an issue).
    pub named_sigs: &'a BTreeMap<String, Option<u64>>,
}

/// A task that is over (landed or closed) and recent enough to count as having covered a red.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecentTask {
    pub id: u64,
    pub title: String,
    pub body: String,
}

impl RecentTask {
    /// Its body cites this run.
    pub fn cites(&self, run: u64) -> bool {
        run != 0 && self.body.contains(&format!("/runs/{run}"))
    }
}

/// The runs that need their jobs read: for a workflow not yet seeded, the newest finished
/// success or failure; otherwise every finished one newer than the last processed.
pub fn needs_jobs(state: &State, rules: &Rules, runs: &[RunRec]) -> Vec<u64> {
    let mut out = vec![];
    for (w, rs) in by_workflow(rules, runs) {
        match state.workflows.get(&w) {
            None => out.extend(rs.last().map(|r| r.id)),
            Some(ws) => out.extend(rs.iter().filter(|r| r.id > ws.last_run).map(|r| r.id)),
        }
    }
    out
}

/// Finished success/failure runs of each watched workflow, oldest first.
fn by_workflow<'a>(rules: &Rules, runs: &'a [RunRec]) -> BTreeMap<String, Vec<&'a RunRec>> {
    let mut m: BTreeMap<String, Vec<&RunRec>> = BTreeMap::new();
    for r in runs {
        if rules.watches(&r.workflow)
            && matches!(r.conclusion.as_deref(), Some("success" | "failure"))
        {
            m.entry(r.workflow.clone()).or_default().push(r);
        }
    }
    for v in m.values_mut() {
        v.sort_by_key(|r| (r.created_ms, r.id));
    }
    m
}

fn job_run_ref(r: &RunRec, j: &JobRec) -> RunRef {
    RunRef {
        run: r.id,
        job: j.id,
        sha: r.sha.clone(),
        url: r.url.clone(),
        at_ms: r.created_ms,
    }
}

/// Take in the runs seen on the base branch. Returns what to file, in order. The state
/// is updated in place; the caller stores it only after acting. Runs whose jobs were not
/// read (empty `jobs`) are not counted as seen, so they are tried again.
/// Drop ANSI colour codes and a leading `2026-11-20T01:02:03.1234567Z ` timestamp.
fn clean_line(l: &str) -> String {
    let mut out = String::with_capacity(l.len());
    let mut it = l.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\u{1b}' && it.peek() == Some(&'[') {
            it.next();
            for d in it.by_ref() {
                if d.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    let b = out.as_bytes();
    let stamped = b.len() > 21
        && b[4] == b'-'
        && b[7] == b'-'
        && b[10] == b'T'
        && b[..20]
            .iter()
            .all(|c| c.is_ascii_digit() || b"-T:.".contains(c))
        && b[20..].iter().take(12).any(|&c| c == b'Z');
    if stamped {
        if let Some(i) = out.find('Z') {
            return out[i + 1..].trim_start().to_string();
        }
    }
    out
}

fn is_failure_line(l: &str) -> bool {
    const MARKERS: &[&str] = &[
        "error[",
        "error:",
        "panicked at",
        "FAILED",
        "failures:",
        "Diff in",
        "FAIL",
        "failed",
        "missing ",
        "FAILURE:",
        "e: ",
        "conformance failed",
        "XCTAssert",
        "failed (",
        "unexpected:",
    ];
    if l.contains("test result: ok") || l.contains(" 0 failed") {
        return false;
    }
    MARKERS.iter().any(|m| l.contains(m))
}

/// The lines of a job's full log that say what broke, for a ticket body: the log is cut at
/// the first `##[error]` line (or where the post-job steps start), the lines with failure
/// markers and two lines of context around each are kept (the last `max_lines` of them), and
/// with no marker the last `max_lines` lines before the cut. Timestamps and colour codes are
/// removed, and the step's `Run <cmd>` header is named first when the log has one.
pub fn error_excerpt(log: &str, max_lines: usize) -> String {
    let lines: Vec<String> = log.lines().map(clean_line).collect();
    let is_post = |l: &str| {
        let t = l.trim_start_matches("##[group]");
        t.starts_with("Post job cleanup.") || t.starts_with("Post Run ")
    };
    let end = if let Some(i) = lines.iter().position(|l| l.contains("##[error]")) {
        i + 1
    } else {
        lines.iter().position(|l| is_post(l)).unwrap_or(lines.len())
    };
    let region = &lines[..end];
    let step = region
        .iter()
        .rev()
        .find_map(|l| l.strip_prefix("##[group]Run "))
        .map(|c| format!("Step: Run {}", c.trim()));
    let mut keep = vec![false; region.len()];
    let mut any = false;
    for (i, l) in region.iter().enumerate() {
        if is_failure_line(l) {
            any = true;
            let hi = (i + 3).min(region.len());
            for flag in &mut keep[i.saturating_sub(2)..hi] {
                *flag = true;
            }
        }
    }
    let mut picked: Vec<&str> = if any {
        region
            .iter()
            .zip(&keep)
            .filter(|(l, k)| **k && !l.starts_with("##[group]") && !l.starts_with("##[endgroup]"))
            .map(|(l, _)| l.as_str())
            .collect()
    } else {
        region.iter().map(String::as_str).collect()
    };
    picked.drain(..picked.len().saturating_sub(max_lines));
    let mut out = String::new();
    if let Some(s) = step {
        out.push_str(&s);
        out.push('\n');
    }
    out.push_str(&picked.join("\n"));
    out
}

pub fn step(state: &mut State, runs: &[RunRec], ctx: &Ctx) -> Vec<File> {
    let rules = ctx.rules;
    let mut out: Vec<File> = vec![];
    let mut open_auto = ctx.open_auto;
    let mut open_approved = ctx.open_approved;
    let mut decided: BTreeSet<String> = BTreeSet::new();

    let mut file = |state: &mut State, out: &mut Vec<File>, mut f: File| {
        let k = f.key();
        let js = state
            .workflows
            .get_mut(&f.workflow)
            .and_then(|w| w.jobs.get_mut(&f.job));
        let Some(js) = js else { return };
        if f.why == Why::Timeout {
            // Its own condition: it leaves the red job's ticket and coverage alone.
            js.timeout_filed = true;
            if ctx.named.contains_key(&k) || !decided.insert(format!("{k}#timeout")) {
                return;
            }
            if open_auto >= rules.max_open_auto {
                f.proposed = true;
            } else {
                open_auto += 1;
            }
            out.push(f);
            return;
        }
        js.checked = Some(ctx.now_ms);
        if f.urgent && rules.freeze_on_gate {
            js.freeze = true;
        }
        let group = f.group();
        f.signatures = js.signatures.clone();
        // Red on `agreement` finished runs that ran the job, with no green or skip between.
        let confirmed = js.pending.len() as u32 >= rules.agreement.max(2);
        if let Some(t) = ctx.named.get(&k).or_else(|| ctx.named.get(&group)) {
            // An open task or issue already names it: no second ticket.
            if js.ticket.is_none() {
                js.ticket = *t;
            }
            js.covered = js.covered.max(f.at.run);
            return;
        }
        if let Some(t) = f.signatures.iter().find_map(|s| ctx.named_sigs.get(s)) {
            // An open task or issue names one of the scripts or tests that failed.
            if t.is_some() {
                js.ticket = *t;
            }
            js.covered = js.covered.max(f.at.run);
            return;
        }
        // An over task covered this run (its body cites it and names the job or its shard
        // group), or, for a long red whose history is unknown, its title names the job:
        // record it, file nothing.
        if let Some(t) = ctx.recent.iter().find(|t| {
            (t.cites(f.at.run)
                && (names_job(&t.title, &f.workflow, &f.job)
                    || names_job(&t.body, &f.workflow, &f.job)))
                || (f.why == Why::LongRed && js.covered == 0 && t.title.contains(&k))
        }) {
            js.ticket = Some(t.id);
            js.covered = js.covered.max(f.at.run);
            js.landed_ms = ctx.landed.get(&t.id).copied();
            return;
        }
        if !f.signatures.is_empty() && !decided.contains(&group) {
            // The same failure on another system (another group of the same workflow, filed
            // in this pass): one ticket naming both.
            if let Some(g) = out.iter_mut().find(|g| {
                g.why != Why::Timeout
                    && g.workflow == f.workflow
                    && g.group() != group
                    && g.signatures.iter().any(|s| f.signatures.contains(s))
            }) {
                decided.insert(group.clone());
                g.also.push(f.job.clone());
                g.signatures.extend(f.signatures.iter().cloned());
                g.signatures.sort();
                g.signatures.dedup();
                return;
            }
        }
        if !decided.insert(group.clone()) {
            // Another shard of the same job was filed in this pass: one ticket for both.
            if let Some(g) = out
                .iter_mut()
                .find(|g| g.why != Why::Timeout && g.workflow == f.workflow && g.group() == group)
            {
                g.also.push(f.job.clone());
                g.signatures.extend(f.signatures.iter().cloned());
                g.signatures.sort();
                g.signatures.dedup();
            }
            return;
        }
        f.repeats = ctx
            .recent
            .iter()
            .filter_map(|t| {
                let at = *ctx.landed.get(&t.id)?;
                let sigs: Vec<String> = f
                    .signatures
                    .iter()
                    .filter(|s| mentions(&t.title, s) || mentions(&t.body, s))
                    .cloned()
                    .collect();
                (at <= f.at.at_ms && !sigs.is_empty()).then(|| Repeat {
                    task: t.id,
                    title: t.title.clone(),
                    landed_ms: at,
                    signatures: sigs,
                })
            })
            .collect();
        if rules.auto_approve && f.why != Why::Gate {
            if confirmed && open_approved < rules.auto_approve_max {
                open_approved += 1;
                f.auto = true;
            } else {
                f.proposed = true;
            }
        } else if open_auto >= rules.max_open_auto {
            f.proposed = true;
        } else {
            open_auto += 1;
        }
        out.push(f);
    };

    for (w, rs) in by_workflow(rules, runs) {
        let excluded = rules.excluded(&w);
        let seeded = state.workflows.contains_key(&w);
        let todo: Vec<&RunRec> = if seeded {
            let last = state.workflows[&w].last_run;
            rs.into_iter()
                .filter(|r| r.id > last && !r.jobs.is_empty())
                .collect()
        } else {
            rs.into_iter()
                .rev()
                .take(1)
                .filter(|r| !r.jobs.is_empty())
                .collect()
        };
        if todo.is_empty() {
            continue;
        }
        if !seeded {
            let r = todo[0];
            let ws = state.workflows.entry(w.clone()).or_default();
            ws.last_run = r.id;
            for j in &r.jobs {
                match j.conclusion.as_deref() {
                    Some("success") => {
                        ws.jobs.insert(
                            j.name.clone(),
                            JobState {
                                full: r.full,
                                ..JobState::default()
                            },
                        );
                    }
                    Some("failure") => {
                        ws.jobs.insert(
                            j.name.clone(),
                            JobState {
                                red: true,
                                known: true,
                                full: r.full,
                                red_since: ctx.now_ms,
                                last_red: job_run_ref(r, j),
                                signatures: j.signatures.clone(),
                                ..JobState::default()
                            },
                        );
                    }
                    _ => {}
                }
            }
            continue;
        }
        for r in todo {
            if let Some(ws) = state.workflows.get_mut(&w) {
                ws.last_run = r.id;
            }
            for j in &r.jobs {
                let gate = rules.is_gate(&w, &j.name);
                let ws = state.workflows.entry(w.clone()).or_default();
                ws.jobs.entry(j.name.clone()).or_default().full = r.full;
                match j.conclusion.as_deref() {
                    // A job that now skips (a workflow change, an `if:`) is no longer red.
                    Some("success" | "skipped" | "neutral") => {
                        let js = ws.jobs.entry(j.name.clone()).or_default();
                        *js = JobState {
                            full: r.full,
                            ..JobState::default()
                        };
                    }
                    Some("timed_out") => {
                        let js = ws.jobs.entry(j.name.clone()).or_default();
                        js.last_timeout = job_run_ref(r, j);
                        if !js.timeouts.contains(&r.id) {
                            js.timeouts.push(r.id);
                        }
                        let n = js.timeouts.len() as u32;
                        if !excluded && !js.timeout_filed && n >= rules.agreement.max(1) {
                            let at = js.last_timeout.clone();
                            file(
                                state,
                                &mut out,
                                File {
                                    workflow: w.clone(),
                                    job: j.name.clone(),
                                    urgent: false,
                                    proposed: false,
                                    auto: false,
                                    why: Why::Timeout,
                                    at,
                                    signatures: vec![],
                                    also: vec![],
                                    repeats: vec![],
                                },
                            );
                        }
                    }
                    Some("failure") => {
                        let js = ws.jobs.entry(j.name.clone()).or_default();
                        js.timeouts.clear();
                        js.timeout_filed = false;
                        let at = job_run_ref(r, j);
                        js.last_red = at.clone();
                        js.signatures = j.signatures.clone();
                        if !js.red {
                            js.red = true;
                            js.known = false;
                            js.red_since = r.created_ms.min(ctx.now_ms).max(1);
                            js.pending = vec![r.id];
                            if gate && !excluded {
                                file(
                                    state,
                                    &mut out,
                                    File {
                                        workflow: w.clone(),
                                        job: j.name.clone(),
                                        urgent: true,
                                        proposed: false,
                                        auto: false,
                                        why: Why::Gate,
                                        at,
                                        signatures: vec![],
                                        also: vec![],
                                        repeats: vec![],
                                    },
                                );
                                continue;
                            }
                        } else if !js.known && !js.pending.contains(&r.id) {
                            js.pending.push(r.id);
                        }
                        let js = &state.workflows[&w].jobs[&j.name];
                        let agreed = !excluded
                            && !gate
                            && !js.known
                            && js.ticket.is_none()
                            && js.checked.is_none()
                            && js.pending.len() as u32 >= rules.agreement.max(1);
                        let at = js.last_red.clone();
                        if agreed {
                            file(
                                state,
                                &mut out,
                                File {
                                    workflow: w.clone(),
                                    job: j.name.clone(),
                                    urgent: false,
                                    proposed: false,
                                    auto: false,
                                    why: Why::Agreed,
                                    at,
                                    signatures: vec![],
                                    also: vec![],
                                    repeats: vec![],
                                },
                            );
                        }
                    }
                    // cancelled, startup_failure, action_required, stale: nothing about
                    // the code.
                    _ => {}
                }
            }
            // A job that a finished run does not have is absent; one that has it is not.
            // After `agreement + 2` runs in a row without it, it was removed or renamed:
            // its state, banner and freeze go.
            if let Some(ws) = state.workflows.get_mut(&w) {
                let limit = rules.agreement + 2;
                for (name, js) in ws.jobs.iter_mut() {
                    if r.jobs.iter().any(|j| &j.name == name) {
                        js.absent = 0;
                    } else if js.full == r.full {
                        js.absent += 1;
                    }
                }
                ws.jobs.retain(|_, js| js.absent < limit);
            }
        }
    }

    // A ticket that landed with a change: remember when.
    for ws in state.workflows.values_mut() {
        for js in ws.jobs.values_mut() {
            if let (Some(t), None) = (js.ticket, js.landed_ms) {
                js.landed_ms = ctx.landed.get(&t).copied();
            }
        }
    }

    // Long reds.
    let mut due: Vec<(String, String, bool, RunRef)> = vec![];
    for (w, ws) in &state.workflows {
        if rules.excluded(w) {
            continue;
        }
        for (name, js) in &ws.jobs {
            let from = js.checked.unwrap_or(js.red_since);
            if js.red && ctx.now_ms.saturating_sub(from) >= rules.red_ticket_after_ms && !held(js) {
                due.push((
                    w.clone(),
                    name.clone(),
                    rules.is_gate(w, name),
                    js.last_red.clone(),
                ));
            }
        }
    }
    for (workflow, job, gate, at) in due {
        file(
            state,
            &mut out,
            File {
                workflow,
                job,
                urgent: gate,
                proposed: false,
                auto: false,
                why: Why::LongRed,
                at,
                signatures: vec![],
                also: vec![],
                repeats: vec![],
            },
        );
    }

    // A job that went green clears its freeze with the rest of its state (above), and a
    // job that is red and checked is not looked at again until another threshold.
    out
}

/// A red job whose latest red run is already covered by a ticket: no new evidence, so the
/// long-red rule does not file it again.
fn held(js: &JobState) -> bool {
    js.covered != 0
        && (js.last_red.run <= js.covered || js.landed_ms.is_some_and(|l| js.last_red.at_ms < l))
}

/// Red jobs the long-red rule holds back: `(key, run, ticket)`.
pub fn held_back(state: &State, rules: &Rules) -> Vec<(String, u64, Option<u64>)> {
    let mut v = vec![];
    for (w, ws) in &state.workflows {
        if rules.excluded(w) {
            continue;
        }
        for (j, js) in &ws.jobs {
            if js.red && js.ticket.is_some() && held(js) {
                let item = (group_key(w, j), js.last_red.run, js.ticket);
                if !v.contains(&item) {
                    v.push(item);
                }
            }
        }
    }
    v
}

/// The shard number of a job name `name (os, N)`.
fn shard_of(job: &str) -> Option<&str> {
    let inner = job.strip_suffix(')')?;
    let (_, n) = inner.rsplit_once(", ")?;
    (!n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())).then_some(n)
}

/// One line of the held-back view: the shards of a job (or the job) under one ticket.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeldLine {
    /// The group key (`CI/exit-scripts (ubuntu-latest, *)` for shards).
    pub key: String,
    pub run: u64,
    pub ticket: u64,
    pub shards: Vec<String>,
    /// The ticket landed a change.
    pub landed: bool,
    /// The job runs only in the full run: it is confirmed by the next one.
    pub full: bool,
}

impl HeldLine {
    /// `CI/exit-scripts (ubuntu-latest): shards 1, 4` or the plain job key.
    pub fn name(&self) -> String {
        match (self.key.strip_suffix(", *)"), self.shards.is_empty()) {
            (Some(head), false) => format!(
                "{head}): shard{} {}",
                if self.shards.len() == 1 { "" } else { "s" },
                self.shards.join(", ")
            ),
            _ => self.key.clone(),
        }
    }
    /// The banner for a job whose open ticket is fixing it.
    pub fn open_text(&self) -> String {
        format!("{} red; #{} is fixing it", self.name(), self.ticket)
    }
}

/// Held-back red jobs, by shard group and ticket. Jobs with a ticket that has no landing
/// recorded are `open` (being fixed); with one, landed and not yet confirmed by a finished run.
pub fn held_lines(state: &State, rules: &Rules) -> Vec<HeldLine> {
    let mut v: Vec<HeldLine> = vec![];
    for (w, ws) in &state.workflows {
        if rules.excluded(w) {
            continue;
        }
        for (j, js) in &ws.jobs {
            let (true, Some(t), true) = (js.red, js.ticket, held(js)) else {
                continue;
            };
            let key = group_key(w, j);
            let landed = js.landed_ms.is_some();
            let i = match v
                .iter()
                .position(|l| l.key == key && l.ticket == t && l.landed == landed)
            {
                Some(i) => i,
                None => {
                    v.push(HeldLine {
                        key,
                        run: 0,
                        ticket: t,
                        shards: vec![],
                        landed,
                        full: js.full,
                    });
                    v.len() - 1
                }
            };
            v[i].run = v[i].run.max(js.last_red.run);
            if let Some(n) = shard_of(j) {
                v[i].shards.push(n.to_string());
            }
        }
    }
    v
}

/// The quiet line for jobs whose fix landed and that no finished run has confirmed yet.
pub fn unconfirmed_text(lines: &[HeldLine]) -> Option<String> {
    let l: Vec<&HeldLine> = lines.iter().filter(|l| l.landed).collect();
    if l.is_empty() {
        return None;
    }
    let items: Vec<String> = l
        .iter()
        .map(|l| {
            let wait = if l.full { ", next full run" } else { "" };
            format!("{} (#{}{wait})", l.name(), l.ticket)
        })
        .collect();
    let n = items.len();
    Some(format!(
        "{n} CI job{}: fix landed, not yet confirmed by a finished run: {}",
        if n == 1 { "" } else { "s" },
        items.join("; ")
    ))
}

/// The text the status shows for a held-back job.
pub fn held_text(key: &str, run: u64, ticket: Option<u64>) -> String {
    match ticket {
        Some(t) => {
            format!("{key} still red on run {run}; #{t} covered it; waiting for a newer run")
        }
        None => format!("{key} still red on run {run}; waiting for a newer run"),
    }
}

/// Like `held_text`, for a job whose newer runs `timed_out` of them did not finish it:
/// "CI/x still red on run N; #T landed 3h ago; the 5 runs since timed out before finishing it".
/// With no such runs it is `held_text`.
pub fn held_text_timeouts(
    key: &str,
    run: u64,
    ticket: Option<u64>,
    landed_ms: Option<u64>,
    timed_out_since: usize,
    now_ms: u64,
) -> String {
    if timed_out_since == 0 {
        return held_text(key, run, ticket);
    }
    let runs = if timed_out_since == 1 {
        "the 1 run since timed out".to_string()
    } else {
        format!("the {timed_out_since} runs since timed out")
    };
    let t = match (ticket, landed_ms) {
        (Some(t), Some(l)) => format!("#{t} landed {}; ", ago(now_ms.saturating_sub(l))),
        (Some(t), None) => format!("#{t} covered it; "),
        _ => String::new(),
    };
    format!("{key} still red on run {run}; {t}{runs} before finishing it")
}

fn ago(ms: u64) -> String {
    let m = ms / 60_000;
    match m {
        0..=59 => format!("{m}m ago"),
        60..=2879 => format!("{}h ago", m / 60),
        _ => format!("{}d ago", m / 1440),
    }
}

/// For a held-back job: when its ticket landed, and how many runs newer than its last red
/// timed out. `(key, landed_ms, timed_out_since)` for every red job, held or not.
pub fn timeout_notes(state: &State) -> BTreeMap<String, (Option<u64>, usize)> {
    let mut m = BTreeMap::new();
    for (w, ws) in &state.workflows {
        for (j, js) in &ws.jobs {
            let n = js.timeouts.iter().filter(|r| **r > js.last_red.run).count();
            m.insert(key(w, j), (js.landed_ms, n));
        }
    }
    m
}

/// Jobs that hit their time limit since their last result: `(key, runs, filed)`.
pub fn timeouts(state: &State, rules: &Rules) -> Vec<(String, usize, bool)> {
    let mut v = vec![];
    for (w, ws) in &state.workflows {
        if rules.excluded(w) {
            continue;
        }
        for (j, js) in &ws.jobs {
            if !js.timeouts.is_empty() {
                v.push((key(w, j), js.timeouts.len(), js.timeout_filed));
            }
        }
    }
    v
}

/// The jobs whose red holds the freeze: `workflow/job` and the task that fixes it.
pub fn freezing(state: &State) -> Vec<(String, Option<u64>)> {
    let mut v = vec![];
    for (w, ws) in &state.workflows {
        for (j, js) in &ws.jobs {
            if js.red && js.freeze {
                v.push((key(w, j), js.ticket));
            }
        }
    }
    v
}

/// Jobs red and waiting for agreement: `(key, red runs so far)`.
pub fn pending(state: &State, rules: &Rules) -> Vec<(String, usize)> {
    let mut v = vec![];
    for (w, ws) in &state.workflows {
        if rules.excluded(w) {
            continue;
        }
        for (j, js) in &ws.jobs {
            if js.red && !js.known && js.ticket.is_none() && js.checked.is_none() {
                v.push((key(w, j), js.pending.len()));
            }
        }
    }
    v
}

/// Record that `key` was filed as `task`.
pub fn filed(state: &mut State, f: &File, task: Option<u64>, now_ms: u64, url: &str) {
    if f.why != Why::Timeout {
        let ws = state.workflows.get_mut(&f.workflow);
        if let Some(ws) = ws {
            for name in std::iter::once(&f.job).chain(&f.also) {
                if let Some(js) = ws.jobs.get_mut(name) {
                    js.ticket = task;
                    js.covered = js.covered.max(if *name == f.job {
                        f.at.run
                    } else {
                        js.last_red.run
                    });
                    js.landed_ms = None;
                }
            }
        }
    }
    state.filed.push(Filed {
        key: f.group(),
        task,
        at: now_ms,
        urgent: f.urgent,
        proposed: f.proposed,
        auto: f.auto,
        url: url.to_string(),
    });
    let n = state.filed.len();
    if n > 20 {
        state.filed.drain(..n - 20);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> Rules {
        Rules {
            workflows: vec![],
            exclude: vec!["Gherkin".into()],
            gate: Some("CI/pr-gate".into()),
            agreement: 2,
            red_ticket_after_ms: 3 * 3_600_000,
            max_open_auto: 2,
            freeze_on_gate: true,
            auto_approve: false,
            auto_approve_max: 5,
        }
    }

    fn run(id: u64, wf: &str, concl: &str, jobs: &[(&str, &str)]) -> RunRec {
        RunRec {
            id,
            workflow: wf.into(),
            sha: format!("sha{id}"),
            url: format!("https://x/runs/{id}"),
            created_ms: id * 1000,
            conclusion: Some(concl.into()),
            full: false,
            jobs: jobs
                .iter()
                .enumerate()
                .map(|(i, (n, c))| JobRec {
                    id: id * 10 + i as u64,
                    name: (*n).into(),
                    conclusion: Some((*c).into()),
                    signatures: vec![],
                })
                .collect(),
        }
    }

    fn go(
        s: &mut State,
        runs: &[RunRec],
        now: u64,
        named: &BTreeMap<String, Option<u64>>,
        open: u32,
    ) -> Vec<File> {
        let r = rules();
        step(
            s,
            runs,
            &Ctx {
                rules: &r,
                now_ms: now,
                named,
                open_auto: open,
                open_approved: 0,
                recent: &[],
                landed: &BTreeMap::new(),
                named_sigs: &BTreeMap::new(),
            },
        )
    }

    fn none() -> BTreeMap<String, Option<u64>> {
        BTreeMap::new()
    }

    #[test]
    fn seeding_files_nothing_even_for_a_red() {
        let mut s = State::default();
        let f = go(
            &mut s,
            &[run(
                1,
                "CI",
                "failure",
                &[("pr-gate", "failure"), ("a", "success")],
            )],
            5000,
            &none(),
            0,
        );
        assert!(f.is_empty());
        assert!(s.workflows["CI"].jobs["pr-gate"].known);
        assert!(freezing(&s).is_empty());
    }

    #[test]
    fn a_cancelled_newest_run_does_not_seed() {
        let mut s = State::default();
        let runs = [
            run(1, "CI", "success", &[("a", "success")]),
            run(2, "CI", "cancelled", &[("a", "cancelled")]),
        ];
        assert_eq!(needs_jobs(&s, &rules(), &runs), vec![1]);
        go(&mut s, &runs, 5000, &none(), 0);
        assert!(!s.workflows["CI"].jobs["a"].red);
        assert_eq!(s.workflows["CI"].last_run, 1);
    }

    #[test]
    fn a_gate_red_files_at_once_and_freezes_until_green() {
        let mut s = State::default();
        go(
            &mut s,
            &[run(1, "CI", "success", &[("pr-gate", "success")])],
            5000,
            &none(),
            0,
        );
        let f = go(
            &mut s,
            &[run(2, "CI", "failure", &[("pr-gate", "failure")])],
            6000,
            &none(),
            0,
        );
        assert_eq!(f.len(), 1);
        assert!(f[0].urgent && !f[0].proposed);
        assert_eq!(f[0].why, Why::Gate);
        assert_eq!(f[0].at.sha, "sha2");
        filed(&mut s, &f[0], Some(7), 6000, "u");
        assert_eq!(freezing(&s), vec![("CI/pr-gate".to_string(), Some(7))]);
        // Still red on the next run: no second filing.
        let f = go(
            &mut s,
            &[run(3, "CI", "failure", &[("pr-gate", "failure")])],
            7000,
            &none(),
            1,
        );
        assert!(f.is_empty());
        assert_eq!(freezing(&s).len(), 1);
        go(
            &mut s,
            &[run(4, "CI", "success", &[("pr-gate", "success")])],
            8000,
            &none(),
            1,
        );
        assert!(freezing(&s).is_empty());
    }

    #[test]
    fn a_gate_red_with_freeze_off_files_and_does_not_freeze() {
        let mut s = State::default();
        let mut r = rules();
        r.freeze_on_gate = false;
        let named = none();
        let go2 = |s: &mut State, runs: &[RunRec]| {
            step(
                s,
                runs,
                &Ctx {
                    rules: &r,
                    now_ms: 9000,
                    named: &named,
                    open_auto: 0,
                    open_approved: 0,
                    recent: &[],
                    landed: &BTreeMap::new(),
                    named_sigs: &BTreeMap::new(),
                },
            )
        };
        go2(
            &mut s,
            &[run(1, "CI", "success", &[("pr-gate", "success")])],
        );
        let f = go2(
            &mut s,
            &[run(2, "CI", "failure", &[("pr-gate", "failure")])],
        );
        assert_eq!(f.len(), 1);
        assert!(freezing(&s).is_empty());
    }

    #[test]
    fn a_job_red_once_then_green_files_nothing() {
        let mut s = State::default();
        go(
            &mut s,
            &[run(1, "CI", "success", &[("a", "success")])],
            5000,
            &none(),
            0,
        );
        let f = go(
            &mut s,
            &[run(2, "CI", "failure", &[("a", "failure")])],
            6000,
            &none(),
            0,
        );
        assert!(f.is_empty());
        assert_eq!(pending(&s, &rules()), vec![("CI/a".to_string(), 1)]);
        let f = go(
            &mut s,
            &[run(3, "CI", "success", &[("a", "success")])],
            7000,
            &none(),
            0,
        );
        assert!(f.is_empty());
        assert!(pending(&s, &rules()).is_empty());
    }

    #[test]
    fn red_on_two_different_runs_files_one_task() {
        let mut s = State::default();
        go(
            &mut s,
            &[run(1, "CI", "success", &[("a", "success")])],
            5000,
            &none(),
            0,
        );
        go(
            &mut s,
            &[run(2, "CI", "failure", &[("a", "failure")])],
            6000,
            &none(),
            0,
        );
        let f = go(
            &mut s,
            &[run(3, "CI", "failure", &[("a", "failure")])],
            7000,
            &none(),
            0,
        );
        assert_eq!(f.len(), 1);
        assert!(!f[0].urgent);
        assert_eq!(f[0].why, Why::Agreed);
        filed(&mut s, &f[0], Some(9), 7000, "u");
        let f = go(
            &mut s,
            &[run(4, "CI", "failure", &[("a", "failure")])],
            8000,
            &none(),
            1,
        );
        assert!(f.is_empty());
    }

    #[test]
    fn red_runs_of_one_job_on_different_commits_share_the_workflow_key() {
        // The workflow's name, never a run's display title, is what the adapter hands over (s928).
        let mut s = State::default();
        go(
            &mut s,
            &[run(1, "CI", "success", &[("windows", "success")])],
            5000,
            &none(),
            0,
        );
        go(
            &mut s,
            &[run(2, "CI", "failure", &[("windows", "failure")])],
            6000,
            &none(),
            0,
        );
        let f = go(
            &mut s,
            &[run(3, "CI", "failure", &[("windows", "failure")])],
            7000,
            &none(),
            0,
        );
        assert_eq!(f.len(), 1);
        assert_eq!(key(&f[0].workflow, &f[0].job), "CI/windows");
        assert_eq!(s.workflows.keys().collect::<Vec<_>>(), vec!["CI"]);
    }

    #[test]
    fn the_same_run_seen_twice_is_not_agreement() {
        let mut s = State::default();
        go(
            &mut s,
            &[run(1, "CI", "success", &[("a", "success")])],
            5000,
            &none(),
            0,
        );
        let r2 = run(2, "CI", "failure", &[("a", "failure")]);
        go(&mut s, std::slice::from_ref(&r2), 6000, &none(), 0);
        let f = go(&mut s, &[r2], 7000, &none(), 0);
        assert!(f.is_empty());
    }

    fn red_then_timeouts(s: &mut State, n: u64) -> Vec<File> {
        go(
            s,
            &[run(1, "CI", "success", &[("a", "success")])],
            0,
            &none(),
            0,
        );
        go(
            s,
            &[run(2, "CI", "failure", &[("a", "failure")])],
            1000,
            &none(),
            0,
        );
        let mut all = vec![];
        for id in 3..3 + n {
            all.extend(go(
                s,
                &[run(id, "CI", "failure", &[("a", "timed_out")])],
                2000,
                &none(),
                0,
            ));
        }
        all
    }

    #[test]
    fn timeouts_agree_into_one_ticket_and_no_more() {
        let mut s = State::default();
        let f = red_then_timeouts(&mut s, 4);
        assert_eq!(f.len(), 1, "{f:?}");
        assert_eq!(f[0].why, Why::Timeout);
        assert_eq!(f[0].at.run, 4);
        assert_eq!(timeouts(&s, &rules()), vec![("CI/a".to_string(), 4, true)]);
        // The red job's own ticket state is untouched.
        assert!(s.workflows["CI"].jobs["a"].ticket.is_none());
    }

    #[test]
    fn one_timeout_is_not_agreement() {
        let mut s = State::default();
        assert!(red_then_timeouts(&mut s, 1).is_empty());
        assert_eq!(timeouts(&s, &rules()), vec![("CI/a".to_string(), 1, false)]);
    }

    #[test]
    fn a_superseded_cancelled_run_changes_nothing() {
        let mut s = State::default();
        go(
            &mut s,
            &[run(1, "CI", "success", &[("a", "success")])],
            0,
            &none(),
            0,
        );
        let f = go(
            &mut s,
            &[run(2, "CI", "cancelled", &[("a", "cancelled")])],
            1000,
            &none(),
            0,
        );
        assert!(f.is_empty());
        assert!(timeouts(&s, &rules()).is_empty());
    }

    #[test]
    fn a_result_clears_the_timeouts() {
        let mut s = State::default();
        assert_eq!(red_then_timeouts(&mut s, 2).len(), 1);
        go(
            &mut s,
            &[run(9, "CI", "success", &[("a", "success")])],
            3000,
            &none(),
            0,
        );
        assert!(timeouts(&s, &rules()).is_empty());
        assert!(!s.workflows["CI"].jobs["a"].red);
        // A failure also ends the count, and a new run of timeouts can file again.
        let f = red_then_timeouts_from(&mut s, 10);
        assert_eq!(f.len(), 1);
    }

    fn red_then_timeouts_from(s: &mut State, from: u64) -> Vec<File> {
        let mut all = vec![];
        for id in from..from + 2 {
            all.extend(go(
                s,
                &[run(id, "CI", "failure", &[("a", "timed_out")])],
                4000,
                &none(),
                0,
            ));
        }
        all
    }

    #[test]
    fn held_text_names_the_runs_that_timed_out_and_the_landing() {
        let mut s = State::default();
        red_then_timeouts(&mut s, 5);
        let notes = timeout_notes(&s);
        let (_, n) = notes["CI/a"];
        assert_eq!(n, 5);
        let h = 3_600_000;
        let t = held_text_timeouts("CI/a", 2, Some(7), Some(h), n, 4 * h);
        assert_eq!(
            t,
            "CI/a still red on run 2; #7 landed 3h ago; the 5 runs since timed out before finishing it"
        );
        assert_eq!(
            held_text_timeouts("CI/a", 2, Some(7), None, 0, 0),
            held_text("CI/a", 2, Some(7))
        );
    }

    #[test]
    fn infrastructure_conclusions_file_nothing() {
        let mut s = State::default();
        go(
            &mut s,
            &[run(
                1,
                "CI",
                "success",
                &[("pr-gate", "success"), ("a", "success")],
            )],
            5000,
            &none(),
            0,
        );
        for (i, c) in ["cancelled", "startup_failure"].iter().enumerate() {
            let id = 2 + i as u64;
            // A failed run whose jobs ended in infrastructure.
            let f = go(
                &mut s,
                &[run(id, "CI", "failure", &[("pr-gate", c), ("a", c)])],
                6000,
                &none(),
                0,
            );
            assert!(f.is_empty(), "{c}");
        }
        assert!(!s.workflows["CI"].jobs["pr-gate"].red);
        // A skipped job inside a finished run changes nothing either.
        go(
            &mut s,
            &[run(9, "CI", "success", &[("pr-gate", "skipped")])],
            6000,
            &none(),
            0,
        );
        assert!(!s.workflows["CI"].jobs["pr-gate"].red);
    }

    fn fixture(n: &str) -> String {
        match n {
            "lint_spdx.log" => include_str!("../tests/fixtures/ci_logs/lint_spdx.log"),
            "monitor_compose.log" => include_str!("../tests/fixtures/ci_logs/monitor_compose.log"),
            "cargo_test.log" => include_str!("../tests/fixtures/ci_logs/cargo_test.log"),
            "rustfmt_diff.log" => include_str!("../tests/fixtures/ci_logs/rustfmt_diff.log"),
            "no_marker.log" => include_str!("../tests/fixtures/ci_logs/no_marker.log"),
            other => panic!("unknown ci log fixture {other}"),
        }
        .to_string()
    }

    #[test]
    fn excerpt_of_the_lint_log_has_the_spdx_line_and_no_post_job_noise() {
        let e = error_excerpt(&fixture("lint_spdx.log"), 60);
        assert!(
            e.contains("missing SPDX-License-Identifier: GPL-3.0-or-later in scripts/exit/s653.sh")
        );
        assert!(e.contains("Step: Run bash scripts/check-spdx.sh"), "{e}");
        assert!(!e.contains("Post job cleanup"), "{e}");
        assert!(!e.contains('\u{1b}') && !e.contains("2026-11-20T"), "{e}");
    }

    #[test]
    fn excerpt_of_monitor_compose_has_the_conformance_lines() {
        let e = error_excerpt(&fixture("monitor_compose.log"), 60);
        assert!(e.contains("conformance failed: 21 difference(s)"), "{e}");
        assert!(e.contains("checkpoint-"), "{e}");
        assert!(!e.contains("Cache cleanup"), "{e}");
    }

    #[test]
    fn excerpt_of_a_cargo_failure_names_the_test() {
        let e = error_excerpt(&fixture("cargo_test.log"), 60);
        assert!(e.contains("panicked at"), "{e}");
        assert!(e.contains("ci_watch::tests::red_is_filed"), "{e}");
    }

    #[test]
    fn excerpt_of_a_rustfmt_failure_has_the_diff() {
        let e = error_excerpt(&fixture("rustfmt_diff.log"), 60);
        assert!(
            e.contains("Diff in /home/runner/work/mason/crates/core/src/lib.rs:12:"),
            "{e}"
        );
    }

    #[test]
    fn excerpt_with_no_marker_is_the_lines_before_the_post_job_section() {
        let e = error_excerpt(&fixture("no_marker.log"), 3);
        assert!(
            e.ends_with("step three done\nthe thing stopped talking"),
            "{e}"
        );
        assert!(!e.contains("Post job cleanup"), "{e}");
        assert!(!e.contains("step one done"), "{e}");
    }

    #[test]
    fn a_red_job_whose_next_run_skips_or_is_neutral_is_no_longer_red() {
        for c in ["skipped", "neutral"] {
            let mut s = State::default();
            go(
                &mut s,
                &[run(1, "CI", "failure", &[("a", "failure")])],
                5000,
                &none(),
                0,
            );
            assert!(s.workflows["CI"].jobs["a"].red);
            go(
                &mut s,
                &[run(2, "CI", "success", &[("a", c)])],
                6000,
                &none(),
                0,
            );
            assert!(!s.workflows["CI"].jobs["a"].red, "{c}");
        }
    }

    #[test]
    fn a_cancelled_job_leaves_a_red_job_red() {
        let mut s = State::default();
        go(
            &mut s,
            &[run(1, "CI", "failure", &[("a", "failure")])],
            5000,
            &none(),
            0,
        );
        go(
            &mut s,
            &[run(2, "CI", "failure", &[("a", "cancelled")])],
            6000,
            &none(),
            0,
        );
        assert!(s.workflows["CI"].jobs["a"].red);
    }

    #[test]
    fn a_long_red_is_filed_once_per_threshold_on_an_injected_clock() {
        let mut s = State::default();
        let h = 3_600_000;
        go(
            &mut s,
            &[run(1, "CI", "success", &[("a", "success")])],
            0,
            &none(),
            0,
        );
        go(
            &mut s,
            &[run(2, "CI", "failure", &[("a", "failure")])],
            1000,
            &none(),
            0,
        );
        // One red run, not agreed; not yet long.
        assert!(go(&mut s, &[], 2 * h, &none(), 0).is_empty());
        let f = go(&mut s, &[], 3 * h + 2000, &none(), 0);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].why, Why::LongRed);
        filed(&mut s, &f[0], Some(5), 3 * h + 2000, "u");
        // Still red on the same run: never again, however long.
        assert!(go(&mut s, &[], 4 * h, &none(), 0).is_empty());
        assert!(go(&mut s, &[], 60 * h, &none(), 0).is_empty());
        // A newer red run is new evidence.
        go(
            &mut s,
            &[run(3, "CI", "failure", &[("a", "failure")])],
            61 * h,
            &none(),
            0,
        );
        assert_eq!(go(&mut s, &[], 70 * h, &none(), 0).len(), 1);
    }

    const H: u64 = 3_600_000;

    fn go_r(
        s: &mut State,
        runs: &[RunRec],
        now: u64,
        recent: &[RecentTask],
        landed: &BTreeMap<u64, u64>,
    ) -> Vec<File> {
        let r = rules();
        step(
            s,
            runs,
            &Ctx {
                rules: &r,
                now_ms: now,
                named: &none(),
                open_auto: 0,
                open_approved: 0,
                recent,
                landed,
                named_sigs: &none(),
            },
        )
    }

    fn red_filed(s: &mut State, task: u64) {
        go(
            s,
            &[run(1, "CI", "success", &[("a", "success")])],
            0,
            &none(),
            0,
        );
        go(
            s,
            &[run(2, "CI", "failure", &[("a", "failure")])],
            1000,
            &none(),
            0,
        );
        let f = go_r(s, &[], 4 * H, &[], &BTreeMap::new());
        assert_eq!(f.len(), 1);
        filed(s, &f[0], Some(task), 4 * H, "u");
    }

    #[test]
    fn a_landed_ticket_with_no_newer_run_files_nothing() {
        let mut s = State::default();
        red_filed(&mut s, 5);
        let landed: BTreeMap<u64, u64> = [(5, 5 * H)].into();
        let recent = [RecentTask {
            id: 5,
            title: "CI/a red on develop".into(),
            body: "Run: https://x/runs/2".into(),
        }];
        for i in 1..=10 {
            assert!(go_r(&mut s, &[], 5 * H + i * 4 * H, &recent, &landed).is_empty());
        }
        assert_eq!(
            held_back(&s, &rules()),
            vec![("CI/a".to_string(), 2, Some(5))]
        );
        // A newer red run, of a commit after the landing: filed once.
        let mut r3 = run(3, "CI", "failure", &[("a", "failure")]);
        r3.created_ms = 6 * H;
        go_r(&mut s, &[r3], 50 * H, &recent, &landed);
        let f = go_r(&mut s, &[], 60 * H, &recent, &landed);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].at.run, 3);
        filed(&mut s, &f[0], Some(6), 60 * H, "u");
        assert!(go_r(&mut s, &[], 90 * H, &recent, &landed).is_empty());
    }

    #[test]
    fn a_newer_run_of_a_commit_before_the_landing_is_held() {
        let mut s = State::default();
        red_filed(&mut s, 5);
        let landed: BTreeMap<u64, u64> = [(5, 50 * H)].into();
        let mut r3 = run(3, "CI", "failure", &[("a", "failure")]);
        r3.created_ms = 10 * H;
        go_r(&mut s, &[r3], 60 * H, &[], &landed);
        assert!(go_r(&mut s, &[], 70 * H, &[], &landed).is_empty());
    }

    #[test]
    fn a_task_closed_citing_the_run_is_recorded_not_refiled() {
        let mut s = State::default();
        go(
            &mut s,
            &[run(1, "CI", "success", &[("a", "success")])],
            0,
            &none(),
            0,
        );
        go(
            &mut s,
            &[run(2, "CI", "failure", &[("a", "failure")])],
            1000,
            &none(),
            0,
        );
        let recent = [RecentTask {
            id: 9,
            title: "something else".into(),
            body: "`CI/a` Run: https://x/runs/2".into(),
        }];
        assert!(go_r(&mut s, &[], 4 * H, &recent, &BTreeMap::new()).is_empty());
        let js = &s.workflows["CI"].jobs["a"];
        assert_eq!((js.ticket, js.covered), (Some(9), 2));
    }

    #[test]
    fn a_terminal_task_outside_the_lookback_does_not_hold_a_newer_run() {
        // The daemon passes only tasks inside the lookback, so an older one is absent.
        let mut s = State::default();
        go(
            &mut s,
            &[run(1, "CI", "success", &[("a", "success")])],
            0,
            &none(),
            0,
        );
        go(
            &mut s,
            &[run(2, "CI", "failure", &[("a", "failure")])],
            1000,
            &none(),
            0,
        );
        assert_eq!(go_r(&mut s, &[], 4 * H, &[], &BTreeMap::new()).len(), 1);
    }

    #[test]
    fn the_gate_freeze_holds_while_a_refile_is_held_back() {
        let mut s = State::default();
        go(
            &mut s,
            &[run(1, "CI", "success", &[("pr-gate", "success")])],
            0,
            &none(),
            0,
        );
        let f = go(
            &mut s,
            &[run(2, "CI", "failure", &[("pr-gate", "failure")])],
            1000,
            &none(),
            0,
        );
        assert_eq!(f.len(), 1);
        filed(&mut s, &f[0], Some(4), 1000, "u");
        assert!(go_r(&mut s, &[], 100 * H, &[], &BTreeMap::new()).is_empty());
        assert_eq!(freezing(&s), vec![("CI/pr-gate".to_string(), Some(4))]);
        assert_eq!(held_back(&s, &rules()).len(), 1);
    }

    #[test]
    fn a_name_in_an_open_task_or_issue_stops_a_second_filing() {
        let mut s = State::default();
        go(
            &mut s,
            &[run(
                1,
                "CI",
                "success",
                &[("pr-gate", "success"), ("a", "success")],
            )],
            0,
            &none(),
            0,
        );
        let named: BTreeMap<String, Option<u64>> = [
            ("CI/pr-gate".to_string(), Some(4)),
            ("CI/a".to_string(), None),
        ]
        .into();
        let f = go(
            &mut s,
            &[run(
                2,
                "CI",
                "failure",
                &[("pr-gate", "failure"), ("a", "failure")],
            )],
            1000,
            &named,
            0,
        );
        assert!(f.is_empty());
        // The gate still freezes, on the task that names it.
        assert_eq!(freezing(&s), vec![("CI/pr-gate".to_string(), Some(4))]);
        let f = go(
            &mut s,
            &[run(3, "CI", "failure", &[("a", "failure")])],
            2000,
            &named,
            0,
        );
        assert!(f.is_empty());
    }

    #[test]
    fn past_the_cap_a_red_is_proposed() {
        let mut s = State::default();
        go(
            &mut s,
            &[run(1, "CI", "success", &[("pr-gate", "success")])],
            0,
            &none(),
            2,
        );
        let f = go(
            &mut s,
            &[run(2, "CI", "failure", &[("pr-gate", "failure")])],
            1000,
            &none(),
            2,
        );
        assert_eq!(f.len(), 1);
        assert!(f[0].proposed && f[0].urgent);
    }

    #[test]
    fn an_excluded_workflow_keeps_its_baseline_and_is_never_filed() {
        let mut s = State::default();
        go(
            &mut s,
            &[run(1, "Gherkin", "success", &[("a", "success")])],
            0,
            &none(),
            0,
        );
        go(
            &mut s,
            &[run(2, "Gherkin", "failure", &[("a", "failure")])],
            1000,
            &none(),
            0,
        );
        let f = go(
            &mut s,
            &[run(3, "Gherkin", "failure", &[("a", "failure")])],
            2000,
            &none(),
            0,
        );
        assert!(f.is_empty());
        assert!(s.workflows["Gherkin"].jobs["a"].red);
        assert!(go(&mut s, &[], 100 * 3_600_000, &none(), 0).is_empty());
        assert!(pending(&s, &rules()).is_empty());
    }

    /// A run with a failing script per job, for the auto-approval tests.
    fn run_sigs(id: u64, jobs: &[(&str, &str)]) -> RunRec {
        let mut r = run(id, "CI", "failure", jobs);
        for j in &mut r.jobs {
            if j.conclusion.as_deref() == Some("failure") {
                j.signatures = vec!["scripts/exit/s9.sh".into()];
            }
        }
        r
    }

    fn auto_go(
        s: &mut State,
        runs: &[RunRec],
        named: &BTreeMap<String, Option<u64>>,
        open_approved: u32,
    ) -> Vec<File> {
        let mut r = rules();
        r.auto_approve = true;
        step(
            s,
            runs,
            &Ctx {
                rules: &r,
                now_ms: 5000,
                named,
                open_auto: 0,
                open_approved,
                recent: &[],
                landed: &BTreeMap::new(),
                named_sigs: &none(),
            },
        )
    }

    fn seeded() -> State {
        let mut s = State::default();
        auto_go(
            &mut s,
            &[run(
                1,
                "CI",
                "success",
                &[("a", "success"), ("b", "success")],
            )],
            &none(),
            0,
        );
        s
    }

    #[test]
    fn auto_approve_files_a_red_seen_on_two_runs_approved_and_one_run_not_at_all() {
        let mut s = seeded();
        // Seen once: nothing is filed yet (agreement), so nothing is approved either.
        assert!(auto_go(&mut s, &[run_sigs(2, &[("a", "failure")])], &none(), 0).is_empty());
        let f = auto_go(&mut s, &[run_sigs(3, &[("a", "failure")])], &none(), 0);
        assert_eq!(f.len(), 1);
        assert!(f[0].auto && !f[0].proposed);
    }

    #[test]
    fn auto_approve_off_leaves_filing_as_it_was() {
        let mut s = seeded();
        go(
            &mut s,
            &[run_sigs(2, &[("a", "failure")])],
            5000,
            &none(),
            0,
        );
        let f = go(
            &mut s,
            &[run_sigs(3, &[("a", "failure")])],
            5000,
            &none(),
            0,
        );
        assert!(!f[0].auto && !f[0].proposed);
    }

    #[test]
    fn a_long_red_seen_on_one_run_is_proposed() {
        let mut s = seeded();
        auto_go(&mut s, &[run_sigs(2, &[("a", "failure")])], &none(), 0);
        let mut r = rules();
        r.auto_approve = true;
        r.agreement = 1;
        r.red_ticket_after_ms = 1;
        let f = step(
            &mut s,
            &[],
            &Ctx {
                rules: &r,
                now_ms: 9_000_000,
                named: &none(),
                open_auto: 0,
                open_approved: 0,
                recent: &[],
                landed: &BTreeMap::new(),
                named_sigs: &none(),
            },
        );
        // One red run only: not confirmed.
        assert_eq!(f.len(), 1);
        assert!(f[0].proposed && !f[0].auto);
    }

    #[test]
    fn a_red_an_open_task_covers_files_nothing() {
        let mut s = seeded();
        let named: BTreeMap<String, Option<u64>> = [("CI/a".to_string(), Some(7))].into();
        auto_go(&mut s, &[run_sigs(2, &[("a", "failure")])], &named, 0);
        assert!(auto_go(&mut s, &[run_sigs(3, &[("a", "failure")])], &named, 0).is_empty());
    }

    #[test]
    fn shards_of_one_job_fold_into_one_ticket() {
        let mut s = State::default();
        let shards = ["x (ubuntu, 1)", "x (ubuntu, 2)", "x (ubuntu, 4)"];
        let green: Vec<(&str, &str)> = shards.iter().map(|n| (*n, "success")).collect();
        let red: Vec<(&str, &str)> = shards.iter().map(|n| (*n, "failure")).collect();
        auto_go(&mut s, &[run(1, "CI", "success", &green)], &none(), 0);
        auto_go(&mut s, &[run_sigs(2, &red)], &none(), 0);
        let f = auto_go(&mut s, &[run_sigs(3, &red)], &none(), 0);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].also.len(), 2);
        assert!(f[0].auto);
    }

    #[test]
    fn one_failure_on_two_systems_is_one_ticket_naming_both() {
        let mut s = State::default();
        let green = [("t (ubuntu)", "success"), ("t (macos)", "success")];
        let red = [("t (ubuntu)", "failure"), ("t (macos)", "failure")];
        auto_go(&mut s, &[run(1, "CI", "success", &green)], &none(), 0);
        auto_go(&mut s, &[run_sigs(2, &red)], &none(), 0);
        let f = auto_go(&mut s, &[run_sigs(3, &red)], &none(), 0);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].also, vec!["t (macos)".to_string()]);
    }

    #[test]
    fn the_cap_on_auto_approved_tickets_holds() {
        let mut s = seeded();
        let two = [("a", "failure"), ("b", "failure")];
        // Different failures, so they are two tickets.
        let mut r2 = run_sigs(2, &two);
        let mut r3 = run_sigs(3, &two);
        for r in [&mut r2, &mut r3] {
            r.jobs[1].signatures = vec!["scripts/exit/s10.sh".into()];
        }
        auto_go(&mut s, &[r2], &none(), 4);
        let f = auto_go(&mut s, &[r3], &none(), 4);
        assert_eq!(f.len(), 2);
        assert_eq!(f.iter().filter(|x| x.auto).count(), 1);
        assert_eq!(f.iter().filter(|x| x.proposed).count(), 1);
    }
}
