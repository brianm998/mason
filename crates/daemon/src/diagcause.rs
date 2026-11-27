// SPDX-License-Identifier: GPL-3.0-or-later

//! Diagnosing a cause outside a task's code (s1176, decision 1166).
//!
//! `diagnose.rs` (s250, s1048) looks at one failed task. This module looks at what several tasks
//! share when the cause is not their code: a landing batch waiting on CI that a job on the base
//! branch hangs, a burst of lost hosts. One read-only session per **cause** is started, with the
//! facts MASON already has gathered before it begins ([`gather`]), and it ends in the same decision
//! card as #1044's, whose options may now also file the ticket for the cause, land now, wait for an
//! open task or open an owner session. A fix-now action and a fix-later action always come as a
//! pair when the cause is outside the code (the contract checks it).
//!
//! Triggers (each starts at most one diagnosis per cause, see [`start_cause`]):
//!
//! * a landing step failed, of any class ([`on_step_failed`]); its automatic retry is held;
//! * a second outside failure of a task with the same `health::signature`;
//! * a `ci-stuck` alert of a batch that is still waiting ([`pass`]);
//! * an `autoretry::bursts` group ([`pass`]).
//!
//! A cause is keyed by `sig:<project>:<signature>`, `batch:<project>:<id>` or `burst:...`. While
//! a session runs or its card is open, later failures with that key join the card instead of
//! starting another session. The card is stored on one diagnosis row (`args.kind = "cause"`,
//! `args.tasks` the tasks it speaks for) and acts on every task it lists. At most
//! [`CAUSE_PER_DAY`] cause-keyed diagnoses per project per day, a session without a verdict counted.
//!
//! A `land_now` from a card is the owner's Land now with its debt, applied only after the owner
//! chose it; nothing here lands anything by itself.

use crate::ci_watch::Source;
use crate::daemon::{now_ms, Daemon};
use mason_adapters::github::{ActionsJob, ActionsRun};
use mason_core::*;
use mason_store::DiagnosisRow;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

/// Cause-keyed diagnoses per project per day, on top of the per-failure budget.
pub const CAUSE_PER_DAY: usize = 4;
const DAY_MS: u64 = 86_400_000;
/// The session's deadline; the auto-retry hold lapses with it.
pub const SESSION_DEADLINE_S: u64 = 900;
/// Lines of a CI job's log put in the facts.
const JOB_LOG_LINES: usize = 80;
/// Newest base runs read per workflow.
const BASE_RUNS: usize = 3;

// ---- pure: keys, budget, rules --------------------------------------------------------------

/// The cause key of a task failure: the project and the failure's signature.
pub fn key_for_failure(project: &str, failure: &str) -> String {
    format!("sig:{project}:{}", crate::health::signature(failure))
}

/// The cause key of a stuck batch.
pub fn key_for_batch(project: &str, batch: u64) -> String {
    format!("batch:{project}:{batch}")
}

/// The cause key of a burst of failures with one normalised reason.
pub fn key_for_burst(project: &str, reason: &str) -> String {
    format!(
        "burst:{project}:{}",
        reason.chars().take(100).collect::<String>()
    )
}

/// Whether another cause-keyed diagnosis may start today, given how many the project had.
pub fn day_budget(today: usize) -> Result<(), String> {
    if today >= CAUSE_PER_DAY {
        return Err(format!(
            "the project has had {CAUSE_PER_DAY} cause diagnoses in the last day, the most there are"
        ));
    }
    Ok(())
}

/// A cause already diagnosed for the failure that began at `failed_at`: a row of the same key
/// written since.
pub fn covered(rows: &[(String, u64)], key: &str, failed_at: u64) -> bool {
    rows.iter().any(|(k, at)| k == key && *at >= failed_at)
}

/// Land now is allowed only when every one of the task's own suites is green, or red the same way
/// on the base. `own_red`: the suites that are red for the task; `same_on_base`: names red there.
pub fn land_now_ok(own_red: &[String], same_on_base: &[String]) -> Result<(), String> {
    let bad: Vec<&String> = own_red
        .iter()
        .filter(|s| !same_on_base.contains(s))
        .collect();
    if bad.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "Land now is refused: {} red for the task and not red on the base",
            bad.iter()
                .map(|s| format!("`{s}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ))
    }
}

/// The first sentence of a text (up to a full stop followed by a space, or a line break), cut to
/// `max` words. A dot inside `tests/scan.rs` ends nothing.
pub fn cause_words(summary: &str, max: usize) -> String {
    let t = summary.trim();
    let end = [t.find(". "), t.find('\n')]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(t.len());
    let words: Vec<&str> = t[..end].split_whitespace().collect();
    words[..words.len().min(max)].join(" ")
}

/// The one line spoken or pushed: `#<task>: <cause in at most 12 words>. Recommended: <label>.`
/// With several tasks the first is named and the rest counted.
pub fn notice_line(tasks: &[u64], summary: &str, label: &str) -> String {
    let who = match tasks {
        [] => "#?".to_string(),
        [one] => format!("#{one}"),
        [first, rest @ ..] => format!("#{first} and {} more", rest.len()),
    };
    format!(
        "{who}: {}. Recommended: {}.",
        cause_words(summary, 12),
        label.trim().trim_end_matches('.')
    )
}

/// What the cause is made of, as the prompt's opening says it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Trigger {
    /// A landing step failed.
    Landing,
    /// The same outside failure a second time.
    Repeat,
    /// The batch's CI has not changed for the `[ci] stuck_after_s`.
    Stuck { batch: u64 },
    /// Several tasks failed outside the code with one reason.
    Burst { reason: String },
}

impl Trigger {
    pub fn text(&self) -> String {
        match self {
            Trigger::Landing => "a landing step failed".into(),
            Trigger::Repeat => "the same failure outside the code came a second time".into(),
            Trigger::Stuck { batch } => format!(
                "the CI of landing batch #{batch} has not changed for the stuck-after time, and the batch is still waiting"
            ),
            Trigger::Burst { reason } => {
                format!("several tasks failed outside the code with one reason: {reason}")
            }
        }
    }
}

// ---- pure: CI facts -------------------------------------------------------------------------

/// A job's state as one word: `green`, `red`, `cancelled`, `timed_out`, `running`, `queued` or
/// the conclusion as GitHub spells it.
pub fn job_state(status: &str, conclusion: Option<&str>) -> String {
    if status != "completed" {
        return match status {
            "in_progress" => "running".into(),
            "queued" | "waiting" | "pending" | "requested" => "queued".into(),
            other => other.to_string(),
        };
    }
    match conclusion {
        Some("success" | "neutral" | "skipped") => "green".into(),
        Some("failure") => "red".into(),
        Some(other) => other.to_string(),
        None => "unfinished".into(),
    }
}

/// A job of the batch's own CI run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobFact {
    pub workflow: String,
    pub name: String,
    pub state: String,
    pub log: String,
    pub annotations: String,
    pub url: String,
}

/// A job in one of the base branch's runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BaseFact {
    pub workflow: String,
    pub job: String,
    pub run: u64,
    pub sha: String,
    pub state: String,
    pub url: String,
}

/// Whether the base's runs of `job` (newest first) are red, cancelled at the time limit or
/// unfinished: the newest run that has the job says.
pub fn same_on_base(base: &[BaseFact], job: &str) -> bool {
    base.iter().find(|b| b.job == job).is_some_and(|b| {
        matches!(
            b.state.as_str(),
            "red" | "timed_out" | "running" | "queued" | "unfinished"
        )
    })
}

/// The sha of the newest base run in which `job` was green.
pub fn last_green(base: &[BaseFact], job: &str) -> Option<String> {
    base.iter()
        .find(|b| b.job == job && b.state == "green")
        .map(|b| b.sha.clone())
}

/// What was read of the CI.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CiFacts {
    pub ci_ref: String,
    pub jobs: Vec<JobFact>,
    pub base: Vec<BaseFact>,
    /// Names of the batch's jobs that are not green and are the same on the base.
    pub same_on_base: Vec<String>,
    /// `(job, sha)`: where each such job was last green on the base.
    pub last_green: Vec<(String, String)>,
    pub notes: Vec<String>,
}

fn tail(text: &str, lines: usize) -> String {
    crate::jobs::tail(text, lines)
}

/// Read the batch's CI runs on `ci_ref` and the base branch's runs of the same workflows.
pub fn gather_ci(src: &dyn Source, ci_ref: &str, base: &str) -> CiFacts {
    let mut out = CiFacts {
        ci_ref: ci_ref.to_string(),
        ..Default::default()
    };
    let runs: Vec<ActionsRun> = match src.runs(ci_ref) {
        Ok(r) => r,
        Err(e) => {
            out.notes
                .push(format!("the batch's CI runs could not be read: {e}"));
            vec![]
        }
    };
    let mut workflows: Vec<String> = vec![];
    for r in &runs {
        if !workflows.contains(&r.workflow) {
            workflows.push(r.workflow.clone());
        }
        let jobs: Vec<ActionsJob> = match src.jobs(r.id) {
            Ok(j) => j,
            Err(e) => {
                out.notes
                    .push(format!("the jobs of run {} could not be read: {e}", r.id));
                continue;
            }
        };
        for j in jobs {
            let ann = || src.annotations(j.id);
            let concluded = crate::ci_watch::job_conclusion(
                r.conclusion.as_deref(),
                j.conclusion.as_deref(),
                ann,
            );
            let state = job_state(&j.status, concluded.as_deref());
            let (log, annotations) = if state == "green" {
                (String::new(), String::new())
            } else {
                (
                    src.log_long(j.id)
                        .map(|l| tail(&l, JOB_LOG_LINES))
                        .unwrap_or_default(),
                    src.annotations(j.id).unwrap_or_default(),
                )
            };
            out.jobs.push(JobFact {
                workflow: r.workflow.clone(),
                name: j.name.clone(),
                state,
                log,
                annotations,
                url: r.url.clone(),
            });
        }
    }
    match src.runs(base) {
        Err(e) => out
            .notes
            .push(format!("the base branch's runs could not be read: {e}")),
        Ok(base_runs) => {
            for w in &workflows {
                for r in base_runs
                    .iter()
                    .filter(|r| &r.workflow == w)
                    .take(BASE_RUNS)
                {
                    let Ok(jobs) = src.jobs(r.id) else { continue };
                    for j in jobs {
                        let concluded = crate::ci_watch::job_conclusion(
                            r.conclusion.as_deref(),
                            j.conclusion.as_deref(),
                            || src.annotations(j.id),
                        );
                        out.base.push(BaseFact {
                            workflow: r.workflow.clone(),
                            job: j.name.clone(),
                            run: r.id,
                            sha: r.sha.clone(),
                            state: job_state(&j.status, concluded.as_deref()),
                            url: r.url.clone(),
                        });
                    }
                }
            }
        }
    }
    for j in out.jobs.iter().filter(|j| j.state != "green") {
        if same_on_base(&out.base, &j.name) && !out.same_on_base.contains(&j.name) {
            out.same_on_base.push(j.name.clone());
            if let Some(sha) = last_green(&out.base, &j.name) {
                out.last_green.push((j.name.clone(), sha));
            }
        }
    }
    out
}

/// A commit on the base since the job was last green there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suspect {
    pub sha: String,
    pub subject: String,
    /// The task whose merge it is (`mason/<project>/tN`).
    pub task: Option<u64>,
}

/// The suspects in `git log --first-parent --format=%H%x09%s` lines.
pub fn suspects(log: &str, project: &str) -> Vec<Suspect> {
    let needle = format!("mason/{project}/t");
    log.lines()
        .filter_map(|l| {
            let (sha, subject) = l.split_once('\t')?;
            let task = subject.find(&needle).and_then(|i| {
                let rest = &subject[i + needle.len()..];
                let n: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                n.parse().ok()
            });
            Some(Suspect {
                sha: sha.chars().take(12).collect(),
                subject: subject.chars().take(120).collect(),
                task,
            })
        })
        .collect()
}

/// Open tasks whose title or body names one of `needles` (job or test names): `(number, title)`.
pub fn related(
    tasks: &[(u64, String, String)],
    needles: &[String],
    exclude: &[u64],
) -> Vec<(u64, String)> {
    let needles: Vec<String> = needles
        .iter()
        .map(|n| n.to_lowercase())
        .filter(|n| n.chars().count() >= 4)
        .collect();
    tasks
        .iter()
        .filter(|(n, _, _)| !exclude.contains(n))
        .filter(|(_, title, body)| {
            let hay = format!("{title}\n{body}").to_lowercase();
            needles.iter().any(|n| hay.contains(n.as_str()))
        })
        .take(8)
        .map(|(n, t, _)| (*n, t.clone()))
        .collect()
}

// ---- gathering ------------------------------------------------------------------------------

/// Everything the session is given beyond the tasks' own logs.
#[derive(Clone, Debug, Default)]
pub struct Gathered {
    pub batch: Option<u64>,
    pub ci: Option<CiFacts>,
    pub suspects: Vec<Suspect>,
    pub project_red: Vec<String>,
    pub retry_schedule: Vec<String>,
    pub alerts: Vec<String>,
    pub related: Vec<(u64, String, String)>,
    pub tasks_text: Vec<String>,
}

/// The facts as the prompt shows them. Pure.
pub fn render(g: &Gathered) -> String {
    let mut s = String::new();
    s.push_str("# The tasks\n");
    s.push_str(&g.tasks_text.join("\n\n"));
    s.push_str("\n\n# The batch and its CI\n");
    match (&g.batch, &g.ci) {
        (None, _) => s.push_str("(the tasks are in no landing batch)\n"),
        (Some(b), None) => s.push_str(&format!(
            "batch #{b}: no CI record (CI never started, or the daemon restarted since)\n"
        )),
        (Some(b), Some(ci)) => {
            s.push_str(&format!("batch #{b}, CI ref {}\n", ci.ci_ref));
            for n in &ci.notes {
                s.push_str(&format!("note: {n}\n"));
            }
            for j in &ci.jobs {
                let mark = if ci.same_on_base.contains(&j.name) {
                    " [same on the base]"
                } else {
                    ""
                };
                s.push_str(&format!(
                    "- {} / {}: {}{mark} {}\n",
                    j.workflow, j.name, j.state, j.url
                ));
                if !j.annotations.trim().is_empty() {
                    s.push_str(&format!("  annotations: {}\n", j.annotations.trim()));
                }
                if !j.log.trim().is_empty() {
                    s.push_str(&format!("  log tail of {}:\n{}\n", j.name, j.log));
                }
            }
            s.push_str("\n# The base branch's runs of the same workflows (newest first)\n");
            for b in &ci.base {
                s.push_str(&format!(
                    "- {} / {} run {} at {}: {}\n",
                    b.workflow, b.job, b.run, b.sha, b.state
                ));
            }
            for (job, sha) in &ci.last_green {
                s.push_str(&format!("`{job}` was last green on the base at {sha}\n"));
            }
        }
    }
    s.push_str("\n# Suspects: commits on the base since the job was last green\n");
    if g.suspects.is_empty() {
        s.push_str("(none found)\n");
    }
    for x in &g.suspects {
        s.push_str(&format!(
            "- {} {}{}\n",
            x.sha,
            x.subject,
            x.task.map_or(String::new(), |t| format!(" (task #{t})"))
        ));
    }
    s.push_str("\n# base_red and persistent_red rows of the project\n");
    if g.project_red.is_empty() {
        s.push_str("(none)\n");
    }
    for r in &g.project_red {
        s.push_str(&format!("- {r}\n"));
    }
    s.push_str("\n# The automatic retry schedule\n");
    for r in &g.retry_schedule {
        s.push_str(&format!("- {r}\n"));
    }
    s.push_str("\n# Open alerts for the project\n");
    if g.alerts.is_empty() {
        s.push_str("(none)\n");
    }
    for a in &g.alerts {
        s.push_str(&format!("- {a}\n"));
    }
    s.push_str("\n# Open tasks that name the same job or test (point to one instead of filing a duplicate)\n");
    if g.related.is_empty() {
        s.push_str("(none)\n");
    }
    for (n, p, t) in &g.related {
        s.push_str(&format!("- #{n} [{p}] {t}\n"));
    }
    s
}

/// The prompt: the opening, the facts, and what the session is asked to write. Pure.
pub fn prompt(
    key: &str,
    trigger: &Trigger,
    tasks: &[u64],
    facts: &str,
    steps: &[String],
) -> String {
    let list = tasks
        .iter()
        .map(|t| format!("#{t}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "Tasks {list} are stuck or failed and no one is watching ({why}). You are the diagnosis of \
         the cause `{key}`: you may not change any file, you have no network and no `gh`; \
         everything MASON knows is below. Put the owner's choice to them as a decision card. End \
         with exactly one call of `submit_diagnosis`: a `summary`, 2 to 4 `options`, exactly one \
         `recommended`, and `cause` (`code`: the tasks' own work; `outside`: something around \
         them; `base`: the base branch is already broken; `pipeline`: MASON itself).\n\n\
         An option has an `action`, or `actions` (several, applied in order, so \"file the fix, \
         then land now\" is one choice): `retry` (from one of the steps {steps}, with a `note`), \
         `explore`, `close`, `cancel`, `file_task` (`project`, `title`, `body`, optional `after`: \
         a ticket for the cause, filed by MASON's own proposal rules, so give `evidence` that \
         names the file or log lines), `land_now` (`reason`; the owner's Land now, which records a \
         debt; allowed only when every one of the task's own suites is green or the same on the \
         base), `wait_for` (`task`: an open task that covers the cause) or `open_session` \
         (`where`: `task` or `base`, and a `brief`).\n\n\
         When the cause is not the code (`outside`, `base` or `pipeline`) the recommended option \
         must hold a fix-later action (`file_task`, or `wait_for` on an existing open task) and a \
         fix-now action (`land_now`, `retry`, `wait_for` or `open_session`). In the summary:\n\
         1. name the cause, saying what changed and where, with the evidence lines;\n\
         2. say whether the same failure is on the base (a job marked `[same on the base]` is);\n\
         3. name the suspect task from the suspects list, if there is one;\n\
         4. write the fix-later ticket's body in the target project's own format (for MASON, the \
            format in CLAUDE.md \"Filing tasks\": the problem, **Cause** with file:line, **Change**, \
            **Coordinate**, **Trust**, **Tests**, **Exit test**);\n\
         5. say why the recommended option beats the others;\n\
         6. if an open ticket below covers it, use `wait_for` on it instead of filing a duplicate.\n\
         Put the decisive lines in `evidence_lines`. Set `no_code_change` only if the code is right \
         and running it again is expected to pass.\n\n\
         # Why this diagnosis started\n{why}\n\n{facts}",
        why = trigger.text(),
        steps = steps.join(", "),
    )
}

/// Read the facts for the tasks of a cause. Reads the store, git and, through `src`, the CI.
pub fn gather(
    d: &Arc<Daemon>,
    pid: ProjectId,
    tasks: &[u64],
    batch: Option<u64>,
    src: Option<&dyn Source>,
) -> Gathered {
    let mut g = Gathered::default();
    let Some(p) = d.project(pid) else { return g };
    let Ok(w) = d.world() else { return g };
    let rows = d.store.batches(Some(&p.id), 60).unwrap_or_default();
    let batch = batch.or_else(|| {
        rows.iter()
            .find(|b| {
                b.tasks
                    .split(',')
                    .filter_map(|x| x.trim().parse::<u64>().ok())
                    .any(|t| tasks.contains(&t))
            })
            .map(|b| b.id)
    });
    g.batch = batch;
    let batch_tasks: Vec<u64> = batch
        .and_then(|b| rows.iter().find(|r| r.id == b))
        .map(|r| {
            r.tasks
                .split(',')
                .filter_map(|x| x.trim().parse().ok())
                .collect()
        })
        .unwrap_or_default();
    let mut names: Vec<String> = vec![];
    // The tasks: failure, history, plan's diffstat is the single-task session's own business.
    for t in tasks {
        let Some(task) = w.tasks.get(&TaskId(*t)) else {
            continue;
        };
        let dir = d.cfg.task_dir(*t);
        let history = std::fs::read_to_string(dir.join("feedback-history.txt")).unwrap_or_default();
        g.tasks_text.push(format!(
            "## #{t} {} ({})\nstate: {}\nfailure: {}\nbody (start):\n{}\nrounds told:\n{}\nlog tails:\n{}",
            task.title,
            p.id,
            mason_store::state_name(task.state),
            task.failure.clone().unwrap_or_default(),
            task.body.chars().take(600).collect::<String>(),
            crate::jobs::tail(&history, 20),
            crate::diagnose::log_tails(&dir, 2, 30),
        ));
        if let Some(r) = task.failure.as_deref().and_then(crate::jobs::red_suite_of) {
            names.push(r);
        }
        g.retry_schedule
            .push(match crate::autoretry::status(d, task) {
                None => format!("#{t}: not a failure outside the code; no automatic retry"),
                Some((done, None)) => {
                    format!("#{t}: {done} automatic retries made, none will be made")
                }
                Some((done, Some(at))) => format!(
                    "#{t}: {done} automatic retries made, the next at {at} ms (now {})",
                    now_ms()
                ),
            });
    }
    if let (Some(b), Some(src)) = (batch, src) {
        match d.landing.ci.get(b) {
            Some(now) => {
                let ci = gather_ci(src, &now.ci_ref, &p.base);
                names.extend(
                    ci.jobs
                        .iter()
                        .filter(|j| j.state != "green")
                        .map(|j| j.name.clone()),
                );
                g.ci = Some(ci);
            }
            None => g.ci = None,
        }
    }
    // Suspects: since the oldest sha any not-green job was last green, else the recent merges.
    let base_ref = p.base_ref();
    let from =
        g.ci.as_ref()
            .and_then(|c| c.last_green.first().map(|(_, sha)| sha.clone()));
    let log = from
        .as_ref()
        .and_then(|sha| {
            mason_landing::git::run(
                &p.path,
                &[
                    "log",
                    "--first-parent",
                    "--format=%H%x09%s",
                    &format!("{sha}..{base_ref}"),
                ],
            )
            .ok()
        })
        .filter(|l| !l.trim().is_empty())
        .or_else(|| {
            mason_landing::git::run(
                &p.path,
                &[
                    "log",
                    "--first-parent",
                    "-n",
                    "30",
                    "--format=%H%x09%s",
                    &base_ref,
                ],
            )
            .ok()
        })
        .unwrap_or_default();
    g.suspects = suspects(&log, &p.id)
        .into_iter()
        .filter(|s| s.task.is_some())
        .collect();
    for r in d.store.base_reds().unwrap_or_default() {
        g.project_red.push(format!(
            "base_red: task #{} waits on `{}` ({}) {}",
            r.task, r.suite, r.kind, r.first_error
        ));
    }
    for (suite, test, at) in d
        .store
        .persistent_reds_since(now_ms().saturating_sub(7 * DAY_MS))
        .unwrap_or_default()
    {
        g.project_red
            .push(format!("persistent_red: {suite}::{test} at {at} ms"));
    }
    g.alerts = crate::attention::alerts_now(d)
        .into_iter()
        .filter(|a| a.2.contains(&format!("{}:", p.id)) || a.0.contains(&format!(":{}:", pid.0)))
        .take(10)
        .map(|a| {
            format!(
                "{} [{}] {}",
                a.0,
                a.1,
                a.2.chars().take(200).collect::<String>()
            )
        })
        .collect();
    let own = crate::proposals::self_project(d).map(|(id, _)| id);
    let open: Vec<(u64, String, String, String)> = w
        .tasks
        .values()
        .filter(|t| !matches!(t.state, State::Landed | State::Canceled))
        .filter(|t| t.project == pid || Some(t.project) == own)
        .filter(|t| !tasks.contains(&t.id.0) && !batch_tasks.contains(&t.id.0))
        .map(|t| {
            (
                t.id.0,
                d.project(t.project).map_or(String::new(), |p| p.id.clone()),
                t.title.clone(),
                t.body.clone(),
            )
        })
        .collect();
    let flat: Vec<(u64, String, String)> = open
        .iter()
        .map(|(n, _, t, b)| (*n, t.clone(), b.clone()))
        .collect();
    g.related = related(&flat, &names, tasks)
        .into_iter()
        .map(|(n, title)| {
            let proj = open
                .iter()
                .find(|o| o.0 == n)
                .map(|o| o.1.clone())
                .unwrap_or_default();
            (n, proj, title)
        })
        .collect();
    g
}

// ---- the CI source --------------------------------------------------------------------------

/// A CI source a test put in place for a daemon (its data directory is the key).
type Shared = Arc<dyn Source + Send + Sync>;
static SOURCES: Mutex<Vec<(String, Shared)>> = Mutex::new(Vec::new());

/// Use `src` instead of GitHub for the facts of daemon `d`. For tests.
pub fn set_source(d: &Daemon, src: Shared) {
    let scope = scope(d);
    let mut v = SOURCES.lock().unwrap_or_else(|e| e.into_inner());
    v.retain(|(s, _)| *s != scope);
    v.push((scope, src));
}

struct Delegate(Shared);

impl Source for Delegate {
    fn runs(&self, branch: &str) -> Result<Vec<ActionsRun>, String> {
        self.0.runs(branch)
    }
    fn jobs(&self, run: u64) -> Result<Vec<ActionsJob>, String> {
        self.0.jobs(run)
    }
    fn log_tail(&self, job: u64) -> Option<String> {
        self.0.log_tail(job)
    }
    fn annotations(&self, job: u64) -> Option<String> {
        self.0.annotations(job)
    }
    fn log_long(&self, job: u64) -> Option<String> {
        self.0.log_long(job)
    }
}

fn scope(d: &Daemon) -> String {
    d.cfg.data_dir.display().to_string()
}

fn with_source<R>(d: &Daemon, pid: ProjectId, f: impl FnOnce(Option<&dyn Source>) -> R) -> R {
    let over = SOURCES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .find(|(s, _)| *s == scope(d))
        .map(|(_, s)| s.clone());
    if let Some(s) = over {
        return f(Some(&Delegate(s)));
    }
    match d
        .project(pid)
        .map(|p| crate::ci_watch::source(p, "the diagnosis"))
    {
        Some(Ok((gh, _))) => f(Some(&gh)),
        _ => f(None),
    }
}

// ---- the live causes and the holds ----------------------------------------------------------

struct Live {
    scope: String,
    key: String,
    tasks: Vec<u64>,
    started: u64,
    ran_on: String,
}

static LIVE: Mutex<Vec<Live>> = Mutex::new(Vec::new());
/// `(scope, task, until ms)`: tasks whose automatic retry waits for a diagnosis.
static HELD: Mutex<Vec<(String, u64, u64)>> = Mutex::new(Vec::new());
/// Starting a cause (and joining one) is decided under this.
static GATE: Mutex<()> = Mutex::new(());

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Gives the live mark and the holds back when dropped.
struct LiveGuard {
    scope: String,
    key: String,
}

impl LiveGuard {
    fn tasks(&self) -> Vec<u64> {
        lock(&LIVE)
            .iter()
            .find(|l| l.scope == self.scope && l.key == self.key)
            .map(|l| l.tasks.clone())
            .unwrap_or_default()
    }
    fn ran_on(&self, text: &str) {
        if let Some(l) = lock(&LIVE)
            .iter_mut()
            .find(|l| l.scope == self.scope && l.key == self.key)
        {
            l.ran_on = text.to_string();
        }
    }
    /// Let the automatic retries go now: the session is over.
    fn release(&self) {
        let tasks = self.tasks();
        lock(&HELD).retain(|h| !(h.0 == self.scope && tasks.contains(&h.1)));
    }
}

impl Drop for LiveGuard {
    fn drop(&mut self) {
        self.release();
        lock(&LIVE).retain(|l| !(l.scope == self.scope && l.key == self.key));
    }
}

/// A cause's session running for `task` of daemon `d`: when it started and on what.
pub fn live_for(d: &Daemon, task: u64) -> Option<(u64, String)> {
    let scope = scope(d);
    lock(&LIVE)
        .iter()
        .find(|l| l.scope == scope && l.tasks.contains(&task))
        .map(|l| (l.started, l.ran_on.clone()))
}

fn hold(scope: &str, task: u64, from: u64) {
    let mut h = lock(&HELD);
    h.retain(|x| !(x.0 == scope && x.1 == task));
    h.push((scope.to_string(), task, from + SESSION_DEADLINE_S * 1000));
}

/// The cause rows (`args.kind = "cause"`) that are pending, with their arguments.
pub fn pending_causes(d: &Daemon) -> Vec<(DiagnosisRow, Value)> {
    d.store
        .diagnoses(None)
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.status == "pending" && r.args.contains("\"kind\":\"cause\""))
        .filter_map(|r| {
            let a: Value = serde_json::from_str(&r.args).ok()?;
            (a["kind"] == "cause").then_some((r, a))
        })
        .collect()
}

fn is_cause_row(r: &DiagnosisRow) -> bool {
    crate::diagnose::is_cause(r)
}

/// The tasks a row speaks for: its `tasks`, else its own task.
pub fn tasks_of(r: &DiagnosisRow) -> Vec<u64> {
    serde_json::from_str::<Value>(&r.args)
        .ok()
        .and_then(|a| {
            a["tasks"]
                .as_array()
                .map(|l| l.iter().filter_map(Value::as_u64).collect::<Vec<_>>())
        })
        .filter(|l| !l.is_empty())
        .unwrap_or_else(|| vec![r.task])
}

/// Whether a cause card is served on this task: it is failed, or waiting in landing.
pub fn serves(t: &Task) -> bool {
    matches!(t.state, State::Failed | State::Landing)
}

/// The open cause card that speaks for `t`, if any.
pub fn open_card_of(d: &Daemon, t: &Task) -> Option<DiagnosisRow> {
    if !serves(t) {
        return None;
    }
    pending_causes(d)
        .into_iter()
        .rev()
        .find(|(r, _)| tasks_of(r).contains(&t.id.0))
        .map(|(r, _)| r)
        .filter(|r| crate::diagnose::row_card(r).is_some())
}

/// Whether the automatic retry of a task waits: for a diagnosis that is running (until its
/// deadline), for a card that is open and does not recommend a retry, or for a task it was
/// told to wait for.
pub fn held(d: &Daemon, task: u64) -> bool {
    let sc = scope(d);
    let now = now_ms();
    if lock(&HELD)
        .iter()
        .any(|h| h.0 == sc && h.1 == task && h.2 > now)
    {
        return true;
    }
    if d.store
        .diagnosis_waits()
        .unwrap_or_default()
        .iter()
        .any(|(t, _)| *t == task)
    {
        return true;
    }
    pending_causes(d)
        .into_iter()
        .any(|(r, a)| tasks_of(&r).contains(&task) && crate::diagnose::recommended(&a).0 != "retry")
}

// ---- starting -------------------------------------------------------------------------------

/// What `start_cause` did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Started {
    Began,
    /// The task joined a diagnosis that is running or whose card is open.
    Joined(String),
}

fn failed_at(d: &Daemon, task: u64) -> Option<u64> {
    d.world()
        .ok()?
        .tasks
        .get(&TaskId(task))
        .map(|t| t.state_entered_at.0)
}

/// Start a diagnosis of the cause `key`, or join the task to the one that has it.
pub fn start_cause(
    d: &Arc<Daemon>,
    key: String,
    pid: ProjectId,
    tasks: Vec<u64>,
    run: StepRunId,
    trigger: Trigger,
) -> Result<Started, String> {
    let project = d.project(pid).ok_or("no such project")?.id.clone();
    let primary = *tasks.first().ok_or("no task")?;
    let sc = scope(d);
    let _gate = lock(&GATE);
    let now = now_ms();
    // Joins a session that is running.
    {
        let mut live = lock(&LIVE);
        if let Some(l) = live
            .iter_mut()
            .find(|l| l.scope == sc && (l.key == key || l.tasks.iter().any(|t| tasks.contains(t))))
        {
            for t in &tasks {
                if !l.tasks.contains(t) {
                    l.tasks.push(*t);
                }
                hold(&sc, *t, l.started);
            }
            return Ok(Started::Joined(format!(
                "a diagnosis of this cause is running (#{primary})"
            )));
        }
    }
    // Joins a card that is open.
    for (row, mut a) in pending_causes(d) {
        if a["cause_key"].as_str() == Some(key.as_str())
            || tasks_of(&row).iter().any(|t| tasks.contains(t))
        {
            let mut list = tasks_of(&row);
            for t in &tasks {
                if !list.contains(t) {
                    list.push(*t);
                }
            }
            a["tasks"] = json!(list);
            d.store
                .set_diagnosis_args(row.id, &a.to_string())
                .map_err(|e| e.to_string())?;
            // The card names its tasks, so it is written again (and its hash changes: an answer
            // to the card as it was is stale).
            if let Some(card) = crate::diagnose::build_card(row.id, row.task, &a) {
                let _ = d.store.set_diagnosis_card(row.id, &card.to_string());
            }
            d.poke();
            return Ok(Started::Joined(format!(
                "diagnosis #{} has its card open",
                row.id
            )));
        }
    }
    let first_failed = tasks
        .iter()
        .filter_map(|t| failed_at(d, *t))
        .min()
        .unwrap_or(0);
    let all = d.store.diagnoses(None).map_err(|e| e.to_string())?;
    let cause_rows: Vec<(String, u64, String)> = all
        .iter()
        .filter_map(|r| {
            let a: Value = serde_json::from_str(&r.args).ok()?;
            (a["kind"] == "cause").then(|| {
                (
                    a["cause_key"].as_str().unwrap_or("").to_string(),
                    r.at_ms,
                    a["project"].as_str().unwrap_or("").to_string(),
                )
            })
        })
        .collect();
    // A task that a cause diagnosis spoke for since this failure began is not diagnosed again
    // under another key (a landing failure and then the burst it is part of).
    let spoken_for = all.iter().any(|r| {
        is_cause_row(r) && r.at_ms >= first_failed && tasks_of(r).iter().any(|t| tasks.contains(t))
    });
    if spoken_for {
        return Err("a diagnosis has spoken for these tasks since they failed".into());
    }
    let keyed: Vec<(String, u64)> = cause_rows
        .iter()
        .map(|(k, at, _)| (k.clone(), *at))
        .collect();
    if covered(&keyed, &key, first_failed) {
        return Err("this cause has been diagnosed for this failure".into());
    }
    day_budget(
        cause_rows
            .iter()
            .filter(|(_, at, p)| *p == project && now.saturating_sub(*at) < DAY_MS)
            .count(),
    )?;
    // #1044's per-failure budget still applies to the task the session is for.
    let own = d
        .store
        .diagnoses(Some(primary))
        .map_err(|e| e.to_string())?;
    crate::diagnose::budget(&own, failed_at(d, primary).unwrap_or(0))?;
    let mark = crate::diagnose::take_mark(d, primary).ok_or("a diagnosis is already running")?;
    for t in &tasks {
        hold(&sc, *t, now);
    }
    lock(&LIVE).push(Live {
        scope: sc.clone(),
        key: key.clone(),
        tasks: tasks.clone(),
        started: now,
        ran_on: String::new(),
    });
    let guard = LiveGuard {
        scope: sc,
        key: key.clone(),
    };
    let d = d.clone();
    std::thread::Builder::new()
        .name(format!("diagnose-cause-{primary}"))
        .spawn(move || {
            let _mark = mark;
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_cause(&d, &key, pid, run, trigger, &guard)
            }));
            if r.is_err() {
                tracing::error!(key, "the cause diagnosis panicked");
            }
            // `guard` drops here: the holds go, the live mark goes.
        })
        .map(|_| Started::Began)
        .map_err(|e| e.to_string())
}

/// Whether a failure starts a cause diagnosis: any landing failure, and an outside failure that
/// came a second time with the same signature (`repeated`). A one-off outside failure outside
/// landing (a lost host, a fetch timeout) is only retried.
pub fn trigger_for(state: State, class: FailClass, repeated: bool) -> Option<Trigger> {
    if state == State::Landing {
        Some(Trigger::Landing)
    } else if class == FailClass::Outside && repeated {
        Some(Trigger::Repeat)
    } else {
        None
    }
}

/// The project and batch a `ci-stuck` alert key names (`landing:<project>:ci-stuck:b<batch>`).
pub fn parse_stuck_key(key: &str) -> Option<(u64, u64)> {
    let (head, batch) = key.rsplit_once(":ci-stuck:b")?;
    let pid = head.strip_prefix("landing:")?.parse().ok()?;
    Some((pid, batch.parse().ok()?))
}

/// A step of a task failed: a landing failure of any class starts a cause diagnosis, and so does
/// a second outside failure with the same signature. Anything else is left to the retry loop and
/// to #1044.
pub fn on_step_failed(d: &Arc<Daemon>, task: TaskId, run: StepRunId, state: State) {
    let found = {
        let Ok(w) = d.world() else { return };
        let Some(t) = w.tasks.get(&task) else { return };
        if t.state != State::Failed {
            return;
        }
        let Some(p) = d.project(t.project) else {
            return;
        };
        if !p.diagnose {
            return;
        }
        let failure = t.failure.clone().unwrap_or_default();
        let class = t
            .failure_class
            .unwrap_or_else(|| FailClass::classify(state, &failure));
        let sig = crate::health::signature(&failure);
        let Some(trigger) = trigger_for(state, class, repeats(d, task.0, &sig)) else {
            return;
        };
        (key_for_failure(&p.id, &failure), t.project, trigger)
    };
    let (key, pid, trigger) = found;
    if let Err(why) = start_cause(d, key, pid, vec![task.0], run, trigger) {
        tracing::debug!(task = task.0, "no cause diagnosis: {why}");
    }
}

/// Whether the daemon already retried this task for a failure with this signature, inside the
/// auto-retry window.
fn repeats(d: &Daemon, task: u64, sig: &str) -> bool {
    let since = now_ms().saturating_sub(d.cfg.auto_retry_window.as_millis() as u64);
    d.store
        .auto_retry_reasons_since(task, since)
        .unwrap_or_default()
        .iter()
        .any(|r| crate::health::signature(r) == sig)
}

/// One pass (the auto-retry loop calls it): start a diagnosis for a batch whose CI is stuck and
/// for a burst of failures, and keep the tasks told to wait for another.
pub fn pass(d: &Arc<Daemon>) {
    pass_waits(d);
    let Ok(w) = d.world() else { return };
    // A batch whose CI has not changed: the `ci-stuck` alert of landing.
    for (key, _kind, text) in d.landing.attention() {
        let Some((pid, batch)) = parse_stuck_key(&key) else {
            continue;
        };
        let pid = ProjectId(pid);
        let Some(p) = d.project(pid) else { continue };
        if !p.diagnose {
            continue;
        }
        let members: Vec<u64> = d
            .store
            .batches(Some(&p.id), 60)
            .unwrap_or_default()
            .into_iter()
            .find(|b| b.id == batch)
            .map(|b| {
                b.tasks
                    .split(',')
                    .filter_map(|x| x.trim().parse().ok())
                    .collect()
            })
            .unwrap_or_default();
        let waiting: Vec<u64> = members
            .into_iter()
            .filter(|t| {
                w.tasks
                    .get(&TaskId(*t))
                    .is_some_and(|t| t.state == State::Landing)
            })
            .collect();
        if waiting.is_empty() {
            continue;
        }
        let _ = text;
        let run = StepRunId(crate::http::latest_run(&w, TaskId(waiting[0])));
        let _ = start_cause(
            d,
            key_for_batch(&p.id, batch),
            pid,
            waiting,
            run,
            Trigger::Stuck { batch },
        );
    }
    // A burst of failures outside the code: once for the group.
    for b in crate::autoretry::current_bursts(d, &w) {
        let Some(first) = b.tasks.first().and_then(|t| w.tasks.get(&TaskId(*t))) else {
            continue;
        };
        let pid = first.project;
        let Some(p) = d.project(pid) else { continue };
        if !p.diagnose {
            continue;
        }
        let tasks: Vec<u64> = b
            .tasks
            .iter()
            .copied()
            .filter(|t| w.tasks.get(&TaskId(*t)).is_some_and(|t| t.project == pid))
            .collect();
        let run = StepRunId(crate::http::latest_run(&w, TaskId(tasks[0])));
        let _ = start_cause(
            d,
            key_for_burst(&p.id, &b.reason),
            pid,
            tasks,
            run,
            Trigger::Burst {
                reason: b.reason.clone(),
            },
        );
    }
}

/// Tasks told to wait for another task (`wait_for`): retried when it lands, released when it is
/// canceled, forgotten when they have moved on.
pub fn pass_waits(d: &Arc<Daemon>) {
    let Ok(w) = d.world() else { return };
    for (task, blocker) in d.store.diagnosis_waits().unwrap_or_default() {
        let Some(t) = w.tasks.get(&TaskId(task)) else {
            let _ = d.store.clear_diagnosis_wait(task);
            continue;
        };
        if t.state != State::Failed {
            let _ = d.store.clear_diagnosis_wait(task);
            continue;
        }
        match w.tasks.get(&TaskId(blocker)).map(|b| b.state) {
            Some(State::Landed) => {
                let _ = d.store.clear_diagnosis_wait(task);
                let latest = crate::http::latest_run(&w, TaskId(task));
                let ask = crate::http::RetryAsk {
                    seen: Some(latest),
                    forget_red: true,
                    ..Default::default()
                };
                match crate::http::retry_task(d, task, ask) {
                    Ok((_, step)) => {
                        tracing::info!(task, blocker, %step, "retried: the task it waited for landed")
                    }
                    Err(_) => tracing::info!(
                        task,
                        blocker,
                        "the task it waited for landed; the retry was refused"
                    ),
                }
            }
            Some(State::Canceled) | None => {
                let _ = d.store.clear_diagnosis_wait(task);
            }
            _ => {}
        }
    }
}

// ---- the session ----------------------------------------------------------------------------

fn run_cause(
    d: &Arc<Daemon>,
    key: &str,
    pid: ProjectId,
    run: StepRunId,
    trigger: Trigger,
    guard: &LiveGuard,
) {
    use mason_adapter_api::contract::{ContractHandle, NoChecks};
    use mason_adapter_api::StepKind;
    use mason_adapters::runner::{RunnerConfig, StepEnd, StepRunner};
    let tasks = guard.tasks();
    let Some(&primary) = tasks.first() else {
        return;
    };
    let Ok(c) = crate::jobs::ctx(d, TaskId(primary), run) else {
        return;
    };
    let so_far = d
        .store
        .diagnoses(Some(primary))
        .map_or(crate::diagnose::MAX_PER_TASK, |v| v.len());
    let batch = match &trigger {
        Trigger::Stuck { batch } => Some(*batch),
        _ => None,
    };
    let gathered = with_source(d, pid, |src| gather(d, pid, &tasks, batch, src));
    let facts = render(&gathered);
    let steps: Vec<String> = crate::jobs::task_step_names(d, TaskId(primary));
    let text = prompt(key, &trigger, &tasks, &facts, &steps);
    // A single task is looked at in its own worktree; several, in a read-only checkout at the base.
    let single_wt = (tasks.len() == 1 && c.wt.exists()).then(|| c.wt.clone());
    let checkout = single_wt
        .is_none()
        .then(|| make_checkout(d, primary, &c.project));
    let wt = single_wt
        .clone()
        .or(checkout.clone())
        .unwrap_or_else(|| c.wt.clone());
    let agent = c.project.agent();
    let eff = crate::tiermap::effective(&d.cfg.config_dir, agent.provider, &c.project.overlay_text);
    let (mut spec, ran_on) = crate::diagnose::session_spec(&text, &wt, &c.project, &eff);
    guard.ran_on(&ran_on);
    let adapter = crate::jobs::adapter_for(d, &c.project, &wt);
    let runner = StepRunner::new(adapter.as_ref(), RunnerConfig::default());
    let contract = ContractHandle::new(StepKind::Diagnose, Box::new(NoChecks));
    let sock = format!("t{primary}r{}cause{so_far}", run.0);
    if contract
        .serve(&d.cfg.sockets_dir().join(format!("{sock}.sock")))
        .is_err()
    {
        remove_checkout(&c.project, checkout.as_deref());
        return;
    }
    spec.env = c.project.agent_env.clone();
    spec.env.retain(|(k, _)| k != "MASON_TASK");
    spec.env.push(("MASON_TASK".into(), primary.to_string()));
    spec.tool_timeout = std::time::Duration::from_secs(120);
    spec.deadline = Some(std::time::Duration::from_secs(SESSION_DEADLINE_S));
    spec.run_key = Some(sock);
    let Ok(slot) = crate::jobs::acquire_llm(&c) else {
        contract.shutdown();
        remove_checkout(&c.project, checkout.as_deref());
        return;
    };
    let report = runner.run(spec, &contract);
    drop(slot);
    contract.shutdown();
    remove_checkout(&c.project, checkout.as_deref());
    let tasks = guard.tasks();
    let sees = same_names(&gathered);
    let StepEnd::Accepted { submission, .. } = report.end else {
        tracing::info!(key, "the cause diagnosis ended without a verdict");
        // Counted all the same; the automatic retries go when `guard` drops.
        let failed_at = failed_at(d, primary).unwrap_or(0);
        let args = json!({"kind": "cause", "cause_key": key, "project": c.project.id,
                          "tasks": tasks, "ran_on": ran_on});
        let _ = d.store.add_diagnosis(
            primary,
            "none",
            &args.to_string(),
            "none",
            now_ms(),
            failed_at,
        );
        return;
    };
    let mut a = submission;
    if let Some(o) = a.as_object_mut() {
        o.insert("kind".into(), json!("cause"));
        o.insert("cause_key".into(), json!(key));
        o.insert("project".into(), json!(c.project.id));
        o.insert("tasks".into(), json!(tasks));
        o.insert("ran_on".into(), json!(ran_on));
        o.insert("trigger".into(), json!(trigger.text()));
        o.insert("same_on_base".into(), json!(sees));
        o.insert(
            "facts".into(),
            json!(facts.chars().take(30_000).collect::<String>()),
        );
    }
    record_cause(d, TaskId(primary), a);
}

fn same_names(g: &Gathered) -> Vec<String> {
    g.ci.as_ref()
        .map(|c| c.same_on_base.clone())
        .unwrap_or_default()
}

/// A detached worktree at the project's base for a session about several tasks.
fn make_checkout(d: &Daemon, task: u64, p: &crate::config::ProjectCfg) -> std::path::PathBuf {
    let dir = d
        .cfg
        .data_dir
        .join("diag")
        .join(format!("{}-{task}-{}", p.id, now_ms()));
    let _ = std::fs::create_dir_all(dir.parent().unwrap_or(&dir));
    let ok = mason_landing::git::ok(
        &p.path,
        &[
            "worktree",
            "add",
            "--detach",
            &dir.to_string_lossy(),
            &p.base_ref(),
        ],
    );
    if ok {
        dir
    } else {
        p.path.clone()
    }
}

fn remove_checkout(p: &crate::config::ProjectCfg, dir: Option<&std::path::Path>) {
    let Some(dir) = dir else { return };
    if dir == p.path {
        return;
    }
    let _ = mason_landing::git::run(
        &p.path,
        &["worktree", "remove", "--force", &dir.to_string_lossy()],
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// Store a submitted cause diagnosis (`a` has `kind`, `cause_key` and `tasks`) and apply its
/// recommended option or leave the card pending. Only a lone `retry` is applied by itself.
pub fn record_cause(d: &Arc<Daemon>, primary: TaskId, mut a: Value) {
    let (verdict, rec_args) = crate::diagnose::recommended(&a);
    let now = now_ms();
    let tasks: Vec<u64> = a["tasks"]
        .as_array()
        .map(|l| l.iter().filter_map(Value::as_u64).collect())
        .filter(|l: &Vec<u64>| !l.is_empty())
        .unwrap_or_else(|| vec![primary.0]);
    // Land now would be refused on the card for a task whose own suite is red and not red on the
    // base: say so on the card, so the owner does not pick it.
    let same: Vec<String> = a["same_on_base"]
        .as_array()
        .map(|l| {
            l.iter()
                .filter_map(|s| s.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let mut refused = vec![];
    if has_action(&a, "land_now") {
        for t in &tasks {
            if let Err(e) = land_now_check(d, *t, &same) {
                refused.push(format!("#{t}: {e}"));
            }
        }
    }
    if !refused.is_empty() {
        let mut lines = a["evidence_lines"].as_array().cloned().unwrap_or_default();
        lines.push(json!(refused.join("; ")));
        a["evidence_lines"] = json!(lines);
    }
    let Ok(w) = d.world() else { return };
    let Some(t) = w.tasks.get(&primary) else {
        return;
    };
    let failed_at = t.state_entered_at.0;
    let recent = d
        .store
        .diagnoses(Some(primary.0))
        .unwrap_or_default()
        .iter()
        .any(|r| {
            r.status == "applied"
                && now.saturating_sub(r.decided_ms) < crate::diagnose::AUTO_WINDOW_MS
        });
    let action = crate::diagnose::decide(crate::diagnose::Facts {
        is_self: d.project(t.project).is_some_and(|p| p.self_cfg.is_some()),
        class: t.failure_class.unwrap_or(FailClass::Code),
        verdict: &verdict,
        no_code_change: rec_args["no_code_change"].as_bool().unwrap_or(false),
        applied_recently: recent,
        auto_retry_pending: crate::autoretry::pending(d, t),
    });
    drop(w);
    let status = if action == crate::diagnose::Action::Apply {
        "applied"
    } else {
        "pending"
    };
    let Ok(id) = d
        .store
        .add_diagnosis(primary.0, &verdict, &a.to_string(), status, now, failed_at)
    else {
        return;
    };
    if let Some(card) = crate::diagnose::build_card(id, primary.0, &a) {
        let _ = d.store.set_diagnosis_card(id, &card.to_string());
    }
    if action == crate::diagnose::Action::Apply {
        let _ = d.store.set_diagnosis_status(id, "applied", now);
        let row = d
            .store
            .diagnoses(Some(primary.0))
            .unwrap_or_default()
            .into_iter()
            .find(|r| r.id == id);
        if let Some(row) = row {
            let mut args = rec_args.clone();
            args["diagnosis"] = json!(id);
            match apply_all(d, &row, &tasks, &verdict, &args, true) {
                Ok(what) => tracing::info!("cause diagnosis #{id} applied by itself: {what}"),
                Err(e) => {
                    tracing::warn!("the cause diagnosis could not be applied: {e}");
                    let _ = d.store.set_diagnosis_status(id, "pending", 0);
                }
            }
        }
    }
    d.poke();
}

/// Whether any option of the card holds this action.
fn has_action(a: &Value, name: &str) -> bool {
    a["options"].as_array().into_iter().flatten().any(|o| {
        o["action"].as_str() == Some(name)
            || o["actions"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|x| x["action"].as_str() == Some(name))
    })
}

/// The suites that are red for a task by its own failure.
fn own_red(d: &Daemon, task: u64) -> Vec<String> {
    d.world()
        .ok()
        .and_then(|w| w.tasks.get(&TaskId(task)).and_then(|t| t.failure.clone()))
        .and_then(|f| crate::jobs::red_suite_of(&f))
        .into_iter()
        .collect()
}

/// Whether Land now may be chosen for the task, given the names that are the same on the base.
pub fn land_now_check(d: &Daemon, task: u64, same: &[String]) -> Result<(), String> {
    let mut same = same.to_vec();
    if let Ok(Some(b)) = d.store.base_red(task) {
        same.push(b.suite);
    }
    land_now_ok(&own_red(d, task), &same)
}

// ---- applying -------------------------------------------------------------------------------

/// The action objects of a stored option action: a compound's list, else the action itself.
fn actions_of(verdict: &str, args: &Value) -> Vec<Value> {
    if verdict == "compound" {
        return args["actions"].as_array().cloned().unwrap_or_default();
    }
    let mut a = args.clone();
    a["action"] = json!(verdict);
    vec![a]
}

/// Do what a cause card says for every task it lists. `file_task` and `open_session` happen once;
/// the others for each task. Land now is checked for every task before anything is done, so a
/// refusal changes nothing.
pub fn apply_all(
    d: &Arc<Daemon>,
    row: &DiagnosisRow,
    tasks: &[u64],
    verdict: &str,
    args: &Value,
    auto: bool,
) -> Result<String, String> {
    let acts = actions_of(verdict, args);
    if acts.is_empty() {
        return Err("this diagnosis has nothing to apply: decide yourself, or dismiss it".into());
    }
    let a: Value = serde_json::from_str(&row.args).unwrap_or(json!({}));
    let same: Vec<String> = a["same_on_base"]
        .as_array()
        .map(|l| {
            l.iter()
                .filter_map(|s| s.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    if acts.iter().any(|x| x["action"] == "land_now") {
        for t in tasks {
            land_now_check(d, *t, &same).map_err(|e| format!("#{t}: {e}"))?;
        }
    }
    let primary = tasks.first().copied().unwrap_or(row.task);
    let mut said: Vec<String> = vec![];
    for act in &acts {
        let name = act["action"].as_str().unwrap_or("none").to_string();
        let mut act = act.clone();
        act["diagnosis"] = json!(row.id);
        if matches!(name.as_str(), "file_task" | "open_session") {
            said.push(crate::diagnose::apply_args(d, primary, &name, &act, auto)?);
            continue;
        }
        let mut oks = vec![];
        let mut errs = vec![];
        for t in tasks {
            match crate::diagnose::apply_args(d, *t, &name, &act, auto) {
                Ok(s) => oks.push(format!("#{t}: {s}")),
                Err(e) => errs.push(format!("#{t}: {e}")),
            }
        }
        if oks.is_empty() {
            return Err(errs.join("; "));
        }
        said.push(format!("{name}: {}", oks.join("; ")));
        if !errs.is_empty() {
            said.push(format!("not done: {}", errs.join("; ")));
        }
    }
    Ok(said.join(" | "))
}

fn s_of(a: &Value, k: &str) -> Option<String> {
    a[k].as_str()
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .map(String::from)
}

/// `file_task`: through `proposals::file`, with its dedupe, caps and evidence rule.
pub fn file_task(d: &Arc<Daemon>, task: u64, a: &Value) -> Result<String, String> {
    let name = s_of(a, "project").ok_or("file_task needs a project")?;
    let pid = d
        .project_id_by_name(&name)
        .ok_or_else(|| format!("no project {name:?}"))?;
    let diag = a["diagnosis"].as_u64();
    let evidence = s_of(a, "evidence").unwrap_or_else(|| {
        // The facts the option cites: the card's own evidence lines.
        diag.and_then(|id| {
            d.store
                .diagnoses(Some(task))
                .ok()?
                .into_iter()
                .find(|r| r.id == id)
        })
        .and_then(|r| serde_json::from_str::<Value>(&r.card).ok())
        .map(|c| {
            c["evidence"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|l| l.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
    });
    let f = crate::proposals::Filing {
        from_task: Some(TaskId(task)),
        project: pid,
        title: s_of(a, "title").unwrap_or_default(),
        body: s_of(a, "body").unwrap_or_default(),
        evidence,
        blocks_task: false,
        explored: false,
        estimate: None,
        lookback: None,
    };
    let filed =
        crate::proposals::file(d, &f).map_err(|why| format!("the ticket was refused: {why}"))?;
    for blocker in a["after"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_u64)
    {
        let _ = d.emit(EventKind::BlockerAdded {
            task: filed.task,
            blocker: TaskId(blocker),
        });
    }
    Ok(format!(
        "filed #{} in {}{}",
        filed.task.0,
        filed.project,
        if filed.proposed { " (proposed)" } else { "" }
    ))
}

/// `land_now`: the owner's Land now with its debt. A failed task is then retried so the mark
/// takes effect.
pub fn land_now(d: &Arc<Daemon>, task: u64, a: &Value) -> Result<String, String> {
    let who = d.cfg.owner_names.first().cloned().unwrap_or_default();
    let reason = s_of(a, "reason").unwrap_or_else(|| "chosen on a diagnosis card".into());
    let (debt, in_batch) =
        crate::http::mark_land_override(d, task, "land-now", &who, &reason).map_err(|(_, e)| e)?;
    let failed = d
        .world()
        .ok()
        .and_then(|w| w.tasks.get(&TaskId(task)).map(|t| t.state == State::Failed))
        .unwrap_or(false);
    let mut out = match debt {
        Some(n) => format!("Land now recorded, debt #{n}"),
        None if in_batch => "Land now marked on the running batch".to_string(),
        None => "Land now recorded".to_string(),
    };
    if failed {
        let latest = d
            .world()
            .map(|w| crate::http::latest_run(&w, TaskId(task)))
            .unwrap_or(0);
        crate::http::retry_task(
            d,
            task,
            crate::http::RetryAsk {
                seen: Some(latest),
                forget_red: true,
                message: Some("Land now was chosen on a diagnosis card.".into()),
                ..Default::default()
            },
        )
        .map_err(|(_, e)| e.0["error"].as_str().unwrap_or("refused").to_string())?;
        out.push_str(", retried from the failed step");
    }
    Ok(out)
}

/// `wait_for`: the task waits behind another open task; its automatic retry is held, and it is
/// retried when that task lands.
pub fn wait_for(d: &Arc<Daemon>, task: u64, a: &Value) -> Result<String, String> {
    let blocker = a["task"].as_u64().ok_or("wait_for needs a task")?;
    let w = d.world().map_err(|e| e.to_string())?;
    let me = w.tasks.get(&TaskId(task)).ok_or("no such task")?;
    if me.state != State::Failed {
        return Err("only a failed task can wait for another".into());
    }
    let other = w
        .tasks
        .get(&TaskId(blocker))
        .ok_or_else(|| format!("no task #{blocker}"))?;
    if matches!(other.state, State::Landed | State::Canceled | State::Failed) || blocker == task {
        return Err(format!("#{blocker} is not an open task to wait for"));
    }
    drop(w);
    d.store
        .add_diagnosis_wait(
            task,
            blocker,
            a["diagnosis"].as_u64().unwrap_or(0),
            now_ms(),
        )
        .map_err(|e| e.to_string())?;
    d.poke();
    Ok(format!("waits for #{blocker} to land"))
}

/// `open_session`: an owner session about the task, seeded with the diagnosis and its facts. In
/// the task's worktree (`task`), or after a fresh worktree of the project at its base on
/// `claude/diag-<id>` has been made for it (`base`). It pushes and lands nothing.
pub fn open_session(d: &Arc<Daemon>, task: u64, a: &Value) -> Result<String, String> {
    let id = a["diagnosis"].as_u64().unwrap_or(0);
    let who = d.cfg.owner_names.first().cloned().unwrap_or_default();
    let brief = s_of(a, "brief").unwrap_or_default();
    let facts = d
        .store
        .diagnoses(None)
        .ok()
        .and_then(|rows| rows.into_iter().find(|r| r.id == id))
        .and_then(|r| serde_json::from_str::<Value>(&r.args).ok())
        .and_then(|v| v["facts"].as_str().map(String::from))
        .unwrap_or_default();
    let mut seed = format!("Diagnosis #{id}: {brief}");
    let mut place = "the task's worktree".to_string();
    if a["where"].as_str() == Some("base") {
        let (p, wt) = {
            let w = d.world().map_err(|e| e.to_string())?;
            let t = w.tasks.get(&TaskId(task)).ok_or("no such task")?;
            let p = d.project(t.project).ok_or("no such project")?.clone();
            let wt = d.cfg.data_dir.join("diag-wt").join(format!("diag-{id}"));
            (p, wt)
        };
        let _ = std::fs::create_dir_all(wt.parent().unwrap_or(&wt));
        let branch = format!("claude/diag-{id}");
        mason_landing::git::run(
            &p.path,
            &[
                "worktree",
                "add",
                "-b",
                &branch,
                &wt.to_string_lossy(),
                &p.base_ref(),
            ],
        )
        .map_err(|e| format!("the worktree could not be made: {e}"))?;
        seed.push_str(&format!(
            "\n\nA fresh worktree of the project at its base is at {} on branch {branch}. Nothing is pushed or landed from it by itself.",
            wt.display()
        ));
        place = format!("{} on {branch}", wt.display());
    }
    if !facts.is_empty() {
        seed.push_str(&format!("\n\n{facts}"));
    }
    let opened =
        crate::usersession::open_for_task(d, TaskId(task), &who, true).map_err(|e| match e {
            crate::usersession::OpenError::StepRunning => "the task's step is running".to_string(),
            crate::usersession::OpenError::Other(e) => e,
        })?;
    let _ = d
        .store
        .add_owner_message(opened.session.id, "tool", &seed, now_ms());
    d.poke();
    Ok(format!(
        "opened owner session #{} in {place}",
        opened.session.id
    ))
}

// ---- reading --------------------------------------------------------------------------------

/// A cause row's one-line notice, or `None` for any other row.
pub fn row_notice(r: &DiagnosisRow) -> Option<String> {
    let a: Value = serde_json::from_str(&r.args).ok()?;
    if a["kind"] != "cause" {
        return None;
    }
    let (_, rec) = crate::diagnose::recommended(&a);
    let label = a["options"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|o| o["recommended"].as_bool() == Some(true))
        .and_then(|o| o["label"].as_str())
        .unwrap_or_else(|| rec["label"].as_str().unwrap_or(""));
    Some(notice_line(
        &tasks_of(r),
        a["summary"].as_str().unwrap_or(""),
        label,
    ))
}

/// The alerts for pending cause cards: kind `diagnosis`, text the one line. A card stays while
/// any task it lists is still failed or waiting in landing.
pub fn alerts(d: &Daemon) -> Vec<(String, String, String)> {
    let Ok(w) = d.world() else { return vec![] };
    pending_causes(d)
        .into_iter()
        .filter(|(r, _)| {
            tasks_of(r)
                .iter()
                .any(|t| w.tasks.get(&TaskId(*t)).is_some_and(serves))
        })
        .filter_map(|(r, _)| {
            row_notice(&r)
                .map(|line| (format!("diagnosis:{}", r.id), "diagnosis".to_string(), line))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cause_is_keyed_by_project_and_signature_or_by_batch() {
        let a = key_for_failure("trd", "landing failed: CI: still not finished after 7200 s");
        let b = key_for_failure("trd", "landing failed: CI: still not finished after 3600 s");
        assert_eq!(a, b);
        assert_ne!(
            a,
            key_for_failure(
                "star",
                "landing failed: CI: still not finished after 7200 s"
            )
        );
        assert_eq!(key_for_batch("trd", 9), "batch:trd:9");
    }

    #[test]
    fn four_cause_diagnoses_a_day_per_project_and_no_fifth() {
        assert!(day_budget(3).is_ok());
        assert!(day_budget(4).is_err());
    }

    #[test]
    fn a_cause_already_diagnosed_for_this_failure_is_not_diagnosed_again() {
        let rows = vec![("k".to_string(), 100)];
        assert!(covered(&rows, "k", 90));
        assert!(
            !covered(&rows, "k", 110),
            "a newer failure is a new diagnosis"
        );
        assert!(!covered(&rows, "other", 0));
    }

    #[test]
    fn land_now_needs_every_own_suite_green_or_the_same_on_the_base() {
        assert!(land_now_ok(&[], &[]).is_ok());
        assert!(land_now_ok(&["unit".into()], &["unit".into()]).is_ok());
        let e = land_now_ok(&["unit".into(), "e2e".into()], &["unit".into()]).unwrap_err();
        assert!(e.contains("`e2e`") && !e.contains("`unit`"), "{e}");
    }

    #[test]
    fn the_notice_is_the_task_the_cause_in_twelve_words_and_the_recommendation() {
        let s = "The trd windows job hangs in tests/scan.rs on develop too since the last landing change. More text.";
        let line = notice_line(&[1003], s, "File the fix and land now.");
        assert_eq!(
            line,
            "#1003: The trd windows job hangs in tests/scan.rs on develop too since the. Recommended: File the fix and land now."
        );
        assert!(notice_line(&[1003, 1004, 1005], s, "x").starts_with("#1003 and 2 more: "));
        assert_eq!(
            cause_words("a b c d e f g h i j k l m n", 12)
                .split(' ')
                .count(),
            12
        );
    }

    #[test]
    fn a_stuck_alert_key_names_its_project_and_batch() {
        assert_eq!(parse_stuck_key("landing:3:ci-stuck:b17"), Some((3, 17)));
        assert_eq!(parse_stuck_key("landing:3:other:b17"), None);
        assert_eq!(parse_stuck_key("ci-stuck:b17"), None);
    }

    #[test]
    fn a_landing_failure_of_any_class_and_a_repeated_outside_failure_are_diagnosed() {
        use FailClass::*;
        assert_eq!(
            trigger_for(State::Landing, Code, false),
            Some(Trigger::Landing)
        );
        assert_eq!(
            trigger_for(State::Landing, Outside, false),
            Some(Trigger::Landing)
        );
        assert_eq!(
            trigger_for(State::Developing, Outside, true),
            Some(Trigger::Repeat)
        );
        // A one-off lost host outside landing is only retried.
        assert_eq!(trigger_for(State::Developing, Outside, false), None);
        assert_eq!(trigger_for(State::Developing, Code, true), None);
    }

    fn base(job: &str, run: u64, sha: &str, state: &str) -> BaseFact {
        BaseFact {
            workflow: "v3-gate".into(),
            job: job.into(),
            run,
            sha: sha.into(),
            state: state.into(),
            url: String::new(),
        }
    }

    #[test]
    fn a_job_is_the_same_on_the_base_when_its_newest_base_run_is_red_timed_out_or_unfinished() {
        let runs = vec![
            base("a", 3, "c3", "timed_out"),
            base("a", 2, "c2", "green"),
            base("b", 3, "c3", "green"),
            base("c", 3, "c3", "running"),
            base("d", 3, "c3", "cancelled"),
        ];
        assert!(same_on_base(&runs, "a"));
        assert!(!same_on_base(&runs, "b"));
        assert!(same_on_base(&runs, "c"));
        assert!(!same_on_base(&runs, "d"), "superseded is not the same");
        assert!(!same_on_base(&runs, "missing"));
        assert_eq!(last_green(&runs, "a").as_deref(), Some("c2"));
        assert_eq!(last_green(&runs, "c"), None);
    }

    #[test]
    fn suspects_are_the_merges_of_tasks_named_in_the_log() {
        let log = "aaaaaaaaaaaaaaaa\tMerge branch 'mason/trd/t1000'\nbbbbbbbbbbbbbbbb\tfix typo\ncccccccccccccccc\tMerge branch 'mason/other/t7'";
        let s = suspects(log, "trd");
        assert_eq!(s.len(), 3);
        assert_eq!(s[0].task, Some(1000));
        assert_eq!(s[1].task, None);
        assert_eq!(s[2].task, None);
    }

    #[test]
    fn open_tasks_that_name_the_job_are_pointed_at() {
        let tasks = vec![
            (
                1104,
                "Fix the scan tests".to_string(),
                "trd (windows-2022) hangs".to_string(),
            ),
            (9, "Unrelated".to_string(), "nothing".to_string()),
        ];
        let r = related(&tasks, &["trd (windows-2022)".into(), "x".into()], &[]);
        assert_eq!(r, vec![(1104, "Fix the scan tests".to_string())]);
        assert!(related(&tasks, &["trd (windows-2022)".into()], &[1104]).is_empty());
    }

    #[test]
    fn the_rendered_facts_mark_the_job_the_base_has_too() {
        let g = Gathered {
            batch: Some(7),
            ci: Some(CiFacts {
                ci_ref: "mason/ci/trd-b7-1".into(),
                jobs: vec![JobFact {
                    workflow: "v3-gate".into(),
                    name: "trd (windows-2022)".into(),
                    state: "running".into(),
                    log: "test scan_works ... running".into(),
                    annotations: String::new(),
                    url: String::new(),
                }],
                same_on_base: vec!["trd (windows-2022)".into()],
                ..Default::default()
            }),
            suspects: vec![Suspect {
                sha: "abc".into(),
                subject: "Merge branch 'mason/trd/t1000'".into(),
                task: Some(1000),
            }],
            ..Default::default()
        };
        let text = render(&g);
        assert!(text.contains("[same on the base]"));
        assert!(text.contains("test scan_works ... running"));
        assert!(text.contains("(task #1000)"));
    }
}
