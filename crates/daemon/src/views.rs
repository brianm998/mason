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

/// Failed, landed and canceled sort newest first and start collapsed.
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

fn tooltip_of(s: State) -> &'static str {
    match s {
        State::NeedsAttention => "A task has asked you a question and is waiting for your answer.",
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
}

impl<'a> Ctx<'a> {
    pub fn new(d: &'a Daemon, w: &'a World) -> Ctx<'a> {
        Ctx {
            d,
            w,
            now: now_ms(),
        }
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
            State::Canceled | State::Landed => vec![],
        };
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
        if let Some(w) = self.d.broker.explain_task(t.id) {
            return w;
        }
        match &t.activity {
            Activity::Waiting(WaitReason::On(w)) => w.clone(),
            Activity::Waiting(WaitReason::Slot) => "Waiting for an LLM session to be free".into(),
            Activity::Waiting(WaitReason::NotBefore(_)) => "Waiting for its start time".into(),
            Activity::Waiting(WaitReason::Owner) => "Waiting for your answer".into(),
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
        self.w
            .step_runs
            .values()
            .filter(|r| r.task == t.id)
            .map(|r| r.started_at.0)
            .max()
            .map_or(t.enqueued_at.0, |s| s.max(t.enqueued_at.0))
    }

    /// Typical time in a step, from this project's own history: only with
    /// enough samples to say (five), as `(low, high)` ms (the middle half).
    fn typical(&self, step: &str) -> Option<(u64, u64)> {
        if step.is_empty() {
            return None;
        }
        let mut spans: Vec<u64> = vec![];
        for r in self
            .w
            .step_runs
            .values()
            .filter(|r| r.name == step && r.outcome == Some(StepOutcome::Succeeded))
        {
            // A run ends when the task's next one starts.
            let next = self
                .w
                .step_runs
                .values()
                .filter(|n| n.task == r.task && n.id.0 > r.id.0)
                .map(|n| n.started_at.0)
                .min();
            if let Some(end) = next {
                spans.push(end.saturating_sub(r.started_at.0));
            }
        }
        if spans.len() < 5 {
            return None;
        }
        spans.sort_unstable();
        Some((spans[spans.len() / 4], spans[spans.len() * 3 / 4]))
    }

    fn progress(&self, t: &Task) -> Vec<ProgressCell> {
        let def = self.def_of(t);
        let runs: Vec<&StepRun> = self
            .w
            .step_runs
            .values()
            .filter(|r| r.task == t.id)
            .collect();
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
                ProgressCell {
                    step: s.name.clone(),
                    state: state as i32,
                    label,
                    done: 0,
                    total: 0,
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
            progress: self.progress(t),
            allowed_actions: self.allowed_actions(t),
            blockers: t.blockers.iter().map(|b| b.0).collect(),
            typical: typical.map_or_else(String::new, |(lo, hi)| {
                format!("usually {} to {} min", lo / 60_000, hi.div_ceil(60_000))
            }),
            // Past the typical time is amber, never red: slow is not broken.
            slow: typical.is_some_and(|(_, hi)| t.state.is_step() && in_state > hi),
            step: t.step.clone(),
            body: t.body.chars().take(2000).collect(),
        }
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
        let count = |s: State| tasks.iter().filter(|t| t.state == s).count() as u32;
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
        for (pid, p) in &self.d.projects {
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
                })
                .collect(),
            banners,
            projects: self.w.projects.values().map(|p| p.name.clone()).collect(),
            status: if attention == 0 {
                "All quiet".into()
            } else {
                format!(
                    "{attention} need{} you",
                    if attention == 1 { "s" } else { "" }
                )
            },
        }
    }

    fn section(&self, s: State, project: &str) -> SidebarSection {
        let mut tasks: Vec<&Task> = self
            .w
            .tasks
            .values()
            .filter(|t| t.state == s && self.matches(t, project))
            .collect();
        // Active states: oldest first, so anything stuck is at the top.
        // History: newest first, so what just happened is not buried.
        if is_history(s) {
            tasks.sort_by_key(|t| (std::cmp::Reverse(t.enqueued_at), std::cmp::Reverse(t.id)));
        } else {
            tasks.sort_by_key(|t| (t.enqueued_at, t.id));
        }
        SidebarSection {
            state: state_name(s).into(),
            title: title_of(s).into(),
            count: tasks.len() as u32,
            collapsed: is_history(s),
            rows: tasks.iter().map(|t| self.row(t)).collect(),
        }
    }

    pub fn sidebar(&self, project: &str) -> SidebarView {
        let mut sections: Vec<SidebarSection> =
            ORDER.iter().map(|s| self.section(*s, project)).collect();
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
        let llm = crate::sched::llm_resource(crate::sched::DEFAULT_ACCOUNT);
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
                .filter(|t| t.state.is_step())
                .map(|t| {
                    let r = self.row(t);
                    ActiveModel {
                        model: r.model,
                        project: r.project,
                        task: t.id.0,
                        step: t.step.clone(),
                    }
                })
                .filter(|a| !a.model.is_empty())
                .collect(),
            sessions_used: self.d.broker.used(&llm),
            sessions_limit: self.d.broker.limit(&llm).unwrap_or(0),
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
            .w
            .step_runs
            .values()
            .filter(|r| r.task == t.id)
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
            })
            .collect();
        steps.sort_by_key(|s| s.run);
        let log = crate::http::render_logs(self.d, id);
        let tail: String = {
            let lines: Vec<&str> = log.lines().collect();
            lines[lines.len().saturating_sub(40)..].join("\n")
        };
        let plan =
            std::fs::read_to_string(self.d.cfg.task_dir(id).join("plan.json")).unwrap_or_default();
        Some(TaskDetailView {
            row: Some(self.row(t)),
            steps,
            plan,
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
            },
            started_at: self
                .w
                .step_runs
                .values()
                .filter(|r| r.task == t.id)
                .min_by_key(|r| r.id)
                .map(|r| r.name.clone())
                .unwrap_or_default(),
            explore_returns: t.explore_returns,
            approval: self.approval(t),
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
        let mut idle = 0;
        let mut running_total = 0;
        for (pid, p) in &self.d.projects {
            if !in_scope(&p.id) {
                continue;
            }
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
        let name = crate::sched::DEFAULT_ACCOUNT;
        let llm = crate::sched::llm_resource(name);
        UsageView {
            providers: vec![ProviderStatus {
                name: name.into(),
                status: "ok".into(),
                sessions_used: self.d.broker.used(&llm),
                sessions_limit: self.d.broker.limit(&llm).unwrap_or(0),
            }],
            note: "Token counts and limit windows arrive with the usage tracking stage.".into(),
        }
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
