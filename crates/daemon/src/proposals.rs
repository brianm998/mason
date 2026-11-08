// SPDX-License-Identifier: GPL-3.0-or-later

//! Proposals (Part 13). An agent that notices a problem outside its task files it
//! instead of fixing it; the daemon refuses duplicates and filings without evidence,
//! caps them, routes them to the project or to MASON's own repository, and lets the
//! policy decide whether the owner must look first. A proposal is a task plus a row
//! in the `proposals` table (the evidence, the route, the dedupe key); the rules are
//! pure functions in `mason_core::proposal`, applied here.

use crate::config::ProjectCfg;
use crate::daemon::{now_ms, Daemon};
use mason_core::proposal::*;
use mason_core::*;
use mason_store::ProposalRow;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;
use toml_edit::DocumentMut;

const DAY_MS: u64 = 24 * 3_600_000;
/// A proposal the owner rejected is not filed again for this long.
const REJECTED_MEMORY_MS: u64 = 7 * DAY_MS;

// ---- configuration -----------------------------------------------------------------------

/// How the owner reaches MASON's upstream repository.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Access {
    /// Can push to the repository: improvements land by the project's configured mode.
    #[default]
    Write,
    /// Cannot: improvements go to a branch on the owner's fork.
    Fork,
}

/// `[proposals]` and the access keys of `[self]`, read from the project file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cfg {
    pub policy: Policy,
    pub caps: Caps,
    pub access: Access,
    /// A git remote name or URL of the owner's fork (access = "fork").
    pub fork: Option<String>,
    /// The fork owner's name on the host, the prefix of a pull request's head (`owner:branch`).
    pub fork_owner: Option<String>,
    /// `takeover` (the default): switching to the branch replaces the running daemon, as an
    /// update does. `check`: build and test the branch but do not take over.
    pub switch_takes_over: bool,
}

pub fn parse_cfg(id: &str, doc: &DocumentMut) -> Result<Cfg, String> {
    let mut policy = Policy::default();
    let mut caps = Caps::default();
    if let Some(t) = doc.get("proposals").and_then(|v| v.as_table_like()) {
        for (k, v) in t.iter() {
            let disp = |v: &toml_edit::Item| {
                v.as_str().and_then(parse_disposition).ok_or_else(|| {
                    format!("{id}: `[proposals] {k}` must be \"proposed\" or \"queued\"")
                })
            };
            let num = |v: &toml_edit::Item| {
                v.as_integer()
                    .filter(|n| *n >= 1)
                    .map(|n| n as u32)
                    .ok_or_else(|| format!("{id}: `[proposals] {k}` must be a whole number from 1"))
            };
            match k {
                "agent_normal" => policy.agent_normal = disp(v)?,
                "agent_blocks" => policy.agent_blocks = disp(v)?,
                "outside_normal" => policy.outside_normal = disp(v)?,
                "outside_blocks" => policy.outside_blocks = disp(v)?,
                "per_task" => caps.per_task = num(v)?,
                "per_day" => caps.per_day = num(v)?,
                other => return Err(format!("{id}: unknown key `[proposals] {other}`")),
            }
        }
    }
    let s = doc.get("self").and_then(|v| v.as_table_like());
    let text = |k: &str| {
        s.and_then(|t| t.get(k))
            .and_then(|v| v.as_str())
            .map(String::from)
    };
    let access = match text("access").as_deref() {
        None | Some("write") => Access::Write,
        Some("fork") => Access::Fork,
        Some(o) => {
            return Err(format!(
                "{id}: `[self] access` must be \"write\" or \"fork\", not {o:?}"
            ))
        }
    };
    let fork = text("fork").filter(|f| !f.trim().is_empty());
    if access == Access::Fork && fork.is_none() {
        return Err(format!(
            "{id}: `[self] access = \"fork\"` needs `fork = \"<remote name or url>\"`"
        ));
    }
    let switch_takes_over = match text("switch").as_deref() {
        None | Some("takeover") => true,
        Some("check") => false,
        Some(o) => {
            return Err(format!(
                "{id}: `[self] switch` must be \"takeover\" or \"check\", not {o:?}"
            ))
        }
    };
    Ok(Cfg {
        policy,
        caps,
        access,
        fork,
        fork_owner: text("fork_owner"),
        switch_takes_over,
    })
}

impl ProjectCfg {
    /// The proposal settings (validated when the project loaded).
    pub fn proposals(&self) -> Cfg {
        self.overlay_text
            .parse::<DocumentMut>()
            .ok()
            .and_then(|d| parse_cfg(&self.id, &d).ok())
            .unwrap_or_else(|| Cfg {
                policy: Policy::default(),
                caps: Caps::default(),
                access: Access::Write,
                fork: None,
                fork_owner: None,
                switch_takes_over: true,
            })
    }
}

// ---- filing --------------------------------------------------------------------------------

/// What an agent (or the owner's command, for tests) asks to file.
pub struct Filing {
    /// The task whose agent noticed it.
    pub from_task: Option<TaskId>,
    /// The project that task works in.
    pub project: ProjectId,
    pub title: String,
    pub body: String,
    pub evidence: String,
    pub blocks_task: bool,
    pub explored: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Filed {
    pub task: TaskId,
    pub route: &'static str,
    pub proposed: bool,
    pub project: String,
}

/// A path cited as evidence, if it is safe to look for inside a repository.
fn relative(p: &str) -> Option<&str> {
    let ok = !p.is_empty()
        && !p.starts_with('/')
        && !p.starts_with('~')
        && !p.split('/').any(|c| c == "..");
    ok.then_some(p)
}

fn exists_in(root: &Path, p: &str) -> bool {
    relative(p).is_some_and(|p| root.join(p).exists())
}

/// Whether a repository has the file: on its base branch, or in its checkout.
fn in_repo(p: &ProjectCfg, file: &str) -> bool {
    relative(file).is_some_and(|f| {
        mason_landing::git::ok(
            &p.path,
            &["cat-file", "-e", &format!("{}:{f}", p.base_ref())],
        ) || exists_in(&p.path, f)
    })
}

/// MASON's own project, if one is registered.
pub fn self_project(d: &Daemon) -> Option<(ProjectId, &ProjectCfg)> {
    d.projects
        .iter()
        .find(|(_, p)| p.self_cfg.is_some())
        .map(|(id, p)| (*id, p))
}

/// The body of the task a proposal becomes: the text, then the evidence.
pub fn compose_body(body: &str, evidence: &str, from: Option<TaskId>) -> String {
    let by = from.map_or(String::new(), |t| {
        format!("\n\n_Filed by the agent of task #{}._", t.0)
    });
    if evidence.trim().is_empty() {
        return body.trim().to_string();
    }
    format!("{}\n\n## Evidence\n{}{by}", body.trim(), evidence.trim())
}

/// Apply every rule and, if the proposal passes, file it. `Err` is the reason it was
/// refused, in words the agent can act on; nothing is filed then.
pub fn file(d: &Arc<Daemon>, f: &Filing) -> Result<Filed, String> {
    let title = f.title.trim().to_string();
    if title.is_empty() || title.chars().count() > 120 {
        return Err("title must be 1 to 120 characters".into());
    }
    if f.body.trim().is_empty() {
        return Err("body must not be empty".into());
    }
    evidence_ok(&f.evidence)?;
    let w = d.world().map_err(|e| e.to_string())?;
    let working = d.project(f.project).ok_or("no such project")?;
    let cfg_here = working.proposals();

    // Where it goes: MASON's own repository when the evidence only cites its files.
    let mut target = (f.project, working);
    let mut route = Dest::Project;
    if working.self_cfg.is_none() {
        if let Some((sid, sp)) = self_project(d) {
            let cited: Vec<String> = locators(&f.evidence)
                .into_iter()
                .filter(|p| relative(p).is_some())
                .collect();
            let wt = f
                .from_task
                .map(|t| working.worktree(&d.cfg, t.0))
                .filter(|p| p.exists());
            let in_project =
                |p: &str| in_repo(working, p) || wt.as_deref().is_some_and(|w| exists_in(w, p));
            if route_of(&cited, |p| in_repo(sp, p), in_project) == Dest::Pipeline {
                target = (sid, sp);
                route = Dest::Pipeline;
            }
        }
    }
    let (tid, tproj) = target;
    let now = now_ms();

    // Duplicates: against what is still open in the target, and what the owner just turned down.
    for row in d.store.proposals_in(&tproj.id).map_err(|e| e.to_string())? {
        let Some(t) = w.tasks.get(&TaskId(row.task)) else {
            continue;
        };
        let live = !matches!(t.state, State::Landed | State::Canceled | State::Failed);
        let recently_rejected =
            row.status == "rejected" && now.saturating_sub(row.at) < REJECTED_MEMORY_MS;
        if (live || recently_rejected) && similar(&title, &f.body, &row.title, &row.body) {
            return Err(if live {
                format!(
                    "duplicate of #{}: {:?}; add to that task instead of filing again",
                    row.task, row.title
                )
            } else {
                format!(
                    "duplicate of #{}: {:?}, which the owner rejected; do not file it again",
                    row.task, row.title
                )
            });
        }
    }

    // Caps: per task, and per day for the project the proposal lands in.
    let by_task = f
        .from_task
        .map_or(Ok(0), |t| d.store.proposals_by_task(t.0))
        .map_err(|e| e.to_string())?;
    let in_day = d
        .store
        .proposals_since(&tproj.id, now.saturating_sub(DAY_MS))
        .map_err(|e| e.to_string())?;
    cfg_here.caps.check(by_task, in_day)?;

    let severity = if f.blocks_task {
        Severity::Blocks
    } else {
        Severity::Normal
    };
    let proposed = cfg_here.policy.decide(Source::Agent, severity) == Disposition::Proposed;
    if tproj.self_cfg.is_some() {
        crate::selfguard::check_filing_as(d, tid, TrustSource::OwnerAgent)?;
    }
    let appended = d
        .emit(EventKind::TaskFiled {
            project: tid,
            title: title.clone(),
            body: compose_body(&f.body, &f.evidence, f.from_task),
            priority: Priority::Normal,
            trust: TrustSource::OwnerAgent,
            explored: f.explored,
            proposed,
            blockers: vec![],
            not_before: None,
        })
        .map_err(|e| e.to_string())?;
    let task = appended
        .effects
        .iter()
        .find_map(|e| {
            if let Effect::TaskCreated { task } = e {
                Some(*task)
            } else {
                None
            }
        })
        .ok_or("the task was not created")?;
    d.store
        .put_proposal(&ProposalRow {
            task: task.0,
            project: tproj.id.clone(),
            source: Source::Agent.name().into(),
            severity: severity.name().into(),
            route: route.name().into(),
            filed_by: f.from_task.map(|t| t.0),
            title,
            body: f.body.trim().to_string(),
            evidence: f.evidence.trim().to_string(),
            key: normalise(&f.title),
            status: "open".into(),
            reject_reason: String::new(),
            publish_state: "local".into(),
            access: String::new(),
            branch: String::new(),
            pr: None,
            pr_state: String::new(),
            at: now,
        })
        .map_err(|e| e.to_string())?;
    Ok(Filed {
        task,
        route: route.name(),
        proposed,
        project: tproj.id.clone(),
    })
}

fn route_of(
    cited: &[String],
    pipeline: impl Fn(&str) -> bool,
    project: impl Fn(&str) -> bool,
) -> Dest {
    route(cited, pipeline, project)
}

/// After a step ends: file what its agent proposed. A refusal is recorded (it shows with the
/// owner's interventions) and logged in the step's directory; nothing else is filed for it.
pub fn file_from_report(
    d: &Arc<Daemon>,
    task: TaskId,
    project: ProjectId,
    proposals: &[Value],
    dir: &Path,
) {
    for p in proposals {
        let text = |k: &str| p[k].as_str().unwrap_or_default().to_string();
        let f = Filing {
            from_task: Some(task),
            project,
            title: text("title"),
            body: text("body"),
            evidence: text("evidence"),
            blocks_task: p["blocks_task"].as_bool().unwrap_or(false),
            explored: p["explored"].as_bool().unwrap_or(false),
        };
        match file(d, &f) {
            Ok(done) => tracing::info!(
                task = task.0,
                filed = done.task.0,
                route = done.route,
                "proposal filed"
            ),
            Err(why) => {
                tracing::info!(task = task.0, %why, "proposal refused");
                crate::attention::intervene(
                    d,
                    "proposal_refused",
                    Some(task),
                    "pipeline",
                    &f.title,
                    &why,
                );
                use std::io::Write;
                if let Ok(mut out) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(dir.join("proposals-refused.txt"))
                {
                    let _ = writeln!(out, "{}: {why}", f.title);
                }
            }
        }
    }
}

// ---- the owner's actions -------------------------------------------------------------------

/// Record that the owner decided, whichever way they reached the task.
pub fn note_decision(d: &Arc<Daemon>, task: u64, approved: bool, reason: &str) {
    let _ = d.store.update_proposal(task, |p| {
        if p.status == "open" {
            p.status = if approved { "approved" } else { "rejected" }.into();
            p.reject_reason = if approved {
                String::new()
            } else {
                reason.to_string()
            };
            if !approved {
                p.at = now_ms();
            }
        }
    });
    crate::improve::carry_decided(d, task, approved);
}

/// Change a proposal's title, body or evidence while it waits for the owner. Nothing else
/// changes, and it is refused once the task has left `proposed`.
pub fn edit(
    d: &Arc<Daemon>,
    task: u64,
    title: Option<&str>,
    body: Option<&str>,
    evidence: Option<&str>,
) -> Result<(), (u16, String)> {
    let row = d.store.proposal(task).ok().flatten();
    let w = d.world().map_err(|e| (500, e.to_string()))?;
    let t = w
        .tasks
        .get(&TaskId(task))
        .ok_or((404, format!("no task #{task}")))?;
    if t.state != State::Proposed {
        return Err((409, format!("task #{task} has started or is decided ({:?}); only a task waiting for you can be edited", t.state)));
    }
    let title = title.map_or_else(|| t.title.clone(), |s| s.trim().to_string());
    if title.is_empty() || title.chars().count() > 120 {
        return Err((400, "title must be 1 to 120 characters".into()));
    }
    let Some(row) = row else {
        // Not a proposal (the owner filed it as proposed): only its text can change.
        if evidence.is_some() {
            return Err((400, format!("task #{task} has no evidence to edit")));
        }
        let body = body.map_or_else(|| t.body.clone(), |s| s.trim().to_string());
        d.emit(EventKind::TrackerEdited {
            task: TaskId(task),
            title,
            body,
            priority: t.priority,
        })
        .map_err(|e| (409, e.to_string()))?;
        return Ok(());
    };
    let body = body.map_or(row.body.clone(), |s| s.trim().to_string());
    let evidence = evidence.map_or(row.evidence.clone(), |s| s.trim().to_string());
    if body.is_empty() {
        return Err((400, "body must not be empty".into()));
    }
    if row.source != Source::Owner.name() {
        evidence_ok(&evidence).map_err(|e| (400, e))?;
    }
    d.emit(EventKind::TrackerEdited {
        task: TaskId(task),
        title: title.clone(),
        body: compose_body(&body, &evidence, row.filed_by.map(TaskId)),
        priority: t.priority,
    })
    .map_err(|e| (409, e.to_string()))?;
    let _ = d.store.update_proposal(task, |p| {
        p.key = normalise(&title);
        p.title = title;
        p.body = body;
        p.evidence = evidence;
        // What the owner previewed is no longer what would be sent.
        if p.publish_state == "previewed" {
            p.publish_state = "local".into();
        }
    });
    Ok(())
}

/// The proposals as the API shows them, newest first.
pub fn list_json(d: &Daemon, open_only: bool) -> Vec<Value> {
    let Ok(w) = d.world() else { return vec![] };
    let mut rows = d.store.proposals().unwrap_or_default();
    rows.reverse();
    rows.iter()
        .filter(|r| !open_only || r.status == "open")
        .map(|r| row_json(&w, r))
        .collect()
}

pub fn row_json(w: &mason_engine::World, r: &ProposalRow) -> Value {
    let state = w
        .tasks
        .get(&TaskId(r.task))
        .map_or("unknown".to_string(), |t| {
            mason_store::state_name(t.state).to_string()
        });
    json!({
        "task": r.task, "project": r.project, "source": r.source, "severity": r.severity,
        "route": r.route, "filed_by": r.filed_by, "title": r.title, "body": r.body,
        "evidence": r.evidence, "status": r.status, "reject_reason": r.reject_reason,
        "publish_state": r.publish_state, "access": r.access, "branch": r.branch,
        "pr": r.pr, "pr_state": r.pr_state, "state": state, "at": r.at,
    })
}

/// A short tag for a task row: `agent · blocks · pipeline`.
pub fn tag(r: &ProposalRow) -> String {
    format!("{} · {} · {}", r.source, r.severity, r.route)
}
