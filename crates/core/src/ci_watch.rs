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
//! - **Passed over.** A run that ended otherwise (cancelled by a newer push, ...) and a job
//!   `skipped` inside a finished run say nothing about the code.
//! - **Infrastructure is not a regression.** `cancelled`, `timed_out` and `startup_failure`
//!   are never filed.
//! - **The gate acts at once.** A gate job going red after green is filed (urgent) on that
//!   one run, and holds the freeze until the job is green again.
//! - **Other jobs need agreement.** The first red after green is pending; `agreement`
//!   distinct runs red with no green between file it. A green clears it as a flake.
//! - **Long reds.** A job red for `red_ticket_after` with no open ticket naming it is
//!   filed, and looked at again only after another threshold.
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobRec {
    pub id: u64,
    pub name: String,
    /// `None`: not finished.
    pub conclusion: Option<String>,
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
    pub jobs: Vec<JobRec>,
}

/// Where a red was seen.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct RunRef {
    pub run: u64,
    pub job: u64,
    pub sha: String,
    pub url: String,
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
    /// The task filed (or found open) for it.
    pub ticket: Option<u64>,
    /// When it was last filed or looked at for the long-red rule.
    pub checked: Option<u64>,
    /// The gate went red and the freeze holds until it is green.
    pub freeze: bool,
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
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct File {
    pub workflow: String,
    pub job: String,
    pub urgent: bool,
    /// Past the cap: filed `proposed`.
    pub proposed: bool,
    pub why: Why,
    pub at: RunRef,
}

impl File {
    pub fn key(&self) -> String {
        key(&self.workflow, &self.job)
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
    }
}

/// Take in the runs seen on the base branch. Returns what to file, in order. The state
/// is updated in place; the caller stores it only after acting. Runs whose jobs were not
/// read (empty `jobs`) are not counted as seen, so they are tried again.
pub fn step(state: &mut State, runs: &[RunRec], ctx: &Ctx) -> Vec<File> {
    let rules = ctx.rules;
    let mut out: Vec<File> = vec![];
    let mut open_auto = ctx.open_auto;
    let mut decided: BTreeSet<String> = BTreeSet::new();

    let mut file = |state: &mut State, out: &mut Vec<File>, mut f: File| {
        let k = f.key();
        let js = state
            .workflows
            .get_mut(&f.workflow)
            .and_then(|w| w.jobs.get_mut(&f.job));
        let Some(js) = js else { return };
        js.checked = Some(ctx.now_ms);
        if f.urgent && rules.freeze_on_gate {
            js.freeze = true;
        }
        if let Some(t) = ctx.named.get(&k) {
            // An open task or issue already names it: no second ticket.
            if js.ticket.is_none() {
                js.ticket = *t;
            }
            return;
        }
        if !decided.insert(k) {
            return;
        }
        if open_auto >= rules.max_open_auto {
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
                        ws.jobs.insert(j.name.clone(), JobState::default());
                    }
                    Some("failure") => {
                        ws.jobs.insert(
                            j.name.clone(),
                            JobState {
                                red: true,
                                known: true,
                                red_since: ctx.now_ms,
                                last_red: job_run_ref(r, j),
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
                match j.conclusion.as_deref() {
                    Some("success") => {
                        let js = ws.jobs.entry(j.name.clone()).or_default();
                        *js = JobState::default();
                    }
                    Some("failure") => {
                        let js = ws.jobs.entry(j.name.clone()).or_default();
                        let at = job_run_ref(r, j);
                        js.last_red = at.clone();
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
                                        why: Why::Gate,
                                        at,
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
                                    why: Why::Agreed,
                                    at,
                                },
                            );
                        }
                    }
                    // skipped, neutral, cancelled, timed_out, startup_failure,
                    // action_required, stale: nothing about the code.
                    _ => {}
                }
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
            if js.red && ctx.now_ms.saturating_sub(from) >= rules.red_ticket_after_ms {
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
                why: Why::LongRed,
                at,
            },
        );
    }

    // A job that went green clears its freeze with the rest of its state (above), and a
    // job that is red and checked is not looked at again until another threshold.
    out
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
    if let Some(js) = state
        .workflows
        .get_mut(&f.workflow)
        .and_then(|w| w.jobs.get_mut(&f.job))
    {
        js.ticket = task;
    }
    state.filed.push(Filed {
        key: f.key(),
        task,
        at: now_ms,
        urgent: f.urgent,
        proposed: f.proposed,
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
            jobs: jobs
                .iter()
                .enumerate()
                .map(|(i, (n, c))| JobRec {
                    id: id * 10 + i as u64,
                    name: (*n).into(),
                    conclusion: Some((*c).into()),
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
        for (i, c) in ["cancelled", "timed_out", "startup_failure"]
            .iter()
            .enumerate()
        {
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
        // Closed and still red: not again until another threshold.
        assert!(go(&mut s, &[], 4 * h, &none(), 0).is_empty());
        assert_eq!(go(&mut s, &[], 6 * h + 3000, &none(), 0).len(), 1);
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
}
