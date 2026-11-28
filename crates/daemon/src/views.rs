// SPDX-License-Identifier: GPL-3.0-or-later

//! Server-computed views (Part 16). The daemon publishes ready-made
//! projections so two clients stay affordable: they draw what they are given
//! and send commands, and never re-derive pipeline rules. Every task carries
//! its `allowed_actions`, its "why waiting" text and its progress.

use crate::daemon::{now_ms, Daemon};
use mason_adapter_api::card;
use mason_core::*;
use mason_engine::World;
use mason_protocol::*;
use mason_store::state_name;

/// The sections of the sidebar, in order. Needs-you comes first and is not a
/// state of its own: it is `needs_attention`.
const ORDER: [State; 11] = [
    State::NeedsAttention,
    State::Queued,
    State::Exploring,
    State::Developing,
    State::Testing,
    State::Review,
    State::Landing,
    State::Proposed,
    State::Deferred,
    State::Failed,
    State::Landed,
];

/// Failed, landed and canceled start collapsed. (Queued is in queue order;
/// every other section has the task that entered it last on top.)
fn is_history(s: State) -> bool {
    matches!(s, State::Failed | State::Landed | State::Canceled)
}

fn title_of(s: State) -> &'static str {
    match s {
        State::NeedsAttention => "Needs you",
        State::Queued => "Queued",
        State::Exploring => "Exploring",
        State::Developing => "Developing",
        State::Testing => "Testing",
        State::Review => "Review",
        State::Landing => "Landing",
        State::Proposed => "Proposed",
        State::Deferred => "Deferred",
        State::Failed => "Failed",
        State::Canceled => "Canceled",
        State::Landed => "Landed",
    }
}

/// The text on a count tile: the state name with spaces, except that
/// needs_attention reads "stuck". The sidebar title is separate (`title_of`).
fn tile_label_of(s: State) -> String {
    match s {
        State::NeedsAttention => "stuck".into(),
        _ => state_name(s).replace('_', " "),
    }
}

fn tooltip_of(s: State) -> &'static str {
    match s {
        State::NeedsAttention => {
            "A task has asked you a question, or waits for your approval of a change."
        }
        State::Queued => "Waiting for a free session or for its turn.",
        State::Exploring => "An agent is reading the repository to plan the work.",
        State::Developing => "An agent is making the change.",
        State::Testing => "Suites are proving the change.",
        State::Review => "A reviewer is looking at the change.",
        State::Landing => "The change is being merged.",
        State::Proposed => "Filed by an agent or a tracker, waiting for your approval.",
        State::Deferred => "Put aside by you. It comes back only when you approve, reject or move it back to Proposed.",
        State::Failed => "Stopped, and needs a decision: retry or cancel.",
        State::Landed => "Merged.",
        State::Canceled => "Stopped on purpose.",
    }
}

fn wants_you(s: State) -> bool {
    matches!(s, State::NeedsAttention | State::Failed | State::Proposed)
}

pub struct Ctx<'a> {
    pub d: &'a Daemon,
    pub w: &'a World,
    pub now: u64,
    /// Task to the tracker issue it was imported from.
    issues: std::collections::HashMap<u64, u64>,
    /// The open owner sessions: by task for the rows, and all of them for the header (s364).
    owner_sessions: Vec<mason_store::owner_sessions::OwnerSessionRow>,
    /// Built once: every step run by task, and the typical span per step.
    runs: RunIndex,
    /// Task to the tasks it blocks, in id order.
    blocks: std::collections::HashMap<TaskId, Vec<u64>>,
    /// The broker's wait reasons, taken once per build.
    waits: std::sync::Arc<std::collections::BTreeMap<TaskId, String>>,
    /// Whether history rows keep their body (default); the sidebar search asks for them.
    pub row_bodies: bool,
    /// The same waits as data, with the queue-held summary (s527).
    wait_view: std::sync::Arc<crate::waitview::WaitsView>,
    /// Tasks the owner made immune to testing (an open `tests:all` debt), read once per build.
    immune: std::collections::HashSet<u64>,
    /// Tasks with their own placement mode (`run_on`), read once per build (#834).
    run_on: std::collections::HashMap<u64, String>,
    /// Developing tasks whose last check failed only tests that are red on develop (#1023).
    red_held: std::collections::BTreeMap<u64, crate::redwatch::Held>,
}

type LogCache = std::collections::HashMap<u64, (Vec<crate::http::LogStamp>, String)>;

/// Most rows a history section (landed, canceled, failed...) sends. `count`
/// stays the true total; a task past the cap still opens by id.
pub const HISTORY_ROW_CAP: usize = 500;

/// Step runs indexed once per view, so a row costs a lookup and not a scan.
#[derive(Default)]
pub struct RunIndex {
    /// A task's runs, sorted by run id.
    by_task: std::collections::HashMap<TaskId, Vec<StepRun>>,
    /// The latest start among a task's runs.
    last_start: std::collections::HashMap<TaskId, u64>,
    /// Typical (low, high) per step name; absent with too few samples.
    typical: std::collections::HashMap<String, (u64, u64)>,
    /// Runs looked at while building: one visit per run, twice at most.
    pub visits: usize,
}

impl RunIndex {
    pub fn build<'r>(runs: impl Iterator<Item = &'r StepRun>) -> RunIndex {
        let mut ix = RunIndex::default();
        for r in runs {
            ix.visits += 1;
            ix.by_task.entry(r.task).or_default().push(r.clone());
        }
        let mut spans: std::collections::HashMap<&str, Vec<u64>> = Default::default();
        for (task, rs) in ix.by_task.iter_mut() {
            rs.sort_by_key(|r| r.id);
            ix.visits += rs.len();
            ix.last_start
                .insert(*task, rs.iter().map(|r| r.started_at.0).max().unwrap_or(0));
            // A run ends when the task's next one starts: the earliest start
            // among the runs after it, taken as a suffix minimum.
            let mut next: Option<u64> = None;
            for r in rs.iter().rev() {
                if r.outcome == Some(StepOutcome::Succeeded) && !r.name.is_empty() {
                    if let Some(end) = next {
                        spans
                            .entry(r.name.as_str())
                            .or_default()
                            .push(end.saturating_sub(r.started_at.0));
                    }
                }
                next = Some(next.map_or(r.started_at.0, |n| n.min(r.started_at.0)));
            }
        }
        for (name, v) in spans {
            if let Some(t) = mason_core::progress::middle_half(&v) {
                ix.typical.insert(name.to_string(), t);
            }
        }
        ix
    }

    /// The upper end of the typical time for a step name, when there are enough runs.
    pub fn typical_hi(&self, step: &str) -> Option<u64> {
        self.typical.get(step).map(|(_, hi)| *hi)
    }

    /// When the task's latest step run started (0: none).
    pub fn last_start_of(&self, t: TaskId) -> u64 {
        self.last_start.get(&t).copied().unwrap_or(0)
    }

    fn of_task(&self, t: TaskId) -> &[StepRun] {
        self.by_task.get(&t).map_or(&[], |v| v.as_slice())
    }
}

/// The public page of an issue, when the tracker is GitHub's own.
fn issue_page(d: &Daemon, pid: ProjectId, n: u64) -> String {
    d.project(pid)
        .and_then(|p| p.services.tracker.as_ref())
        .filter(|t| t.api.trim_end_matches('/') == "https://api.github.com")
        .map_or_else(String::new, |t| {
            format!("https://github.com/{}/issues/{n}", t.repo)
        })
}

impl<'a> Ctx<'a> {
    pub fn new(d: &'a Daemon, w: &'a World) -> Ctx<'a> {
        let mut issues = std::collections::HashMap::new();
        for p in d.projects().values() {
            for r in d.store.tracker_items(&p.id).unwrap_or_default() {
                issues.insert(r.task, r.number);
            }
        }
        let mut blocks: std::collections::HashMap<TaskId, Vec<u64>> = Default::default();
        for o in w.tasks.values() {
            for b in &o.blockers {
                blocks.entry(*b).or_default().push(o.id.0);
            }
        }
        let now = now_ms();
        Ctx {
            d,
            w,
            now,
            red_held: crate::redwatch::held(d, w, now),
            issues,
            owner_sessions: d.store.open_owner_sessions().unwrap_or_default(),
            runs: RunIndex::build(w.step_runs.values()),
            blocks,
            waits: d.broker.waits_snapshot(),
            row_bodies: true,
            wait_view: d.broker.wait_view(),
            run_on: d
                .store
                .task_overrides_for_key(mason_definition::layers::RUN_ON)
                .unwrap_or_default()
                .into_iter()
                .filter_map(|(t, v)| {
                    crate::place::Mode::parse(&v).map(|m| (t, m.name().to_string()))
                })
                .collect(),
            immune: d
                .store
                .debts(None, true)
                .unwrap_or_default()
                .into_iter()
                .filter(|x| x.scope == crate::suites::ALL_TESTS)
                .map(|x| x.task)
                .collect(),
        }
    }

    /// A proposed task imported from the tracker: it has its own section.
    fn is_import(&self, t: &Task) -> bool {
        t.state == State::Proposed && self.issues.contains_key(&t.id.0)
    }

    fn project_name(&self, t: &Task) -> String {
        self.w
            .projects
            .get(&t.project)
            .map_or_else(|| "?".into(), |p| p.name.clone())
    }

    fn def_of(&self, t: &Task) -> std::sync::Arc<mason_definition::Definition> {
        self.d
            .pinned_def(&t.def)
            .filter(|_| !t.def.is_empty())
            .unwrap_or_else(|| self.d.current_def(t.project))
    }

    /// What can be done to a task right now. The one list: commands outside
    /// it are refused, and every client draws exactly these buttons.
    pub fn allowed_actions(&self, t: &Task) -> Vec<String> {
        let mut a: Vec<&str> = match t.state {
            State::Proposed => vec!["approve", "defer", "reject", "edit"],
            State::Deferred => vec!["approve", "reject", "undefer"],
            State::Queued => vec!["cancel"],
            State::Exploring | State::Developing | State::Review => {
                vec!["cancel", "retry", "open_session"]
            }
            State::Testing => vec![
                "cancel",
                "retry",
                "skip_test",
                "land_now",
                "force",
                "open_session",
            ],
            State::Landing => {
                let mut a = vec!["cancel", "skip_test", "land_now", "force"];
                // A cause's card for a batch that is stuck (s1176).
                if crate::diagcause::open_card_of(self.d, t).is_some() {
                    a.insert(0, "answer");
                }
                a
            }
            State::NeedsAttention => vec!["answer", "cancel", "retry", "open_session"],
            State::Failed => self.failed_actions(t),
            State::Canceled if t.cancel_keeps_branch => vec!["revive"],
            State::Canceled | State::Landed => vec![],
        };
        if matches!(t.state, State::Proposed | State::Queued) {
            a.push("block");
            if !t.blockers.is_empty() {
                a.push("unblock");
            }
        }
        let waiting_approval =
            t.activity == Activity::Waiting(WaitReason::On(APPROVAL_WAIT.into()));
        if (waiting_approval || self.pending_hold(t).is_some()) && !a.contains(&"approve") {
            a.extend(["approve", "reject"]);
        }
        if t.state.is_step() && t.route.is_some_and(Route::skipped_exploring) {
            a.push("explore_anyway");
        }
        // Immunity to testing (a fast track: tests skipped from develop's checks to landing)
        // can be set once, at any point from approval until the task lands.
        let open = !matches!(
            t.state,
            State::Proposed | State::Deferred | State::Canceled | State::Landed
        );
        if open && !self.immune.contains(&t.id.0) {
            a.push("skip_all_tests");
        }
        a.into_iter().map(String::from).collect()
    }

    /// A failed task's actions: `answer` first when it holds a diagnosis card, and `diagnose`
    /// when one may be asked for (none pending or running, and the budget allows).
    fn failed_actions(&self, t: &Task) -> Vec<&'static str> {
        let rows = self.d.store.diagnoses(Some(t.id.0)).unwrap_or_default();
        let pending = crate::diagnose::pending_for(self.d, t);
        let mut a = vec![];
        if pending
            .as_ref()
            .and_then(crate::diagnose::row_card)
            .is_some()
        {
            a.push("answer");
        }
        a.extend(["retry", "cancel", "open_session"]);
        if pending.is_none()
            && crate::diagnose::running(self.d, t.id.0).is_none()
            && crate::diagnose::budget(&rows, t.state_entered_at.0).is_ok()
        {
            a.push("diagnose");
        }
        a
    }

    /// What a failed task's row says about its diagnosis: "running", "ready" (it holds a card) or
    /// "" (none, or not a failed task).
    fn diagnosis_state(&self, t: &Task) -> String {
        if t.state != State::Failed {
            return String::new();
        }
        if crate::diagnose::running(self.d, t.id.0).is_some() {
            return "running".into();
        }
        let rows = self.d.store.diagnoses(Some(t.id.0)).unwrap_or_default();
        match crate::diagnose::pending_in(&rows, t).and_then(crate::diagnose::row_card) {
            Some(_) => "ready".into(),
            None => String::new(),
        }
    }

    /// A self-hosting hold nobody has approved yet, for a task still going.
    fn pending_hold(&self, t: &Task) -> Option<mason_store::HoldRow> {
        if !t.state.is_step() {
            return None;
        }
        self.d
            .store
            .hold(t.id.0)
            .ok()
            .flatten()
            .filter(|h| h.approved_by.is_none())
    }

    /// Does the task wait for the owner's decision on something other than a question?
    fn needs_approval(&self, t: &Task) -> bool {
        t.state == State::Proposed
            || t.activity == Activity::Waiting(WaitReason::On(APPROVAL_WAIT.into()))
            || self.pending_hold(t).is_some()
    }

    /// Is the task in the "Needs you" section: it asked a question, or it waits for the
    /// owner's approval of a held change. (Proposed and failed tasks have their own
    /// sections, which are marked as wanting you.) A held task is in a step state, so
    /// without this it counted in the header but never showed in the section or the tile.
    fn needs_you(&self, t: &Task) -> bool {
        t.state == State::NeedsAttention
            || (t.state != State::Proposed && self.needs_approval(t))
            || self.circling(t)
    }

    /// Is the develop step going round in circles on the same failing tests (s511)?
    fn circling(&self, t: &Task) -> bool {
        t.state == State::Developing && crate::circling::of_task(self.d, t.id).is_some()
    }

    /// Does the task belong in sidebar section `s`? Each task is in exactly one.
    fn in_section(&self, t: &Task, s: State) -> bool {
        if s == State::NeedsAttention {
            self.needs_you(t)
        } else {
            t.state == s && !self.needs_you(t)
        }
    }

    /// What the task is waiting on for approval, in full.
    fn approval(&self, t: &Task) -> Option<Approval> {
        let p = self.d.project(t.project);
        let branch = p.map_or_else(String::new, |p| p.branch(t.id.0));
        let diff_command = p.map_or_else(String::new, |p| {
            format!(
                "git -C {} diff {}...{branch}",
                p.path.display(),
                p.base_ref()
            )
        });
        if let Some(h) = self.pending_hold(t) {
            return Some(Approval {
                kind: "hold".into(),
                waiting_for: "Your approval of a change to MASON's own rules".into(),
                lines: h
                    .reasons
                    .split("; ")
                    .filter(|l| !l.is_empty())
                    .map(String::from)
                    .collect(),
                diff_hash: h.diff_hash,
                branch,
                diff_command,
            });
        }
        if t.activity == Activity::Waiting(WaitReason::On(APPROVAL_WAIT.into())) {
            return Some(Approval {
                kind: "approval".into(),
                waiting_for: format!("Your approval at the {} step", t.step),
                lines: vec![],
                diff_hash: String::new(),
                branch,
                diff_command,
            });
        }
        if t.state == State::Proposed {
            return Some(Approval {
                kind: "proposed".into(),
                waiting_for: "Your approval to start this proposed task".into(),
                lines: vec![],
                diff_hash: String::new(),
                branch: String::new(),
                diff_command: String::new(),
            });
        }
        None
    }

    /// A developing task whose last check failed only tests that are red on develop waits in
    /// the class `base-red`, as a task does at the test step: not "developing" (#1023).
    fn red_wait(&self, t: &Task) -> Option<mason_protocol::WaitInfo> {
        let h = self.red_held.get(&t.id.0)?;
        Some(mason_protocol::WaitInfo {
            cause: "base-red".into(),
            resource: "base-red".into(),
            sentence: crate::redwatch::held_sentence(h),
            ..Default::default()
        })
    }

    fn why_waiting(&self, t: &Task) -> String {
        // The sentence comes with the structured wait, from the same plan.
        if let Some(i) = self.wait_view.tasks.get(&t.id.0) {
            return crate::nodewait::line_for(self.d, i, self.now)
                .unwrap_or_else(|| i.sentence.clone());
        }
        if let Some(w) = self.waits.get(&t.id) {
            return w.clone();
        }
        if let Some(i) = self.red_wait(t) {
            return i.sentence;
        }
        if t.state == State::Proposed && !t.blockers.is_empty() {
            return format!(
                "Proposed, held behind {}",
                t.blockers
                    .iter()
                    .map(|b| format!("#{}", b.0))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        match &t.activity {
            Activity::Waiting(WaitReason::On(w)) => w.clone(),
            Activity::Waiting(WaitReason::Slot) => self
                .d
                .queued_wait_text(self.w, t)
                .unwrap_or_else(|| "Waiting for an LLM session to be free".into()),
            Activity::Waiting(WaitReason::NotBefore(_)) => "Waiting for its start time".into(),
            Activity::Waiting(WaitReason::Owner) => "Waiting for your answer".into(),
            Activity::Held(_) if crate::blockers::held_line(self.w, t).is_some() => {
                crate::blockers::held_line(self.w, t).unwrap_or_default()
            }
            Activity::Held(by) => format!(
                "Held, blocked by {}",
                by.iter()
                    .map(|b| format!("#{}", b.0))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Activity::Paused => "Paused".into(),
            _ => String::new(),
        }
    }

    /// When the task entered its present state.
    fn state_since(&self, t: &Task) -> u64 {
        self.runs
            .last_start
            .get(&t.id)
            .map_or(t.enqueued_at.0, |s| (*s).max(t.enqueued_at.0))
    }

    /// Typical time in a step, from this project's own history: only with
    /// enough samples to say (five), as `(low, high)` ms (the middle half).
    fn typical(&self, step: &str) -> Option<(u64, u64)> {
        self.runs.typical.get(step).copied()
    }

    fn progress(&self, t: &Task) -> Vec<ProgressCell> {
        let def = self.def_of(t);
        let live = self.d.progress_of_task(t.id);
        let runs = self.runs.of_task(t.id);
        def.steps
            .iter()
            .map(|s| {
                let done = runs
                    .iter()
                    .any(|r| r.name == s.name && r.outcome == Some(StepOutcome::Succeeded));
                let current = t.state.is_step() && t.step == s.name;
                let (state, label) = if current {
                    match &t.activity {
                        Activity::Waiting(_) | Activity::Held(_) => {
                            (CellState::Hatched, self.why_waiting(t))
                        }
                        _ => (CellState::Part, "running".to_string()),
                    }
                } else if done {
                    (CellState::Filled, String::new())
                } else {
                    (CellState::Empty, String::new())
                };
                // A running step carries its live record: what it has counted, the
                // output so far and when it last moved.
                if current && state == CellState::Part {
                    if let Some(p) = live.iter().rev().find(|p| p.step == s.name) {
                        return crate::progress::cell_of(p, state, &label);
                    }
                }
                ProgressCell {
                    step: s.name.clone(),
                    state: state as i32,
                    label,
                    ..Default::default()
                }
            })
            .collect()
    }

    pub fn row(&self, t: &Task) -> TaskRow {
        let since = self.state_since(t);
        let in_state = self.now.saturating_sub(since);
        let typical = self.typical(&t.step);
        let model = if t.state.is_step() {
            let def = self.def_of(t);
            def.step(&t.step)
                .filter(|s| s.kind == mason_definition::StepType::Agent)
                .map(|s| {
                    s.model
                        .clone()
                        .or_else(|| s.agent.clone())
                        .unwrap_or_else(|| {
                            mason_definition::builtin::default_profile(s.category).to_string()
                        })
                })
                .unwrap_or_default()
        } else {
            String::new()
        };
        // What the step really launched (or, before it starts, what it is expected
        // to use). The old `model` above stays as it was for older clients.
        let model_use = crate::launchview::task_use(self.d, self.w, t);
        let live = self.develop_record(t);
        TaskRow {
            id: t.id.0,
            key: format!("#{}", t.id.0),
            title: t.title.clone(),
            project: self.project_name(t),
            state: state_name(t.state).to_string(),
            activity: self
                .d
                .queued_wait_text(self.w, t)
                .unwrap_or_else(|| crate::http::activity_text(&t.activity)),
            why_waiting: self.why_waiting(t),
            wait: self
                .wait_view
                .tasks
                .get(&t.id.0)
                .cloned()
                .map(|mut i| {
                    // A wait for a node's unit says why, who holds it and the estimate (#1260).
                    if let Some(l) = crate::nodewait::line_for(self.d, &i, self.now) {
                        i.sentence = l;
                    }
                    i
                })
                .or_else(|| self.red_wait(t)),
            enqueued_ms: t.enqueued_at.0,
            // Whole minutes, so a sidebar that changed in nothing else hashes the same.
            in_state_ms: in_state / 60_000 * 60_000,
            state_since_ms: since,
            run_on_override: self.run_on.get(&t.id.0).cloned().unwrap_or_default(),
            diagnosis: self.diagnosis_state(t),
            model,
            display: model_use
                .as_ref()
                .map_or_else(String::new, |m| m.display.clone()),
            model_use,
            progress: self.progress(t),
            allowed_actions: self.allowed_actions(t),
            test_immune: self.immune.contains(&t.id.0),
            blockers: t.blockers.iter().map(|b| b.0).collect(),
            last_run: crate::http::latest_run(self.w, t.id),
            check: live
                .as_ref()
                .map_or_else(String::new, |p| p.check_short(self.now)),
            doing: live
                .as_ref()
                .map_or_else(String::new, |p| p.doing_text(self.now)),
            owner_session: self
                .owner_sessions
                .iter()
                .find(|s| s.task == Some(t.id.0))
                .map(|s| OwnerSessionInfo {
                    id: s.id,
                    opened_ms: s.opened_ms,
                    last_activity_ms: s.last_ms,
                    turns: s.turns,
                    working: s.state == "working",
                    kind: s.kind.clone(),
                }),
            blocker_links: crate::blockers::links(self.d, self.w, t)
                .into_iter()
                .map(|l| BlockerLink {
                    task: l.task.0,
                    state: state_name(l.state).to_string(),
                    origin: l.origin.to_string(),
                    actions: l.actions.iter().map(|s| s.to_string()).collect(),
                })
                .collect(),
            blocks: self.blocks.get(&t.id).cloned().unwrap_or_default(),
            typical: typical.map_or_else(String::new, |(lo, hi)| {
                format!("usually {} to {} min", lo / 60_000, hi.div_ceil(60_000))
            }),
            // Past the typical time is amber, never red: slow is not broken.
            slow: typical.is_some_and(|(_, hi)| t.state.is_step() && in_state > hi),
            step: t.step.clone(),
            body: t.body.chars().take(2000).collect(),
            proposal: self
                .d
                .store
                .proposal(t.id.0)
                .ok()
                .flatten()
                .map_or_else(String::new, |r| crate::proposals::tag(&r)),
            issue: self.issues.get(&t.id.0).copied().unwrap_or(0),
            issue_url: self
                .issues
                .get(&t.id.0)
                .map_or_else(String::new, |n| issue_page(self.d, t.project, *n)),
        }
    }

    /// The proposal behind a task, if it is one, and where its improvement branch is.
    fn proposal_info(&self, t: &Task) -> Option<ProposalInfo> {
        let r = self.d.store.proposal(t.id.0).ok().flatten()?;
        let channel = crate::improve::switched(self.d)
            .filter(|s| s["task"].as_u64() == Some(r.task))
            .and_then(|s| s["branch"].as_str().map(|b| format!("branch:{b}")))
            .unwrap_or_default();
        Some(ProposalInfo {
            source: r.source,
            severity: r.severity,
            route: r.route,
            evidence: r.evidence,
            text: r.body,
            filed_by: r.filed_by.unwrap_or(0),
            status: r.status,
            reject_reason: r.reject_reason,
            publish_state: r.publish_state,
            access: r.access,
            branch: r.branch,
            pr: r.pr.unwrap_or(0),
            pr_state: r.pr_state,
            channel,
        })
    }

    fn matches(&self, t: &Task, project: &str) -> bool {
        project.is_empty() || self.project_name(t) == project
    }

    // ---- the views ----

    pub fn header(&self, project: &str) -> HeaderView {
        let tasks: Vec<&Task> = self
            .w
            .tasks
            .values()
            .filter(|t| self.matches(t, project))
            .collect();
        let count = |s: State| tasks.iter().filter(|t| self.in_section(t, s)).count() as u32;
        let attention = tasks
            .iter()
            .filter(|t| {
                matches!(
                    t.state,
                    State::NeedsAttention | State::Failed | State::Proposed
                ) || self.needs_approval(t)
                    || self.circling(t)
            })
            .count() as u32;
        let mut banners = vec![];
        for p in self.d.store.pauses(self.now).unwrap_or_default() {
            // A hold for a refused push has its own banner, with the action that fixes it.
            if p.owner == crate::ssh_keys::PAUSE_OWNER {
                continue;
            }
            banners.push(Banner {
                kind: "paused".into(),
                text: format!(
                    "Paused ({}) for {} by {}",
                    p.mode,
                    if p.project.is_empty() {
                        "everything"
                    } else {
                        &p.project
                    },
                    p.owner
                ),
                project: p.project.clone(),
                mode: p.mode.clone(),
                ..Default::default()
            });
        }
        // A pause that has just gone says who lifted it, for ten minutes.
        if let Some(l) = crate::http::last_lift(self.d, self.now) {
            let text = format!(
                "Pause lifted {} by {} via {}",
                crate::update::clock_at(l.at),
                l.who,
                l.via
            );
            banners.push(Banner {
                kind: "pause_lifted".into(),
                detail: format!("{text}: {}", l.what),
                text,
                ..Default::default()
            });
        }
        for (_, _, text) in crate::ssh_keys::alerts(self.d) {
            banners.push(Banner {
                kind: "push_auth".into(),
                text,
                ..Default::default()
            });
        }
        for (pid, p) in &self.d.projects() {
            if self.d.definition_error(*pid).is_some() {
                banners.push(Banner {
                    kind: "definition".into(),
                    text: format!(
                        "{}: the definition is refused; the last good one is running",
                        p.id
                    ),
                    ..Default::default()
                });
            }
        }
        // The base-branch CI watch: what is red and waiting for agreement, or why it cannot look.
        for (pid, p) in &self.d.projects() {
            if !(project.is_empty() || p.id == project) {
                continue;
            }
            let v = crate::ci_watch::view(self.d, *pid);
            if v["enabled"] != true {
                continue;
            }
            banners.extend(ci_watch_banners(&p.id, &v));
        }
        for f in self.d.store.freezes().unwrap_or_default() {
            if project.is_empty() || f.project == project {
                banners.push(Banner {
                    kind: "landing_frozen".into(),
                    text: format!("Landing is frozen for {}", f.project),
                    detail: if f.source == "ci" {
                        format!(
                            "Landing is frozen for {}: only the fix task lands until that job is green on the base branch ({})",
                            f.project, f.reason
                        )
                    } else {
                        format!(
                            "Landing is frozen for {}: only the fix task lands until the post-land suites are green ({})",
                            f.project, f.reason
                        )
                    },
                    ..Default::default()
                });
            }
        }
        for (kind, text) in crate::holds::banners(self.d) {
            banners.push(Banner {
                kind,
                text,
                ..Default::default()
            });
        }
        if let Some((session, text)) = crate::health::banner(self.d, &self.owner_sessions) {
            banners.push(Banner {
                kind: "health".into(),
                text,
                session,
                ..Default::default()
            });
        }
        for b in &mut banners {
            if b.level.is_empty() {
                b.level = banner_level(&b.kind).into();
            }
            one_line(b);
        }
        HeaderView {
            project: project.to_string(),
            project_placements: self
                .d
                .projects()
                .values()
                .map(|p| ProjectPlacement {
                    project: p.id.clone(),
                    placement: Some(crate::place::project_placement(self.d, &p.id)),
                })
                .collect(),
            attention,
            counts: ORDER
                .iter()
                .copied()
                .chain([State::Canceled])
                .map(|s| StateCount {
                    state: state_name(s).into(),
                    count: count(s),
                    wants_you: wants_you(s) && count(s) > 0,
                    tooltip: tooltip_of(s).into(),
                    label: tile_label_of(s),
                })
                .collect(),
            banners,
            update: crate::update::info(self.d),
            open_owner_sessions: self.owner_sessions.len() as u32,
            active_sessions: self
                .owner_sessions
                .iter()
                .map(|s| ActiveSession {
                    id: s.id,
                    task: s.task.unwrap_or(0),
                    kind: s.kind.clone(),
                    label: match s.task {
                        Some(t) => self
                            .w
                            .tasks
                            .get(&TaskId(t))
                            .map_or_else(|| format!("#{t}"), |x| format!("#{t} {}", x.title)),
                        None => "Pipeline health".to_string(),
                    },
                    working: s.state == "working",
                    waiting: s.state == "open" && (s.turns > 0 || s.kind == "health"),
                })
                .collect(),
            projects: self.w.projects.values().map(|p| p.name.clone()).collect(),
            pending_issues: self
                .d
                .projects()
                .iter()
                .filter(|(_, p)| project.is_empty() || p.id == project)
                .flat_map(|(pid, p)| {
                    self.d
                        .tracker
                        .pending(*pid)
                        .into_iter()
                        .map(move |w| PendingIssue {
                            project: p.id.clone(),
                            number: w.number,
                            title: w.title,
                            reason: w.reason,
                            url: issue_page(self.d, *pid, w.number),
                        })
                })
                .collect(),
            status: if attention == 0 {
                "All quiet".into()
            } else {
                format!(
                    "{attention} need{} you",
                    if attention == 1 { "s" } else { "" }
                )
            },
            self_project: crate::proposals::self_project(self.d)
                .map_or_else(String::new, |(_, p)| p.id.clone()),
        }
    }

    fn section(&self, s: State, project: &str) -> SidebarSection {
        let tasks: Vec<&Task> = self
            .w
            .tasks
            .values()
            .filter(|t| self.in_section(t, s) && self.matches(t, project))
            .filter(|t| !(s == State::Proposed && self.is_import(t)))
            .collect();
        self.section_of(s, tasks)
    }

    /// Proposed tasks the tracker imported: they wait for approval like any
    /// proposal, but come from an outside signal, so they are listed apart.
    fn imports_section(&self, project: &str) -> SidebarSection {
        let tasks: Vec<&Task> = self
            .w
            .tasks
            .values()
            .filter(|t| self.in_section(t, State::Proposed) && self.matches(t, project))
            .filter(|t| self.is_import(t))
            .collect();
        let mut sec = self.section_of(State::Proposed, tasks);
        sec.state = "proposed_imports".into();
        sec.title = "Proposed from the tracker".into();
        sec
    }

    fn section_of(&self, s: State, mut tasks: Vec<&Task>) -> SidebarSection {
        // Queued is the real queue: what the daemon would admit first is on
        // top (tasks that can go, then held/paused/not_before ones, each by
        // `queued_rank`). Every other section has the task that entered it
        // most recently on top, tasks that are not waiting above those that are.
        if s == State::Queued {
            sort_queued(&mut tasks, |t| self.d.queued_rank(t));
        } else {
            sort_unqueued(&mut tasks);
        }
        SidebarSection {
            state: state_name(s).into(),
            title: title_of(s).into(),
            count: tasks.len() as u32,
            collapsed: is_history(s) || s == State::Deferred,
            rows: tasks
                .iter()
                .take(if is_history(s) {
                    HISTORY_ROW_CAP
                } else {
                    usize::MAX
                })
                .map(|t| {
                    let mut r = self.row(t);
                    if !self.row_bodies && (is_history(s) || s == State::Deferred) {
                        r.body.clear();
                    }
                    r
                })
                .collect(),
        }
    }

    pub fn sidebar(&self, project: &str) -> SidebarView {
        let mut sections: Vec<SidebarSection> = vec![];
        for s in ORDER {
            sections.push(self.section(s, project));
            // Only while there are some, so a project with no tracker sees no new section.
            if s == State::Proposed {
                let imports = self.imports_section(project);
                if imports.count > 0 {
                    sections.push(imports);
                }
            }
        }
        sections.push(self.section(State::Canceled, project));
        SidebarView {
            project: project.to_string(),
            sections,
            pages: ["landing", "usage", "pipeline", "health"]
                .map(String::from)
                .to_vec(),
        }
    }

    fn card(&self, t: &Task) -> Option<DecisionCard> {
        let (c, hash) = crate::attention::card_of(self.d, t)?;
        Some(DecisionCard {
            ask: c.ask,
            why: c.why,
            options: c
                .options
                .into_iter()
                .map(|o| CardOption {
                    label: o.label,
                    recommended: o.recommended,
                })
                .collect(),
            evidence: c.evidence,
            hash,
            origin: c.origin.name().into(),
            multi_select: c.multi_select,
        })
    }

    /// The CPU shares, with each task's title filled in.
    pub fn cpu_usage(&self) -> CpuUsage {
        let mut u = self.d.cpu_reading().usage.clone();
        for s in u.shares.iter_mut().filter(|s| s.kind == "task") {
            if let Some(t) = self.w.tasks.get(&TaskId(s.task)) {
                s.title = t.title.clone();
                s.project = self.project_name(t);
            }
        }
        u
    }

    /// The broker's compilation with each holder's project and full title filled
    /// in from the store, looked up by task id (not from home's filtered list).
    fn compilation_named(&self) -> mason_protocol::Compilation {
        let mut c = self.d.broker.compilation();
        for h in c.running.iter_mut().filter(|h| h.task > 0) {
            if let Some(t) = self.w.tasks.get(&TaskId(h.task)) {
                h.project = self.project_name(t);
                h.title = t.title.clone();
            }
        }
        // The shared build directories' `deps` sizes (one stat each), so a bloated one shows.
        for p in self.d.projects().values() {
            if let Some(parent) = crate::taskcache::shared_parent(p) {
                for (name, bytes) in crate::taskcache::shared_deps_sizes(&parent) {
                    c.notes.push(crate::taskcache::deps_note(
                        &format!("{}/{name}", p.id),
                        bytes,
                    ));
                }
            }
        }
        c
    }

    /// The live record of a developing task's current step.
    fn develop_record(&self, t: &Task) -> Option<mason_core::progress::Progress> {
        if t.state != State::Developing {
            return None;
        }
        self.d
            .progress_of_task(t.id)
            .into_iter()
            .rev()
            .find(|p| p.step == t.step)
    }

    /// What most of the waiting work is held by: the scheduler's (an exclusive request, a gate),
    /// or a test that is red on the base for many tasks, whichever holds more of them.
    fn queue_hold(&self, project: &str) -> Option<mason_protocol::QueueHold> {
        let red = self.red_hold(project);
        match (self.wait_view.hold.clone(), red) {
            (Some(h), Some(r)) => Some(if r.waiting > h.waiting { r } else { h }),
            (h, r) => h.or(r),
        }
    }

    /// Tasks waiting because the base is red the same way: one line for all of them.
    fn red_hold(&self, project: &str) -> Option<mason_protocol::QueueHold> {
        let reds = self.d.store.base_reds().ok()?;
        let mut groups: std::collections::BTreeMap<(String, String), Vec<u64>> = Default::default();
        for r in reds {
            let Some(t) = self.w.tasks.get(&TaskId(r.task)) else {
                continue;
            };
            if t.state.is_terminal() || !self.matches(t, project) {
                continue;
            }
            groups
                .entry((r.suite, r.first_error))
                .or_default()
                .push(r.task);
        }
        let ((suite, error), tasks) = groups.into_iter().max_by_key(|(_, t)| t.len())?;
        if tasks.len() < 2 {
            return None;
        }
        let error: String = error.split_whitespace().collect::<Vec<_>>().join(" ");
        let error: String = error.chars().take(120).collect();
        let n = tasks.len();
        Some(mason_protocol::QueueHold {
            kind: "red".into(),
            cause: "red".into(),
            summary: format!(
                "Queue held: `{suite}` is red on develop ({error}); {n} tasks fail the same way."
            ),
            waiting: n as u32,
            tasks: tasks.into_iter().take(20).collect(),
            ..Default::default()
        })
    }

    /// What an agent is doing, for the home page: `working` (it holds its `llm` slot),
    /// `waiting_check` (its `run_check` is out, on GitHub or queued here) or `waiting_slot`
    /// (its step waits for an `llm` slot). `None` for a task that is no agent now.
    /// Returns (state, place, runs_on, run_url, resource).
    fn agent_state(
        &self,
        t: &Task,
        llm_holders: &std::collections::HashSet<TaskId>,
        holds: &[crate::remote::RemoteHold],
        runs: &[crate::running::Run],
    ) -> Option<(&'static str, String, String, String, String)> {
        if llm_holders.contains(&t.id) {
            return Some((
                "working",
                String::new(),
                String::new(),
                String::new(),
                String::new(),
            ));
        }
        if !t.state.is_step() {
            return None;
        }
        if let Some(h) = holds.iter().find(|h| h.task == t.id.0 && h.kind == "check") {
            let (url, runs_on) = crate::remote::record_view(&h.record);
            return Some((
                "waiting_check",
                "github".into(),
                runs_on,
                url,
                String::new(),
            ));
        }
        let node = runs.iter().find(|r| {
            r.task == Some(t.id.0)
                && r.kind == crate::running::Kind::Check
                && matches!(r.host, crate::running::Host::Node(_))
        });
        if let Some(r) = node {
            return Some((
                "waiting_check",
                r.host.key(),
                String::new(),
                String::new(),
                String::new(),
            ));
        }
        // A step that has not started its agent is queued, not an active model; one that
        // has started (it has a live record) and waits for a session again is `waiting_slot`.
        let live = self.develop_record(t)?;
        if live.check_open().is_some() {
            return Some((
                "waiting_check",
                "here".into(),
                String::new(),
                String::new(),
                String::new(),
            ));
        }
        let w = self.wait_view.tasks.get(&t.id.0)?;
        if w.resource.starts_with("llm") {
            return Some((
                "waiting_slot",
                String::new(),
                String::new(),
                String::new(),
                w.resource.clone(),
            ));
        }
        None
    }

    pub fn home(&self, project: &str) -> HomeView {
        let tasks: Vec<&Task> = self
            .w
            .tasks
            .values()
            .filter(|t| self.matches(t, project))
            .collect();
        let mut attention: Vec<&&Task> = tasks
            .iter()
            .filter(|t| t.state == State::NeedsAttention || self.needs_approval(t))
            .collect();
        attention.sort_by_key(|t| (t.enqueued_at, t.id));
        let (sessions_used, sessions_limit) = self.d.accounts.iter().fold((0, 0), |(u, l), a| {
            let r = crate::sched::llm_resource(&a.name);
            (
                u + self.d.broker.used(&r),
                l + self.d.broker.limit(&r).unwrap_or(0),
            )
        });
        let llm_holders = self.d.broker.llm_holders();
        let holds =
            crate::remote::running_holds(&crate::remote::holds_dir(&self.d.cfg.hosts_dir()));
        let runs = crate::running::runs(self.d, self.w);
        HomeView {
            hosts: self.hosts(project, &runs),
            project: project.to_string(),
            placement: Some(crate::place::global_placement(self.d)),
            project_placement: (!project.is_empty())
                .then(|| crate::place::project_placement(self.d, project)),
            status: self.header(project).status,
            attention: attention
                .iter()
                .map(|t| AttentionItem {
                    task: Some(self.row(t)),
                    card: self.card(t),
                    approval: self.approval(t),
                })
                .collect(),
            active: tasks
                .iter()
                .filter_map(|t| {
                    let (state, place, runs_on, run_url, resource) =
                        self.agent_state(t, &llm_holders, &holds, &runs)?;
                    let r = self.row(t);
                    // A held session is never hidden: with no model on the
                    // row, fall back to its display name, then the step.
                    let model = if !r.model.is_empty() {
                        r.model.clone()
                    } else if !r.display.is_empty() {
                        r.display.clone()
                    } else {
                        t.step.clone()
                    };
                    Some(ActiveModel {
                        model,
                        display: r.display,
                        model_use: r.model_use,
                        project: r.project,
                        task: t.id.0,
                        step: t.step.clone(),
                        state: state.into(),
                        place,
                        runs_on,
                        run_url,
                        resource,
                    })
                })
                .collect(),
            remote: remote_summary(self.d, &holds),
            runner_pools: runner_pools(self.d),
            developing: {
                let kinds: Vec<mason_core::checks::PhaseKind> = tasks
                    .iter()
                    .filter(|t| t.state == State::Developing)
                    .map(|t| {
                        self.develop_record(t)
                            .map_or(mason_core::checks::PhaseKind::Editing, |p| {
                                p.phase_kind(self.now)
                            })
                    })
                    .collect();
                mason_core::checks::phase_summary(&kinds)
            },
            sessions_used,
            sessions_limit,
            compilation: Some(self.compilation_named()),
            queue_hold: self.queue_hold(project),
            cpu: Some(self.cpu_usage()),
            github: Some(crate::remote_runs::view(
                self.d,
                &|id| {
                    if id == 0 {
                        return None;
                    }
                    let t = self.w.tasks.get(&TaskId(id))?;
                    if !self.matches(t, project) {
                        return None;
                    }
                    let r = self.row(t);
                    Some((r.title, r.state, r.activity, r.project))
                },
                project,
            )),
            stages: [
                State::Exploring,
                State::Developing,
                State::Testing,
                State::Review,
                State::Landing,
            ]
            .iter()
            .map(|s| self.section(*s, project))
            .filter(|s| s.count > 0)
            .collect(),
        }
    }

    /// Every configured host with the runs of `project` (empty: all) on it (#1155).
    fn hosts(&self, project: &str, runs: &[crate::running::Run]) -> Vec<HostView> {
        crate::running::arrange(crate::running::lines(self.d), runs, project)
            .into_iter()
            .map(|(line, runs)| HostView {
                kind: line.kind.into(),
                name: line.name,
                status: line.status,
                runs: runs.iter().map(|r| self.host_run(r)).collect(),
            })
            .collect()
    }

    fn host_run(&self, r: &crate::running::Run) -> HostRun {
        HostRun {
            task: r
                .task
                .and_then(|t| self.w.tasks.get(&TaskId(t)))
                .map(|t| self.row(t)),
            kind: r.kind.label().into(),
            name: r.name.clone(),
            host: r.host.key(),
            since_ms: r.since_ms,
            url: r.url.clone(),
        }
    }

    /// The runs of task `id` that are in flight, wherever they are (#1155).
    pub fn running_on(&self, id: u64) -> Vec<HostRun> {
        crate::running::runs(self.d, self.w)
            .iter()
            .filter(|r| r.task == Some(id))
            .map(|r| self.host_run(r))
            .collect()
    }

    pub fn task_detail(&self, id: u64) -> Option<TaskDetailView> {
        let t = self.w.tasks.get(&TaskId(id))?;
        let p = self.d.project(t.project);
        let context = crate::context::of_task(self.d, id);
        let mut steps: Vec<StepLine> = self
            .runs
            .of_task(t.id)
            .iter()
            .map(|r| StepLine {
                run: r.id.0,
                name: r.name.clone(),
                category: state_name(r.step).into(),
                attempt: r.attempt,
                outcome: r.outcome.map_or_else(
                    || "running".to_string(),
                    |o| format!("{o:?}").to_lowercase(),
                ),
                started_ms: r.started_at.0,
                launches: crate::launchview::history(self.d, r.id.0),
                context: context.get(&r.id.0).map_or_else(String::new, |f| f.line()),
            })
            .collect();
        steps.sort_by_key(|s| s.run);
        let tail = log_tail(self.d, id);
        let plan =
            std::fs::read_to_string(self.d.cfg.task_dir(id).join("plan.json")).unwrap_or_default();
        let plan_view = (!plan.is_empty()).then(|| crate::plan_view::parse_plan(&plan));
        let removes = crate::landing::removals_view(&self.d.cfg.task_dir(id));
        Some(TaskDetailView {
            running_on: self.running_on(id),
            diagnosis_running: (t.state == State::Failed)
                .then(|| crate::diagnose::running(self.d, id))
                .flatten()
                .map(|(at, on)| DiagnosisRunning {
                    started_ms: at,
                    ran_on: on,
                }),
            placement: Some(crate::place::task_placement(
                self.d,
                &self.project_name(t),
                id,
            )),
            step_tokens: crate::usage::step_tokens(self.d, id, self.now / 1000),
            ci_links: crate::landing::ci_links_of(&self.d.cfg.task_dir(id))
                .into_iter()
                .map(|l| CiLink {
                    label: l.label,
                    url: l.url,
                    kind: l.kind.into(),
                })
                .collect(),
            cpu_permille: self
                .d
                .cpu_reading()
                .by_task
                .get(&id)
                .map_or(-1, |p| *p as i32),
            row: Some(self.row(t)),
            steps,
            plan,
            plan_view,
            card: self.card(t),
            review_findings: crate::reviewfindings::lines(&self.d.cfg.task_dir(id)),
            closing: self
                .d
                .store
                .closing(id)
                .ok()
                .flatten()
                .map_or_else(String::new, |(_, note)| note),
            debts: self
                .d
                .store
                .debts(Some(id), false)
                .unwrap_or_default()
                .iter()
                .map(debt_line)
                .collect(),
            log_tail: tail,
            branch: p.map_or_else(String::new, |p| p.branch(id)),
            worktree: p.map_or_else(String::new, |p| {
                p.worktree(&self.d.cfg, id).display().to_string()
            }),
            definition: t.def.clone(),
            failure: t.failure.clone().unwrap_or_default(),
            body: t.body.clone(),
            route: match t.route {
                None => String::new(),
                Some(Route::Explore) => "explored first".into(),
                Some(Route::Flag) => "skipped exploring: filed as already explored".into(),
                Some(Route::Clarity) => "skipped exploring: the clarity check passed".into(),
                Some(Route::Classifier) => {
                    "skipped exploring: the project's classifier said it was clear".into()
                }
                Some(Route::ExploreAnyway) => "explored, at the owner's request".into(),
            } + &removes,
            started_at: self
                .runs
                .of_task(t.id)
                .first()
                .map(|r| r.name.clone())
                .unwrap_or_default(),
            explore_returns: t.explore_returns,
            approval: self.approval(t),
            proposal: self.proposal_info(t),
            numbers: self
                .d
                .number_lines(t)
                .into_iter()
                .map(|n| NumberLine {
                    kind: n.kind,
                    number: n.number,
                    released: n.released,
                    path: n.path,
                    source: n.source,
                })
                .collect(),
        })
    }

    pub fn landing(&self, project: &str) -> LandingView {
        let in_scope = |p: &str| project.is_empty() || p == project;
        let freezes: Vec<FreezeLine> = self
            .d
            .store
            .freezes()
            .unwrap_or_default()
            .iter()
            .filter(|f| in_scope(&f.project))
            .map(|f| FreezeLine {
                project: f.project.clone(),
                reason: f.reason.clone(),
                fix_task: f.fix_task.unwrap_or(0),
                since_ms: f.since,
            })
            .collect();
        let mut modes = vec![];
        let mut trackers = vec![];
        let mut idle = 0;
        let mut running_total = 0;
        for (pid, p) in &self.d.projects() {
            if !in_scope(&p.id) {
                continue;
            }
            trackers.push(crate::tracker::state_line(self.d, *pid));
            idle += self.d.landing().idle_ms(*pid);
            running_total += self.d.landing().running_batches(*pid);
            modes.push(match crate::landing::load_config(self.d, *pid) {
                Ok(c) => {
                    let mut s = format!("{}: {}", p.id, c.mode.name());
                    if !c.window.is_zero() {
                        s.push_str(&format!(", window {}s", c.window.as_secs()));
                    }
                    if c.max_size > 0 {
                        s.push_str(&format!(", up to {} per batch", c.max_size));
                    }
                    if c.parallel > 1 {
                        s.push_str(&format!(", {} batches at once", c.parallel));
                    }
                    s
                }
                Err(e) => e,
            });
        }
        let live: std::collections::BTreeSet<u64> = self
            .d
            .store
            .batches(None, 200)
            .unwrap_or_default()
            .iter()
            .filter(|b| b.finished_at.is_none())
            .map(|b| b.id)
            .collect();
        let ids = |s: &str| -> Vec<u64> { s.split(',').filter_map(|x| x.parse().ok()).collect() };
        let mut histories = std::collections::HashMap::new();
        LandingView {
            project: project.to_string(),
            debts: self
                .d
                .store
                .debts(None, true)
                .unwrap_or_default()
                .iter()
                .filter(|x| in_scope(&x.project))
                .map(debt_line)
                .collect(),
            frozen: !freezes.is_empty(),
            note: if !freezes.is_empty() {
                "Landing is frozen: only the fix task lands until the post-land suites are green."
                    .into()
            } else if running_total > 0 {
                format!("{running_total} batch(es) landing now.")
            } else {
                String::new()
            },
            queue: self
                .w
                .tasks
                .values()
                .filter(|t| t.state == State::Landing && self.matches(t, project))
                .map(|t| self.row(t))
                .collect(),
            items: self.landing_items(project),
            modes,
            trackers,
            batches: self
                .d
                .store
                .batches(None, 40)
                .unwrap_or_default()
                .iter()
                .filter(|b| in_scope(&b.project))
                .take(12)
                .map(|b| {
                    let info = self.batch_info(b, &mut histories);
                    BatchLine {
                        id: b.id,
                        project: b.project.clone(),
                        phase: b.phase.clone(),
                        tasks: ids(&b.tasks),
                        landed: ids(&b.landed),
                        parked: ids(&b.parked),
                        note: b.note.clone(),
                        running: live.contains(&b.id),
                        allowed_actions: batch_actions(&b.phase, live.contains(&b.id)),
                        ci: self
                            .d
                            .landing
                            .ci
                            .get(b.id)
                            .map(|c| crate::batch_ci::to_proto(&c)),
                        state_text: info.state.text,
                        severity: info.state.severity.into(),
                        severity_reason: info.state.reason,
                        phase_ms: info.phase_ms,
                        typical_ms: info.typical_ms.unwrap_or(0),
                        split_round: b.round as u32,
                        remaining: info.state.remaining as u32,
                        members: info.members,
                    }
                })
                .collect(),
            freezes,
            queue_idle_ms: idle,
        }
    }

    /// The batch as one thing (s788): its state in words, severity, time in the phase and the
    /// phase's usual time, and each member's standing. `histories` caches a project's recorded
    /// phase entries across the batches of one view.
    fn batch_info(
        &self,
        b: &mason_store::BatchRow,
        histories: &mut std::collections::HashMap<String, PhaseHistory>,
    ) -> BatchInfo {
        let ids = |s: &str| -> Vec<u64> { s.split(',').filter_map(|x| x.parse().ok()).collect() };
        let h = histories.entry(b.project.clone()).or_insert_with(|| {
            let rows = self
                .d
                .store
                .batch_phases(&b.project, 30)
                .unwrap_or_default();
            let finished = self
                .d
                .store
                .batches(Some(&b.project), 30)
                .unwrap_or_default()
                .iter()
                .filter_map(|x| x.finished_at.map(|f| (x.id, f)))
                .collect();
            PhaseHistory { rows, finished }
        });
        let entered = h
            .rows
            .iter()
            .rev()
            .find(|r| r.batch == b.id)
            .map_or(b.at, |r| r.at);
        let phase_ms = self.now.saturating_sub(entered);
        let typical_ms = typical_from(&phase_durations(&h.rows, &h.finished, b.id, &b.phase));
        let frozen = self
            .d
            .store
            .freezes()
            .unwrap_or_default()
            .iter()
            .any(|f| f.project == b.project);
        let (landed, parked) = (ids(&b.landed), ids(&b.parked));
        let all = ids(&b.tasks);
        let state = batch_state(&BatchFacts {
            phase: &b.phase,
            note: &b.note,
            round: b.round,
            total: all.len(),
            landed: &landed,
            parked: &parked,
            phase_ms,
            typical_ms,
            frozen,
        });
        let members = all
            .iter()
            .map(|t| BatchMember {
                task: *t,
                standing: member_standing(&b.phase, *t, &landed, &parked),
            })
            .collect();
        BatchInfo {
            state,
            phase_ms,
            typical_ms,
            members,
        }
    }

    /// Each landing task's status: in a running batch (phase, CI, time) or queued (position and
    /// why). Read through existing accessors; nothing is changed.
    fn landing_items(&self, project: &str) -> Vec<TaskLanding> {
        let batches = self.d.store.batches(None, 200).unwrap_or_default();
        let freezes = self.d.store.freezes().unwrap_or_default();
        let ids = |s: &str| -> Vec<u64> { s.split(',').filter_map(|x| x.parse().ok()).collect() };
        let mut out = vec![];
        for (pid, p) in &self.d.projects() {
            if !project.is_empty() && p.id != project {
                continue;
            }
            let live: Vec<_> = batches
                .iter()
                .filter(|b| b.project == p.id && b.finished_at.is_none())
                .collect();
            let frozen = freezes
                .iter()
                .find(|f| f.project == p.id)
                .map(|f| f.fix_task);
            let cfg = crate::landing::load_config(self.d, *pid).ok();
            let mut seen = std::collections::BTreeSet::new();
            for b in &live {
                let (index, phase) = batch_phase(&b.phase);
                let ci = self
                    .d
                    .landing
                    .ci
                    .get(b.id)
                    .map(|c| crate::batch_ci::to_proto(&c).state)
                    .unwrap_or_default();
                let done: Vec<u64> = ids(&b.landed).into_iter().chain(ids(&b.parked)).collect();
                for id in ids(&b.tasks).into_iter().filter(|i| !done.contains(i)) {
                    let Some(t) = self.w.tasks.get(&TaskId(id)) else {
                        continue;
                    };
                    seen.insert(id);
                    out.push(TaskLanding {
                        task: id,
                        key: format!("#{id}"),
                        title: t.title.clone(),
                        project: p.id.clone(),
                        status: "in_batch".into(),
                        batch: b.id,
                        phase: phase.into(),
                        phase_index: index,
                        phase_total: LANDING_PHASES,
                        phase_ms: self.now.saturating_sub(b.at),
                        standing: member_standing(&b.phase, id, &ids(&b.landed), &ids(&b.parked)),
                        ci_state: ci.clone(),
                        allowed_actions: batch_actions(&b.phase, true),
                        ..Default::default()
                    });
                }
            }
            let mut position = 0;
            for id in self.d.landing().queued(*pid) {
                let Some(t) = self.w.tasks.get(&id) else {
                    continue;
                };
                if !seen.insert(id.0) {
                    continue;
                }
                position += 1;
                out.push(TaskLanding {
                    task: id.0,
                    key: format!("#{}", id.0),
                    title: t.title.clone(),
                    project: p.id.clone(),
                    status: "queued".into(),
                    allowed_actions: vec!["land_now".into()],
                    position,
                    why: queued_why(
                        frozen,
                        self.d.landing().draining(),
                        live.len(),
                        cfg.as_ref().map(|c| (c.parallel, c.window.as_secs())),
                    ),
                    ..Default::default()
                });
            }
        }
        out
    }

    pub fn usage(&self) -> UsageView {
        let title = |id: u64| {
            self.w
                .tasks
                .get(&TaskId(id))
                .map_or_else(String::new, |t| t.title.clone())
        };
        crate::usage::view(self.d, self.now / 1000, &title)
    }

    pub fn editor(&self, project: &str, step: &str) -> EditorView {
        let pid = self
            .w
            .projects
            .values()
            .find(|p| project.is_empty() || p.name == project)
            .map(|p| p.id);
        let Some(pid) = pid else {
            return EditorView::default();
        };
        let m = crate::editor::model(self.d, pid, step);
        let strs = |v: &serde_json::Value| -> Vec<String> {
            v.as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default()
        };
        let def = self.d.current_def(pid);
        EditorView {
            project: m["project"].as_str().unwrap_or_default().into(),
            text: m["text"].as_str().unwrap_or_default().into(),
            steps: def
                .steps
                .iter()
                .map(|s| StepNode {
                    name: s.name.clone(),
                    kind: s.kind.name().into(),
                    category: mason_definition::model::category_name(s.category).into(),
                    next: s.next.clone(),
                    on_red: s.on_red.clone().unwrap_or_default(),
                })
                .collect(),
            problems: strs(&m["problems"]),
            hash: def.hash.clone(),
            lanes: m["lanes"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|l| EditorLane {
                    category: l["category"].as_str().unwrap_or_default().into(),
                    title: l["title"].as_str().unwrap_or_default().into(),
                })
                .collect(),
            nodes: m["nodes"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|n| EditorNode {
                    name: n["name"].as_str().unwrap_or_default().into(),
                    kind: n["kind"].as_str().unwrap_or_default().into(),
                    category: n["category"].as_str().unwrap_or_default().into(),
                    lane: n["lane"].as_u64().unwrap_or(0) as u32,
                    x: n["x"].as_i64().unwrap_or(0),
                    y: n["y"].as_i64().unwrap_or(0),
                    placed: n["placed"].as_bool().unwrap_or(false),
                    problems: strs(&n["problems"]),
                    summary: n["summary"].as_str().unwrap_or_default().into(),
                    is_start: n["is_start"].as_bool().unwrap_or(false),
                })
                .collect(),
            edges: m["edges"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|e| EditorEdge {
                    from: e["from"].as_str().unwrap_or_default().into(),
                    to: e["to"].as_str().unwrap_or_default().into(),
                    kind: e["kind"].as_str().unwrap_or_default().into(),
                    bound: e["bound"].as_u64().unwrap_or(0) as u32,
                })
                .collect(),
            grid: Some(TestGridView {
                columns: strs(&m["grid"]["columns"]),
                levels: strs(&m["grid"]["levels"]),
                rows: m["grid"]["rows"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|r| GridRowView {
                        suite: r["suite"].as_str().unwrap_or_default().into(),
                        level: r["level"].as_str().unwrap_or_default().into(),
                        cells: r["cells"]
                            .as_array()
                            .map(|c| c.iter().map(|x| x.as_bool().unwrap_or(false)).collect())
                            .unwrap_or_default(),
                    })
                    .collect(),
            }),
            inspector: if m["inspector"].is_null() {
                None
            } else {
                Some(Inspector {
                    step: m["inspector"]["step"].as_str().unwrap_or_default().into(),
                    kind: m["inspector"]["kind"].as_str().unwrap_or_default().into(),
                    schema: m["inspector"]["schema"].to_string(),
                    values: m["inspector"]["values"].to_string(),
                    launch: m["inspector"]["launch"]["line"]
                        .as_str()
                        .unwrap_or_default()
                        .into(),
                    launch_from: {
                        let l = &m["inspector"]["launch"];
                        let from = |k: &str| l[k]["from"].as_str().unwrap_or("").to_string();
                        if l.is_null() {
                            String::new()
                        } else {
                            format!(
                                "tier: {}; level: {}; model: {}; effort: {}",
                                from("tier"),
                                from("level"),
                                from("model"),
                                from("effort")
                            )
                        }
                    },
                })
            },
            landing: m["landing"].to_string(),
            palette: m["palette"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|p| p.to_string())
                .collect(),
            suites: strs(&m["suites"]),
            step_names: strs(&m["steps"]),
            dirty: m["dirty"].as_bool().unwrap_or(false),
            can_undo: m["can_undo"].as_bool().unwrap_or(false),
            can_redo: m["can_redo"].as_bool().unwrap_or(false),
            diff: m["diff"].as_str().unwrap_or_default().into(),
            changes: strs(&m["changes"]),
            affected: strs(&m["affected"]),
            simulation: if m["simulation"].is_null() {
                String::new()
            } else {
                m["simulation"].to_string()
            },
            last_error: strs(&m["last_error"]),
            selected: m["selected"].as_str().unwrap_or_default().into(),
            protected: m["protected"].as_bool().unwrap_or(false),
        }
    }

    pub fn view(
        &self,
        kind: ViewKind,
        project: &str,
        task: u64,
        step: &str,
        cpu_span_ms: u64,
        disk_span_ms: u64,
    ) -> Option<View> {
        use mason_protocol::view::Body;
        let body = match kind {
            ViewKind::Header => Body::Header(self.header(project)),
            ViewKind::Sidebar => Body::Sidebar(self.sidebar(project)),
            ViewKind::Home => Body::Home(self.home(project)),
            ViewKind::TaskDetail => Body::TaskDetail(self.task_detail(task)?),
            ViewKind::Landing => Body::Landing(self.landing(project)),
            ViewKind::Usage => Body::Usage(self.usage()),
            ViewKind::Editor => Body::Editor(self.editor(project, step)),
            ViewKind::CpuHistory => Body::CpuHistory(self.d.cpu_history(
                self.now,
                if cpu_span_ms == 0 {
                    3_600_000
                } else {
                    cpu_span_ms
                },
            )),
            ViewKind::DiskHistory => Body::DiskHistory(self.d.disk_history(
                self.now,
                if disk_span_ms == 0 {
                    3_600_000
                } else {
                    disk_span_ms
                },
            )),
            _ => return None,
        };
        Some(View { body: Some(body) })
    }
}

/// The longest banner `text`, in characters (#951).
pub const BANNER_TEXT_MAX: usize = 80;

/// Keeps a banner's `text` to one short line (#951): newlines become spaces, and a text over
/// [`BANNER_TEXT_MAX`] is cut at a word with an ellipsis. Whatever was cut goes to `detail`
/// (the whole text, if the producer wrote none), so nothing is lost.
fn one_line(b: &mut Banner) {
    let flat = b.text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= BANNER_TEXT_MAX {
        if flat != b.text && b.detail.is_empty() {
            b.detail = b.text.trim().to_string();
        }
        b.text = flat;
        return;
    }
    let mut cut: String = flat.chars().take(BANNER_TEXT_MAX - 1).collect();
    if let Some(i) = cut.rfind(' ') {
        if i > BANNER_TEXT_MAX / 2 {
            cut.truncate(i);
        }
    }
    let cut = cut.trim_end_matches([' ', ',', ';', ':']).to_string();
    if b.detail.is_empty() {
        b.detail = b.text.trim().to_string();
    }
    b.text = format!("{cut}…");
}

/// Who a banner is for (s929): `action` needs the owner, `warning` is wrong and nobody is
/// handling it, `info` is progress or already being handled. A kind not listed is a warning,
/// so a new producer shows in the bar until it is classified.
pub fn banner_level(kind: &str) -> &'static str {
    match kind {
        "push_auth" | "health" | "definition" | "credentials" | "provider_budget"
        | "provider_hold" => "action",
        "provider_limit"
        | "provider_outage"
        | "provider_overage"
        | "provider_extra_billing"
        | "provider_warning"
        | "ci_watch_quiet"
        | "review" => "info",
        _ => "warning",
    }
}

/// The last 40 log lines of a task, kept with the logs' signature (each
/// journal's size and modified time) so an event that changed nothing in the
/// logs does not read every journal again.
/// The banners of one project's base-branch CI watch (`ci_watch::view`): what is red and
/// waiting for agreement, jobs an open ticket is fixing, why the watch cannot look, and, as
/// a quiet line of its own kind, fixes that landed and are not yet confirmed by a run.
fn ci_watch_banners(project: &str, v: &serde_json::Value) -> Vec<Banner> {
    let banner = |kind: &str, text: String| Banner {
        kind: kind.into(),
        text,
        ..Default::default()
    };
    let mut banners = vec![];
    let pending: Vec<String> = v["pending"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|x| x["job"].as_str().map(String::from))
        .collect();
    if !pending.is_empty() {
        banners.push(banner(
            "ci_watch",
            format!("{project}: red on the base branch, waiting for a second red run"),
        ));
        if let Some(b) = banners.last_mut() {
            b.detail = format!(
                "{project}: red on the base branch, waiting for a second red run: {}",
                pending.join(", ")
            );
        }
    }
    // Jobs an open ticket is fixing are handled: one info entry per project.
    let held: Vec<&serde_json::Value> = v["held_back"].as_array().into_iter().flatten().collect();
    if !held.is_empty() {
        let tickets: Vec<String> = held
            .iter()
            .filter_map(|h| h["task"].as_u64())
            .collect::<std::collections::BTreeSet<u64>>()
            .into_iter()
            .map(|t| format!("#{t}"))
            .collect();
        let n = held.len();
        let text = if tickets.is_empty() {
            format!(
                "{project}: {n} red CI job{} being fixed",
                if n == 1 { "" } else { "s" }
            )
        } else {
            format!(
                "{project}: {n} red CI job{} being fixed: {}",
                if n == 1 { "" } else { "s" },
                tickets.join(", ")
            )
        };
        let summary = format!(
            "{project}: {n} red CI job{} being fixed",
            if n == 1 { "" } else { "s" }
        );
        banners.push(Banner {
            level: "info".into(),
            detail: text,
            ..banner("ci_watch", summary)
        });
    }
    if let Some(t) = v["unconfirmed_text"].as_str() {
        banners.push(banner("ci_watch_quiet", format!("{project}: {t}")));
    }
    if let Some(e) = v["error"].as_str() {
        banners.push(Banner {
            detail: format!("{project}: the base-branch CI watch cannot look: {e}"),
            ..banner(
                "ci_watch",
                format!("{project}: the base-branch CI watch cannot look"),
            )
        });
    }
    banners
}

fn log_tail(d: &Daemon, id: u64) -> String {
    static CACHE: std::sync::Mutex<Option<LogCache>> = std::sync::Mutex::new(None);
    let stamp = crate::http::log_signature(d, id);
    if let Some((s, tail)) = CACHE
        .lock()
        .ok()
        .and_then(|c| c.as_ref().and_then(|m| m.get(&id).cloned()))
    {
        if s == stamp {
            return tail;
        }
    }
    let log = crate::http::render_logs(d, id);
    let lines: Vec<&str> = log.lines().collect();
    let tail = lines[lines.len().saturating_sub(40)..].join("\n");
    if let Ok(mut c) = CACHE.lock() {
        let m = c.get_or_insert_with(Default::default);
        if m.len() >= 64 {
            m.clear();
        }
        m.insert(id, (stamp, tail.clone()));
    }
    tail
}

fn debt_line(x: &mason_store::DebtRow) -> DebtLine {
    DebtLine {
        id: x.id,
        task: x.task,
        scope: x.scope.clone(),
        status: x.status.clone(),
        reason: x.reason.clone(),
        deferred: x.deferred,
    }
}

/// A short, plain sentence for an event, for clients that show a feed.
pub fn summarize(kind: &EventKind) -> (String, String, u64) {
    let name = format!("{kind:?}");
    let name = name
        .split([' ', '{', '('])
        .next()
        .unwrap_or("?")
        .to_string();
    let task = match kind {
        EventKind::Approved { task }
        | EventKind::Rejected { task }
        | EventKind::Admitted { task, .. }
        | EventKind::StepSucceeded { task, .. }
        | EventKind::SentBack { task, .. }
        | EventKind::ExplorationRequested { task, .. }
        | EventKind::ExploreAnyway { task, .. }
        | EventKind::QuestionAsked { task, .. }
        | EventKind::Answered { task, .. }
        | EventKind::StepFailed { task, .. }
        | EventKind::Retried { task, .. }
        | EventKind::Repinned { task, .. }
        | EventKind::Canceled { task, .. }
        | EventKind::ClosedNoChange { task, .. }
        | EventKind::Paused { task }
        | EventKind::Resumed { task }
        | EventKind::WaitStarted { task, .. }
        | EventKind::WaitEnded { task }
        | EventKind::TrackerEdited { task, .. }
        | EventKind::BlockerRemoved { task, .. }
        | EventKind::BlockersChanged { task, .. }
        | EventKind::Revived { task }
        | EventKind::BlockerAdded { task, .. } => task.0,
        _ => 0,
    };
    let summary = match kind {
        EventKind::TaskFiled { title, .. } => format!("filed: {title}"),
        EventKind::StepFailed { reason, .. } => format!("failed: {reason}"),
        EventKind::SentBack { reason, .. } => format!("sent back: {reason}"),
        EventKind::Repinned {
            from, to, changed, ..
        } => format!("repinned {from} -> {to}: {changed}"),
        EventKind::WaitStarted { on, .. } => format!("waiting: {on}"),
        EventKind::Canceled { .. } => "canceled".into(),
        EventKind::TrackerEdited { .. } => "edited in the tracker".into(),
        EventKind::StepSucceeded { next, .. } => match next {
            Some(n) if n.state == State::Landed => "landed".into(),
            Some(n) => format!("step done, next: {}", n.step),
            None => "step done".into(),
        },
        _ => name.clone(),
    };
    let _ = card::ASK_MAX;
    (name, summary, task)
}

/// A project's recorded phase entries and when its finished batches ended.
struct PhaseHistory {
    rows: Vec<mason_store::BatchPhaseRow>,
    finished: std::collections::HashMap<u64, u64>,
}

struct BatchInfo {
    state: BatchState,
    phase_ms: u64,
    typical_ms: Option<u64>,
    members: Vec<BatchMember>,
}

/// What `batch_state` reads.
struct BatchFacts<'a> {
    phase: &'a str,
    note: &'a str,
    round: u64,
    total: usize,
    landed: &'a [u64],
    parked: &'a [u64],
    phase_ms: u64,
    typical_ms: Option<u64>,
    frozen: bool,
}

#[derive(Debug, PartialEq, Eq)]
struct BatchState {
    text: String,
    severity: &'static str,
    reason: String,
    remaining: usize,
}

/// With no history, a batch is stuck after this long in one phase.
const STUCK_NO_HISTORY_MS: u64 = 30 * 60_000;

/// When a batch counts as stuck in its phase: three times the usual time there, or 30 minutes
/// without history. (The store keeps no output times, so time in the phase stands for "no
/// progress".)
fn stuck_after(typical_ms: Option<u64>) -> u64 {
    typical_ms.map_or(STUCK_NO_HISTORY_MS, |t| t.saturating_mul(3))
}

/// The durations of every finished batch's stay in `phase`, from the recorded entries: a stay
/// ends at the batch's next entry, or at its end. `current` is left out.
fn phase_durations(
    rows: &[mason_store::BatchPhaseRow],
    finished: &std::collections::HashMap<u64, u64>,
    current: u64,
    phase: &str,
) -> Vec<u64> {
    let mut out = vec![];
    for (i, r) in rows.iter().enumerate() {
        if r.batch == current || r.phase != phase {
            continue;
        }
        let end = match rows.get(i + 1) {
            Some(n) if n.batch == r.batch => Some(n.at),
            _ => finished.get(&r.batch).copied(),
        };
        if let Some(e) = end {
            out.push(e.saturating_sub(r.at));
        }
    }
    out
}

/// The median of at least three samples; nothing with fewer.
fn typical_from(samples: &[u64]) -> Option<u64> {
    if samples.len() < 3 {
        return None;
    }
    let mut v = samples.to_vec();
    v.sort_unstable();
    Some(v[v.len() / 2])
}

/// One member's own standing in its batch.
fn member_standing(phase: &str, task: u64, landed: &[u64], parked: &[u64]) -> String {
    if landed.contains(&task) {
        "landed".into()
    } else if parked.contains(&task) {
        "parked by blame".into()
    } else {
        match phase {
            "merged" => "integrating",
            "verifying" => "verifying",
            "red" => "verifying, red batch",
            "promoting" => "landing",
            "landed" => "landed",
            _ => "forming",
        }
        .into()
    }
}

/// The failing suites a red batch's note names: its first clause, up to a parenthesis.
fn red_suites(note: &str) -> String {
    let n = note.split(" (").next().unwrap_or(note);
    if let Some(i) = n.find(" is red in ") {
        let rest = &n[i + " is red in ".len()..];
        return rest.split(" and ").next().unwrap_or(rest).to_string();
    }
    n.trim().to_string()
}

/// The batch's state in words, how worried to be, and why. Display only: nothing here
/// decides a verdict.
fn batch_state(f: &BatchFacts) -> BatchState {
    let remaining = f
        .total
        .saturating_sub(f.landed.len() + f.parked.len())
        .min(f.total);
    let (_, words) = batch_phase(f.phase);
    let mut warns: Vec<String> = vec![];
    let mut stuck: Vec<String> = vec![];
    let text = if f.phase == "red" {
        let suite = red_suites(f.note);
        let held = f.note.contains("held") || f.note.contains("waits for the owner");
        if held {
            stuck.push("held: the red was not pinned on a task".into());
            format!("red in {suite}, held")
        } else if !f.parked.is_empty() {
            let blamed: Vec<String> = f.parked.iter().map(|t| format!("#{t}")).collect();
            warns.push(format!("a task was parked: blamed {}", blamed.join(", ")));
            format!("red in {suite}, blamed {}", blamed.join(", "))
        } else {
            warns.push("red: the batch is being split".into());
            format!(
                "red in {suite}, splitting (round {}, {remaining} tasks left)",
                f.round.max(1)
            )
        }
    } else if f.round > 1 && matches!(f.phase, "verifying" | "merged") {
        format!("{words} (round {})", f.round)
    } else {
        words.to_string()
    };
    if f.phase != "red" && !f.parked.is_empty() {
        warns.push(format!("{} task(s) parked from the batch", f.parked.len()));
    }
    if f.round > 1 && f.phase != "red" {
        warns.push(format!("it was split: round {}", f.round));
    }
    for (needle, why) in [
        ("rerun", "a check was rerun"),
        ("timed out", "a remote run was cut off at its limit"),
        ("known-red", "a known-red excuse applies"),
        ("known red", "a known-red excuse applies"),
    ] {
        if f.note.contains(needle) && !warns.iter().any(|w| w == why) {
            warns.push(why.into());
        }
    }
    if f.frozen && !matches!(f.phase, "landed" | "parked" | "failed" | "abandoned") {
        stuck.push("the project is frozen".into());
    }
    let live = !matches!(f.phase, "landed" | "parked" | "failed" | "abandoned");
    let limit = stuck_after(f.typical_ms);
    if live && f.phase_ms > limit {
        stuck.push(format!(
            "no progress for {} min in this phase (limit {} min)",
            f.phase_ms / 60_000,
            limit / 60_000
        ));
    }
    let (severity, reason) = if !stuck.is_empty() {
        ("stuck", stuck.join("; "))
    } else if !warns.is_empty() {
        ("warn", warns.join("; "))
    } else {
        ("ok", String::new())
    };
    BatchState {
        text,
        severity,
        reason,
        remaining,
    }
}

/// Phases a batch goes through while landing: forming, integrating, running suites, landing.
const LANDING_PHASES: u32 = 4;

/// The batch row's phase as (index, words), matching the progress cells in landing.rs.
fn batch_phase(phase: &str) -> (u32, &'static str) {
    match phase {
        "merged" => (1, "integrating"),
        "verifying" | "red" => (2, "running suites"),
        "promoting" => (3, "landing"),
        "landed" => (LANDING_PHASES, "landed"),
        _ => (0, "forming"),
    }
}

/// Why a queued task waits. `frozen` is the freeze's fix task when the project is frozen;
/// `config` is (batches at once, window seconds), absent when landing.toml is unreadable.
fn queued_why(
    frozen: Option<Option<u64>>,
    paused: bool,
    live: usize,
    config: Option<(usize, u64)>,
) -> String {
    if let Some(fix) = frozen {
        return match fix {
            Some(n) => format!("frozen until #{n} lands"),
            None => "frozen".into(),
        };
    }
    if paused {
        return "paused".into();
    }
    if live > 0 && config.is_none_or(|(par, _)| live >= par.max(1)) {
        return format!("behind {live} batch{}", if live == 1 { "" } else { "es" });
    }
    match config {
        Some((_, w)) if w > 0 => format!("waiting for the {w}s window to close"),
        _ => "waiting for the next batch".into(),
    }
}

/// The `gha` and `gha-macos` pools for the monitors' limit control (#1261): the limit, the
/// account ceiling, MASON's units and the other workflows', and how many requests wait. Always
/// sent, so an idle view can still raise the limit. A pool the daemon has no capacity for is left
/// out.
pub fn runner_pools(d: &crate::daemon::Daemon) -> Vec<mason_protocol::RunnerPool> {
    let snap = d.broker.snapshot();
    let gha = &snap["gha"];
    let num = |v: &serde_json::Value| v.as_u64().unwrap_or(0).min(u64::from(u32::MAX)) as u32;
    let mut waiting = [0u32; 2];
    for w in gha["waiters"].as_array().into_iter().flatten() {
        for (i, name) in [mason_scheduler::GHA, mason_scheduler::GHA_MACOS]
            .iter()
            .enumerate()
        {
            if w["uses"].get(*name).is_some() {
                waiting[i] += 1;
            }
        }
    }
    let mut out = Vec::new();
    for (i, name) in [mason_scheduler::GHA, mason_scheduler::GHA_MACOS]
        .into_iter()
        .enumerate()
    {
        let Some(limit) = d.broker.limit(name) else {
            continue;
        };
        let (ceiling, other) = if i == 0 {
            (num(&gha["ceiling"]), num(&gha["other"]))
        } else {
            (num(&gha["macos"]["ceiling"]), num(&gha["macos"]["other"]))
        };
        let used = d.broker.used(name);
        // MASON's share: what is held beyond the other workflows' jobs and the part of the limit
        // above the ceiling.
        let phantom = other.min(limit)
            + limit
                .saturating_sub(ceiling)
                .min(limit.saturating_sub(other.min(limit)));
        out.push(mason_protocol::RunnerPool {
            name: name.into(),
            limit,
            ceiling,
            used,
            mason: used.saturating_sub(phantom),
            other,
            waiting: waiting[i],
        });
    }
    out
}

/// The runs out on GitHub now, by pool and by what they are for. `None` when there are none.
pub fn remote_summary(
    d: &crate::daemon::Daemon,
    holds: &[crate::remote::RemoteHold],
) -> Option<mason_protocol::RemoteSummary> {
    if holds.is_empty() {
        return None;
    }
    let mut out = mason_protocol::RemoteSummary::default();
    for name in ["gha", "gha-macos"] {
        if let Some(limit) = d.broker.limit(name) {
            out.pools.push(mason_protocol::PoolUse {
                name: name.into(),
                used: d.broker.used(name),
                limit,
            });
        }
    }
    for h in holds {
        let units = h.units.get("gha").copied().unwrap_or(1);
        match h.kind.as_str() {
            "check" => out.checks += units,
            "test_step" => out.test_steps += units,
            "debt" => out.debts += units,
            _ => out.batches += units,
        }
        let (url, runs_on) = crate::remote::record_view(&h.record);
        out.runs.push(mason_protocol::RemoteRun {
            kind: if h.kind.is_empty() {
                "batch".into()
            } else {
                h.kind.clone()
            },
            task: h.task,
            runs_on,
            url,
            units,
            label: h.label.clone(),
        });
    }
    Some(out)
}

#[cfg(test)]
mod batch_phase_tests {
    use super::*;

    #[test]
    fn land_now_is_offered_until_the_batch_starts_promoting() {
        for phase in ["forming", "merged", "verifying"] {
            assert_eq!(batch_actions(phase, true), ["land_now"], "{phase}");
        }
        for phase in ["promoting", "landed", "parked", "failed", "red"] {
            assert!(batch_actions(phase, true).is_empty(), "{phase}");
        }
        assert!(batch_actions("verifying", false).is_empty(), "not running");
    }

    #[test]
    fn suites_are_phase_two_of_four() {
        assert_eq!(batch_phase("verifying"), (2, "running suites"));
        assert_eq!(batch_phase("forming"), (0, "forming"));
        assert_eq!(batch_phase("promoting").0, 3);
    }

    #[test]
    fn a_queued_task_says_why_it_waits() {
        assert_eq!(queued_why(None, false, 1, Some((1, 30))), "behind 1 batch");
        assert_eq!(queued_why(None, false, 2, Some((2, 0))), "behind 2 batches");
        assert_eq!(
            queued_why(Some(Some(12)), false, 1, Some((1, 0))),
            "frozen until #12 lands"
        );
        assert_eq!(queued_why(None, true, 0, None), "paused");
        assert_eq!(
            queued_why(None, false, 0, Some((1, 30))),
            "waiting for the 30s window to close"
        );
        assert_eq!(
            queued_why(None, false, 1, Some((2, 30))),
            "waiting for the 30s window to close"
        );
    }
}

/// Whether a task is waiting: the same activities `why_waiting` explains.
fn is_waiting(t: &Task) -> bool {
    matches!(
        t.activity,
        Activity::Waiting(_) | Activity::Held(_) | Activity::Paused
    )
}

/// Order of the Queued section: tasks waiting for a slot first, then by rank.
fn sort_queued<R: Ord>(tasks: &mut [&Task], rank: impl Fn(&Task) -> R) {
    tasks.sort_by_cached_key(|t| (t.activity != Activity::Waiting(WaitReason::Slot), rank(t)));
}

/// Order of every section but Queued: tasks that are not waiting first, then
/// the one that entered the state most recently, then the higher id.
fn sort_unqueued(tasks: &mut [&Task]) {
    tasks.sort_by_key(|t| {
        (
            is_waiting(t),
            std::cmp::Reverse(t.state_entered_at),
            std::cmp::Reverse(t.id),
        )
    });
}

#[cfg(test)]
mod section_order_tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};

    fn task(id: u64, at: u64, activity: Activity) -> Task {
        Task {
            id: TaskId(id),
            project: ProjectId(1),
            title: String::new(),
            body: String::new(),
            priority: Priority::Normal,
            trust: TrustSource::Owner,
            enqueued_at: Timestamp(0),
            state_entered_at: Timestamp(at),
            state: State::Testing,
            activity,
            landed: None,
            explored: false,
            blockers: BTreeSet::new(),
            not_before: None,
            paused: false,
            failures: 0,
            attempts: BTreeMap::new(),
            decision: None,
            cancel_keeps_branch: false,
            failure: None,
            failure_class: None,
            def: String::new(),
            step: String::new(),
            step_attempts: BTreeMap::new(),
            route: None,
            explore_returns: 0,
            approved: false,
            force_explore: false,
        }
    }

    #[test]
    fn tasks_that_are_not_waiting_come_first_then_newest_first() {
        let a = task(1, 10, Activity::Running);
        let b = task(
            2,
            30,
            Activity::Waiting(WaitReason::On("a test slot".into())),
        );
        let c = task(3, 30, Activity::Running);
        let d = task(4, 20, Activity::Paused);
        let e = task(5, 25, Activity::Held(BTreeSet::from([TaskId(9)])));
        let mut v = vec![&a, &b, &c, &d, &e];
        sort_unqueued(&mut v);
        let ids: Vec<u64> = v.iter().map(|t| t.id.0).collect();
        assert_eq!(ids, vec![3, 1, 2, 5, 4]);
    }

    #[test]
    fn queued_keeps_slot_waiters_first_then_rank_order() {
        let a = task(1, 50, Activity::Paused);
        let b = task(2, 10, Activity::Waiting(WaitReason::Slot));
        let c = task(3, 40, Activity::Running);
        let d = task(4, 20, Activity::Waiting(WaitReason::Slot));
        let mut v = vec![&a, &b, &c, &d];
        // Rank is the id here: neither the activity order nor age enters it.
        sort_queued(&mut v, |t| t.id.0);
        let ids: Vec<u64> = v.iter().map(|t| t.id.0).collect();
        assert_eq!(ids, vec![2, 4, 1, 3]);
    }
}

#[cfg(test)]
mod tile_label_tests {
    use super::*;

    #[test]
    fn needs_attention_reads_stuck_and_the_rest_keep_their_names() {
        assert_eq!(tile_label_of(State::NeedsAttention), "stuck");
        assert_eq!(tile_label_of(State::Queued), "queued");
        assert_eq!(tile_label_of(State::Exploring), "exploring");
        assert_eq!(tile_label_of(State::Landed), "landed");
        assert_eq!(title_of(State::NeedsAttention), "Needs you");
    }
}

#[cfg(test)]
mod index_tests {
    use super::*;

    fn run(id: u64, task: u64, name: &str, at: u64, ok: bool) -> StepRun {
        StepRun {
            id: StepRunId(id),
            task: TaskId(task),
            step: State::Developing,
            name: name.into(),
            attempt: 1,
            started_at: Timestamp(at),
            outcome: ok.then_some(StepOutcome::Succeeded),
        }
    }

    /// The code this index replaced: every run scans every run.
    fn naive(runs: &[StepRun], step: &str) -> Option<(u64, u64)> {
        let mut spans = vec![];
        for r in runs
            .iter()
            .filter(|r| r.name == step && r.outcome == Some(StepOutcome::Succeeded))
        {
            let next = runs
                .iter()
                .filter(|n| n.task == r.task && n.id.0 > r.id.0)
                .map(|n| n.started_at.0)
                .min();
            if let Some(end) = next {
                spans.push(end.saturating_sub(r.started_at.0));
            }
        }
        mason_core::progress::middle_half(&spans)
    }

    /// A deterministic spread of tasks, steps, outcomes and starts that go
    /// backwards now and then (the old code took the earliest later start).
    fn data(tasks: u64) -> Vec<StepRun> {
        let names = ["explore", "develop", "test", "land"];
        let mut runs = vec![];
        let mut id = 0;
        for t in 0..tasks {
            for k in 0..6u64 {
                id += 1;
                let at = 10_000 + t * 7 + k * (3 + (t * 13 + k * 5) % 11) * 1_000
                    - if (t + k) % 5 == 4 { 2_500 } else { 0 };
                runs.push(run(
                    id,
                    t,
                    names[((t + k) % 4) as usize],
                    at,
                    (t + 2 * k) % 7 != 0,
                ));
            }
        }
        runs
    }

    #[test]
    fn typical_figures_match_the_code_they_replaced() {
        let runs = data(60);
        let ix = RunIndex::build(runs.iter());
        let mut some = 0;
        for name in ["explore", "develop", "test", "land", "nothing", ""] {
            let want = naive(&runs, name).filter(|_| !name.is_empty());
            some += usize::from(want.is_some());
            assert_eq!(ix.typical.get(name).copied(), want, "{name}");
        }
        assert!(some >= 3, "the data must exercise real figures");
    }

    #[test]
    fn building_the_index_visits_each_run_a_fixed_number_of_times() {
        // By construction, not by timing: the visits grow in step with the runs.
        for n in [50, 500, 5_000] {
            let runs = data(n);
            let ix = RunIndex::build(runs.iter());
            assert_eq!(ix.visits, 2 * runs.len(), "{n} tasks");
            // A row reads one task's runs and never the whole history.
            assert_eq!(ix.of_task(TaskId(0)).len(), 6);
        }
    }

    #[test]
    fn a_tasks_runs_are_sorted_and_its_last_start_is_the_latest() {
        let runs = [run(5, 1, "a", 10, true), run(2, 1, "a", 30, true)];
        let ix = RunIndex::build(runs.iter());
        let ids: Vec<u64> = ix.of_task(TaskId(1)).iter().map(|r| r.id.0).collect();
        assert_eq!(ids, vec![2, 5]);
        assert_eq!(ix.last_start[&TaskId(1)], 30);
    }
}

/// What the owner may do to a batch (or a task in it): Land now, while it runs and has not
/// started promoting.
fn batch_actions(phase: &str, running: bool) -> Vec<String> {
    if running && matches!(phase, "forming" | "merged" | "verifying") {
        vec!["land_now".into()]
    } else {
        vec![]
    }
}

#[cfg(test)]
mod batch_state_tests {
    use super::*;

    fn facts<'a>(
        phase: &'a str,
        note: &'a str,
        round: u64,
        phase_ms: u64,
        typical_ms: Option<u64>,
    ) -> BatchFacts<'a> {
        BatchFacts {
            phase,
            note,
            round,
            total: 5,
            landed: &[],
            parked: &[],
            phase_ms,
            typical_ms,
            frozen: false,
        }
    }

    fn row(batch: u64, phase: &str, at: u64) -> mason_store::BatchPhaseRow {
        mason_store::BatchPhaseRow {
            batch,
            phase: phase.into(),
            at,
        }
    }

    #[test]
    fn a_red_batch_in_a_split_round_is_a_warning_not_running_suites() {
        let s = batch_state(&facts(
            "red",
            "lint is red in lint (3 tests)",
            1,
            1_000,
            None,
        ));
        assert_eq!(s.severity, "warn");
        assert!(
            s.text.contains("splitting (round 1, 5 tasks left)"),
            "{}",
            s.text
        );
        assert!(!s.text.contains("running suites"));
    }

    #[test]
    fn a_batch_verifying_past_stuck_after_is_stuck() {
        let limit = stuck_after(Some(60_000));
        assert_eq!(limit, 180_000);
        let s = batch_state(&facts("verifying", "", 1, limit + 1, Some(60_000)));
        assert_eq!(s.severity, "stuck");
        let s = batch_state(&facts("verifying", "", 1, limit - 1, Some(60_000)));
        assert_eq!(s.severity, "ok");
        assert_eq!(stuck_after(None), 30 * 60_000);
    }

    #[test]
    fn typical_time_is_the_median_of_three_phase_durations_and_absent_with_fewer() {
        let rows = vec![
            row(1, "verifying", 0),
            row(1, "promoting", 100),
            row(2, "verifying", 0),
            row(2, "promoting", 300),
            row(3, "verifying", 0),
            row(3, "promoting", 200),
            row(4, "verifying", 0),
        ];
        let finished = std::collections::HashMap::new();
        let d = phase_durations(&rows, &finished, 4, "verifying");
        assert_eq!(typical_from(&d), Some(200));
        assert_eq!(typical_from(&d[..2]), None);
    }
}

#[cfg(test)]
mod banner_level_tests {
    use super::*;

    #[test]
    fn every_banner_kind_gets_its_level() {
        let table = [
            ("push_auth", "action"),
            ("health", "action"),
            ("definition", "action"),
            ("credentials", "action"),
            ("provider_budget", "action"),
            ("provider_hold", "action"),
            ("landing_frozen", "warning"),
            ("paused", "warning"),
            ("ci_watch", "warning"),
            ("ci_watch_quiet", "info"),
            ("review", "info"),
            ("provider_limit", "info"),
            ("provider_outage", "info"),
            ("provider_overage", "info"),
            ("provider_extra_billing", "info"),
            ("provider_warning", "info"),
            ("something_new", "warning"),
        ];
        for (kind, level) in table {
            assert_eq!(banner_level(kind), level, "{kind}");
        }
    }

    #[test]
    fn held_back_jobs_of_one_project_are_one_info_banner() {
        let v = serde_json::json!({
            "held_back": [
                {"job": "a", "task": 935, "text": "a red; #935 is fixing it"},
                {"job": "b", "task": 836, "text": "b red; #836 is fixing it"},
                {"job": "c", "task": 935, "text": "c red; #935 is fixing it"},
            ],
            "pending": [{"job": "d"}],
            "error": "gh failed",
        });
        let b = ci_watch_banners("mason", &v);
        let held: Vec<_> = b.iter().filter(|b| b.level == "info").collect();
        assert_eq!(held.len(), 1);
        assert_eq!(held[0].text, "mason: 3 red CI jobs being fixed");
        assert_eq!(
            held[0].detail,
            "mason: 3 red CI jobs being fixed: #836, #935"
        );
        // Waiting for a second red run and "cannot look" are not handled: they keep the default.
        let rest: Vec<_> = b.iter().filter(|b| b.level != "info").collect();
        assert_eq!(rest.len(), 2);
        assert!(rest.iter().all(|b| banner_level(&b.kind) == "warning"));
    }

    #[test]
    fn a_banner_text_is_one_short_line_and_the_rest_is_detail() {
        let long = format!(
            "Landing frozen:\nonly the fix task lands {}",
            "until green ".repeat(20)
        );
        let mut b = Banner {
            kind: "landing_frozen".into(),
            text: long.clone(),
            ..Default::default()
        };
        one_line(&mut b);
        assert!(!b.text.contains('\n'));
        assert!(b.text.chars().count() <= BANNER_TEXT_MAX, "{}", b.text);
        assert!(b.text.ends_with('…'));
        assert_eq!(b.detail, long.trim());
        // A producer's own detail is kept; a short text with no newline is untouched.
        let mut c = Banner {
            text: "short".into(),
            detail: "why".into(),
            ..Default::default()
        };
        one_line(&mut c);
        assert_eq!((c.text.as_str(), c.detail.as_str()), ("short", "why"));
        let mut d = Banner {
            text: "two\nlines".into(),
            ..Default::default()
        };
        one_line(&mut d);
        assert_eq!(
            (d.text.as_str(), d.detail.as_str()),
            ("two lines", "two\nlines")
        );
    }

    #[test]
    fn ci_watch_banners_keep_the_reason_in_the_detail() {
        let v = serde_json::json!({"pending": [{"job": "d"}], "error": "gh failed\nbadly"});
        let mut b = ci_watch_banners("mason", &v);
        for x in &mut b {
            one_line(x);
            assert!(!x.text.contains('\n') && x.text.chars().count() <= BANNER_TEXT_MAX);
        }
        assert!(b[0].detail.contains('d'));
        assert!(b[1].detail.contains("gh failed"));
    }
}
