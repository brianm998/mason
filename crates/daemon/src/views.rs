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
const ORDER: [State; 10] = [
    State::NeedsAttention,
    State::Queued,
    State::Exploring,
    State::Developing,
    State::Testing,
    State::Review,
    State::Landing,
    State::Proposed,
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
    /// Built once: every step run by task, and the typical span per step.
    runs: RunIndex,
    /// Task to the tasks it blocks, in id order.
    blocks: std::collections::HashMap<TaskId, Vec<u64>>,
    /// The broker's wait reasons, taken once per build.
    waits: std::sync::Arc<std::collections::BTreeMap<TaskId, String>>,
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
        Ctx {
            d,
            w,
            now: now_ms(),
            issues,
            runs: RunIndex::build(w.step_runs.values()),
            blocks,
            waits: d.broker.waits_snapshot(),
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
            State::Proposed => vec!["approve", "reject", "edit"],
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
            State::Landing => vec!["cancel", "skip_test", "land_now", "force"],
            State::NeedsAttention => vec!["answer", "cancel", "retry", "open_session"],
            State::Failed => vec!["retry", "cancel", "open_session"],
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
        a.into_iter().map(String::from).collect()
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
        t.state == State::NeedsAttention || (t.state != State::Proposed && self.needs_approval(t))
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

    fn why_waiting(&self, t: &Task) -> String {
        if let Some(w) = self.waits.get(&t.id) {
            return w.clone();
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
            Activity::Waiting(WaitReason::Slot) => "Waiting for an LLM session to be free".into(),
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
        TaskRow {
            id: t.id.0,
            key: format!("#{}", t.id.0),
            title: t.title.clone(),
            project: self.project_name(t),
            state: state_name(t.state).to_string(),
            activity: crate::http::activity_text(&t.activity),
            why_waiting: self.why_waiting(t),
            enqueued_ms: t.enqueued_at.0,
            in_state_ms: in_state,
            model,
            display: model_use
                .as_ref()
                .map_or_else(String::new, |m| m.display.clone()),
            model_use,
            progress: self.progress(t),
            allowed_actions: self.allowed_actions(t),
            blockers: t.blockers.iter().map(|b| b.0).collect(),
            last_run: crate::http::latest_run(self.w, t.id),
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
            });
        }
        for (_, _, text) in crate::ssh_keys::alerts(self.d) {
            banners.push(Banner {
                kind: "push_auth".into(),
                text,
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
            let pending: Vec<String> = v["pending"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|x| x["job"].as_str().map(String::from))
                .collect();
            if !pending.is_empty() {
                banners.push(Banner {
                    kind: "ci_watch".into(),
                    text: format!(
                        "{}: red on the base branch, waiting for a second red run: {}",
                        p.id,
                        pending.join(", ")
                    ),
                });
            }
            if let Some(e) = v["error"].as_str() {
                banners.push(Banner {
                    kind: "ci_watch".into(),
                    text: format!("{}: the base-branch CI watch cannot look: {e}", p.id),
                });
            }
        }
        for f in self.d.store.freezes().unwrap_or_default() {
            if project.is_empty() || f.project == project {
                banners.push(Banner {
                    kind: "landing_frozen".into(),
                    text: if f.source == "ci" {
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
                });
            }
        }
        for (kind, text) in crate::holds::banners(self.d) {
            banners.push(Banner { kind, text });
        }
        HeaderView {
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
        // most recently on top.
        if s == State::Queued {
            tasks.sort_by_cached_key(|t| {
                (
                    t.activity != Activity::Waiting(WaitReason::Slot),
                    self.d.queued_rank(t),
                )
            });
        } else {
            tasks.sort_by_key(|t| {
                (
                    std::cmp::Reverse(t.state_entered_at),
                    std::cmp::Reverse(t.id),
                )
            });
        }
        SidebarSection {
            state: state_name(s).into(),
            title: title_of(s).into(),
            count: tasks.len() as u32,
            collapsed: is_history(s),
            rows: tasks
                .iter()
                .take(if is_history(s) {
                    HISTORY_ROW_CAP
                } else {
                    usize::MAX
                })
                .map(|t| self.row(t))
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
        })
    }

    /// The CPU shares, with each task's title filled in.
    pub fn cpu_usage(&self) -> CpuUsage {
        let mut u = self.d.cpu_reading().usage.clone();
        for s in u.shares.iter_mut().filter(|s| s.kind == "task") {
            if let Some(t) = self.w.tasks.get(&TaskId(s.task)) {
                s.title = t.title.clone();
            }
        }
        u
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
        HomeView {
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
                .filter(|t| self.d.broker.llm_held_by(t.id))
                .map(|t| {
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
                    ActiveModel {
                        model,
                        display: r.display,
                        model_use: r.model_use,
                        project: r.project,
                        task: t.id.0,
                        step: t.step.clone(),
                    }
                })
                .collect(),
            sessions_used,
            sessions_limit,
            compilation: Some(self.d.broker.compilation()),
            cpu: Some(self.cpu_usage()),
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

    pub fn task_detail(&self, id: u64) -> Option<TaskDetailView> {
        let t = self.w.tasks.get(&TaskId(id))?;
        let p = self.d.project(t.project);
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
            })
            .collect();
        steps.sort_by_key(|s| s.run);
        let tail = log_tail(self.d, id);
        let plan =
            std::fs::read_to_string(self.d.cfg.task_dir(id).join("plan.json")).unwrap_or_default();
        let plan_view = (!plan.is_empty()).then(|| crate::plan_view::parse_plan(&plan));
        let removes = crate::landing::removals_view(&self.d.cfg.task_dir(id));
        Some(TaskDetailView {
            step_tokens: crate::usage::step_tokens(self.d, id, self.now / 1000),
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
        LandingView {
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
                .map(|b| BatchLine {
                    id: b.id,
                    project: b.project.clone(),
                    phase: b.phase.clone(),
                    tasks: ids(&b.tasks),
                    landed: ids(&b.landed),
                    parked: ids(&b.parked),
                    note: b.note.clone(),
                    running: live.contains(&b.id),
                })
                .collect(),
            freezes,
            queue_idle_ms: idle,
        }
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

    pub fn view(&self, kind: ViewKind, project: &str, task: u64, step: &str) -> Option<View> {
        use mason_protocol::view::Body;
        let body = match kind {
            ViewKind::Header => Body::Header(self.header(project)),
            ViewKind::Sidebar => Body::Sidebar(self.sidebar(project)),
            ViewKind::Home => Body::Home(self.home(project)),
            ViewKind::TaskDetail => Body::TaskDetail(self.task_detail(task)?),
            ViewKind::Landing => Body::Landing(self.landing(project)),
            ViewKind::Usage => Body::Usage(self.usage()),
            ViewKind::Editor => Body::Editor(self.editor(project, step)),
            _ => return None,
        };
        Some(View { body: Some(body) })
    }
}

/// The last 40 log lines of a task, kept with the logs' signature (each
/// journal's size and modified time) so an event that changed nothing in the
/// logs does not read every journal again.
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
