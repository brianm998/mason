// SPDX-License-Identifier: GPL-3.0-or-later

//! The graphical editor, daemon side (Part 18). Each project may have a draft: the
//! three files an edit can change (the pipeline, how it lands, the canvas layout)
//! with a history for undo and redo. Clients send small operations; the engine in
//! `mason_definition::editor` applies them to the text with `toml_edit` and refuses
//! those that make things worse; this module keeps the draft, builds the model a
//! canvas draws (and the simulation, the diff and what the change affects), and
//! applies a finished draft by committing it to the project's base branch (or
//! opening a pull request where the landing mode uses them).

use crate::config::ProjectCfg;
use crate::daemon::now_ms;
use crate::Daemon;
use mason_core::*;
use mason_definition::builtin::BUILTIN;
use mason_definition::dryrun::{dry_run, default_scenarios, End, Scenario};
use mason_definition::editor::{self as ed, Edge, Files, Op};
use mason_definition::graph::Outcome;
use mason_definition::{parse, Capabilities};
use mason_landing::{edit as landing_edit, git};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

pub struct DraftState {
    base: Files,
    history: Vec<Files>,
    cursor: usize,
    last_error: Vec<String>,
    simulation: Option<Value>,
}

impl DraftState {
    fn new(base: Files) -> DraftState {
        DraftState { history: vec![base.clone()], base, cursor: 0, last_error: vec![], simulation: None }
    }

    pub fn current(&self) -> &Files {
        &self.history[self.cursor]
    }

    fn dirty(&self) -> bool {
        self.current() != &self.base
    }

    fn push(&mut self, f: Files) {
        if &f == self.current() {
            return;
        }
        self.history.truncate(self.cursor + 1);
        self.history.push(f);
        self.cursor += 1;
        self.simulation = None;
    }
}

#[derive(Default)]
pub struct Drafts(Mutex<HashMap<ProjectId, DraftState>>);

/// What the base branch holds now: the pipeline (the built-in one if the project has
/// none), how it lands, and the canvas layout.
pub fn base_files(d: &Daemon, p: &ProjectCfg, pid: ProjectId) -> Files {
    let show = |path: &str| {
        let spec = format!("{}:{path}", p.base_ref());
        if git::ok(&p.path, &["cat-file", "-e", &spec]) {
            git::run(&p.path, &["show", &spec]).ok()
        } else {
            None
        }
    };
    let definition = match d.current_origin(pid) {
        crate::defs::Origin::Builtin => BUILTIN.to_string(),
        crate::defs::Origin::Project { .. } => show(".mason/mason.toml").unwrap_or_else(|| BUILTIN.to_string()),
    };
    Files { definition, landing: show(".mason/landing.toml").unwrap_or_default(), layout: show(".mason/layout.toml").unwrap_or_default() }
}

impl Daemon {
    /// Run `f` on the project's draft, making it from the base if there is none. A
    /// draft nobody has changed follows the base when the base moves.
    pub fn with_draft<T>(&self, pid: ProjectId, f: impl FnOnce(&mut DraftState) -> T) -> Option<T> {
        let p = self.project(pid)?;
        let fresh = base_files(self, p, pid);
        let mut m = self.drafts.0.lock().unwrap_or_else(|e| e.into_inner());
        let st = m.entry(pid).or_insert_with(|| DraftState::new(fresh.clone()));
        if !st.dirty() && st.base != fresh {
            *st = DraftState::new(fresh);
        }
        Some(f(st))
    }
}

fn s(v: &Value, k: &str) -> Result<String, String> {
    v[k].as_str().map(String::from).ok_or_else(|| format!("`{k}` is required"))
}

fn edge(v: &Value) -> Result<Edge, String> {
    match v["edge"].as_str() {
        Some("next") => Ok(Edge::Next),
        Some("on_red") => Ok(Edge::OnRed),
        _ => Err("`edge` is \"next\" or \"on_red\"".into()),
    }
}

enum Parsed {
    Pipeline(Op),
    Landing(landing_edit::Op),
}

fn parse_op(v: &Value) -> Result<Parsed, String> {
    let op = v["op"].as_str().ok_or("`op` is required")?;
    Ok(match op {
        "insert_step" => Parsed::Pipeline(Op::InsertStep {
            after: s(v, "after")?,
            name: s(v, "name")?,
            kind: s(v, "type")?,
            category: s(v, "category")?,
            extends: v["extends"].as_str().filter(|x| !x.is_empty()).map(String::from),
            fields: v["fields"].as_object().map(|o| o.iter().map(|(k, x)| (k.clone(), x.clone())).collect()).unwrap_or_default(),
        }),
        "remove_step" => Parsed::Pipeline(Op::RemoveStep { name: s(v, "name")? }),
        "set_edge" => Parsed::Pipeline(Op::SetEdge { from: s(v, "from")?, edge: edge(v)?, to: v["to"].as_str().filter(|x| !x.is_empty()).map(String::from) }),
        "add_next" => Parsed::Pipeline(Op::AddNext { from: s(v, "from")?, to: s(v, "to")? }),
        "set_field" => Parsed::Pipeline(Op::SetField { step: s(v, "step")?, key: s(v, "key")?, value: v["value"].clone() }),
        "unset_field" => Parsed::Pipeline(Op::UnsetField { step: s(v, "step")?, key: s(v, "key")? }),
        "rename_step" => Parsed::Pipeline(Op::RenameStep { from: s(v, "from")?, to: s(v, "to")? }),
        "set_start" => Parsed::Pipeline(Op::SetStart { name: s(v, "name")? }),
        "set_binding" => Parsed::Pipeline(Op::SetBinding { level: s(v, "level")?, when: s(v, "when")?, on: v["on"].as_bool().ok_or("`on` is true or false")? }),
        "set_suite_level" => Parsed::Pipeline(Op::SetSuiteLevel { suite: s(v, "suite")?, level: s(v, "level")? }),
        "move" => Parsed::Pipeline(Op::Move { step: s(v, "step")?, x: v["x"].as_i64().ok_or("`x` is a whole number")?, y: v["y"].as_i64().ok_or("`y` is a whole number")? }),
        "landing_set" => Parsed::Landing(landing_edit::Op::Set { key: s(v, "key")?, value: v["value"].clone() }),
        "landing_unset" => Parsed::Landing(landing_edit::Op::Unset { key: s(v, "key")? }),
        "landing_add_tier" => Parsed::Landing(landing_edit::Op::AddTier {
            name: s(v, "name")?,
            suites: v["suites"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default(),
        }),
        "landing_remove_tier" => Parsed::Landing(landing_edit::Op::RemoveTier { name: s(v, "name")? }),
        "landing_set_tier" => Parsed::Landing(landing_edit::Op::SetTier { name: s(v, "name")?, key: s(v, "key")?, value: v["value"].clone() }),
        "landing_set_ladder" => Parsed::Landing(landing_edit::Op::SetLadder {
            rungs: v["rungs"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default(),
        }),
        other => return Err(format!("unknown operation `{other}`")),
    })
}

/// One operation on the project's draft. `Err` is why it was refused (the draft is
/// unchanged, and the reasons are remembered for the view).
pub fn operate(d: &Daemon, pid: ProjectId, op: &Value) -> Result<(), Vec<String>> {
    let parsed = parse_op(op).map_err(|e| vec![e])?;
    d.with_draft(pid, |st| {
        let cur = st.current().clone();
        let r: Result<Files, Vec<String>> = match &parsed {
            Parsed::Pipeline(o) => ed::apply(o, &cur, &Capabilities::now()).map_err(|ps| ps.iter().map(|p| if p.path.is_empty() { p.message.clone() } else { format!("{}: {}", p.path, p.message) }).collect()),
            Parsed::Landing(o) => landing_edit::apply(o, &cur.landing).map(|t| Files { landing: t, ..cur.clone() }),
        };
        match r {
            Ok(f) => {
                st.last_error.clear();
                st.push(f);
                Ok(())
            }
            Err(e) => {
                st.last_error = e.clone();
                Err(e)
            }
        }
    })
    .unwrap_or_else(|| Err(vec!["no such project".into()]))
}

pub fn undo(d: &Daemon, pid: ProjectId) -> bool {
    d.with_draft(pid, |st| {
        st.last_error.clear();
        if st.cursor > 0 {
            st.cursor -= 1;
            st.simulation = None;
            true
        } else {
            false
        }
    })
    .unwrap_or(false)
}

pub fn redo(d: &Daemon, pid: ProjectId) -> bool {
    d.with_draft(pid, |st| {
        st.last_error.clear();
        if st.cursor + 1 < st.history.len() {
            st.cursor += 1;
            st.simulation = None;
            true
        } else {
            false
        }
    })
    .unwrap_or(false)
}

pub fn discard(d: &Daemon, pid: ProjectId) {
    if let Some(p) = d.project(pid) {
        let fresh = base_files(d, p, pid);
        d.drafts.0.lock().unwrap_or_else(|e| e.into_inner()).insert(pid, DraftState::new(fresh));
    }
}

fn outcome(s: &str) -> Result<Outcome, String> {
    Outcome::from_name(s).ok_or_else(|| format!("`{s}` is not an outcome; use green, red or fail"))
}

/// A scenario from JSON: `{"name", "explored", "clear", "results": {"test": ["red", "green"]}}`.
fn scenario(v: &Value) -> Result<Scenario, String> {
    let mut sc = Scenario { name: v["name"].as_str().unwrap_or("your scenario").into(), explored: v["explored"].as_bool().unwrap_or(false), clear: v["clear"].as_bool().unwrap_or(false), ..Default::default() };
    if let Some(r) = v["results"].as_object() {
        for (step, list) in r {
            let mut q = VecDeque::new();
            for x in list.as_array().ok_or_else(|| format!("results.{step} is a list"))? {
                q.push_back(outcome(x.as_str().unwrap_or(""))?);
            }
            sc.results.insert(step.clone(), q);
        }
    }
    Ok(sc)
}

/// Walk scripted tasks through the draft: the one asked for, or the standard set. A
/// draft that does not parse cannot be simulated (the reasons are the problems).
pub fn simulate(d: &Daemon, pid: ProjectId, custom: Option<&Value>) -> Result<Value, String> {
    let files = d.with_draft(pid, |st| st.current().clone()).ok_or("no such project")?;
    let def = parse(&files.definition).map_err(|ps| ps.iter().map(|p| p.message.clone()).collect::<Vec<_>>().join("; "))?;
    let scenarios = match custom {
        Some(v) => vec![scenario(v)?],
        None => default_scenarios(&def),
    };
    let out: Vec<Value> = scenarios
        .iter()
        .map(|sc| match dry_run(&def, sc) {
            Ok(t) => json!({
                "name": sc.name,
                "route": format!("{:?}", t.route).to_lowercase(),
                "steps": t.steps.iter().map(|s| json!({"step": s.step, "outcome": s.outcome.name()})).collect::<Vec<_>>(),
                "end": match &t.end { End::Landed => "landed".to_string(), End::Failed(w) => format!("failed: {w}") },
            }),
            Err(e) => json!({"name": sc.name, "error": e}),
        })
        .collect();
    let v = json!({"scenarios": out});
    d.with_draft(pid, |st| st.simulation = Some(v.clone()));
    Ok(v)
}

/// Tasks that have started and not finished, grouped by the step they are in, for
/// "2 tasks in this step will finish on the old version".
fn tasks_by_step(d: &Daemon, pid: ProjectId) -> HashMap<String, u32> {
    let mut m = HashMap::new();
    if let Ok(w) = d.world() {
        for t in w.tasks.values().filter(|t| t.project == pid && !t.def.is_empty()) {
            if matches!(t.state, State::Landed | State::Failed | State::Canceled) {
                continue;
            }
            if let Some(run) = w.open_run(t.id) {
                *m.entry(w.step_runs[&run].name.clone()).or_default() += 1;
            }
        }
    }
    m
}

/// Everything a canvas draws, as JSON (the protocol view and the HTTP answer are both
/// made from it).
pub fn model(d: &Daemon, pid: ProjectId, selected: &str) -> Value {
    let Some(p) = d.project(pid) else { return json!({"error": "no such project"}) };
    let caps = Capabilities::now();
    let (files, base, can_undo, can_redo, dirty, last_error, simulation) = d
        .with_draft(pid, |st| (st.current().clone(), st.base.clone(), st.cursor > 0, st.cursor + 1 < st.history.len(), st.dirty(), st.last_error.clone(), st.simulation.clone()))
        .expect("project exists");
    let pic = ed::picture(&files, &caps);
    let def = parse(&files.definition).ok();
    let old = parse(&base.definition).ok();
    let kind = def.as_ref().and_then(|d| d.step(selected)).map(|s| s.kind);
    let inspector = match (&def, kind) {
        (Some(def), Some(k)) => json!({
            "step": selected,
            "kind": k.name(),
            "schema": ed::step_schema(k, def),
            "values": ed::step_values(&files.definition, selected),
            "extends": ed::step_values(&files.definition, selected).get("extends").cloned(),
        }),
        _ => Value::Null,
    };
    let mut diff = String::new();
    for (name, a, b) in [(".mason/mason.toml", &base.definition, &files.definition), (".mason/landing.toml", &base.landing, &files.landing), (".mason/layout.toml", &base.layout, &files.layout)] {
        diff.push_str(&ed::unified_diff(name, a, b));
    }
    let mut changes = match (&old, &def) {
        (Some(o), Some(n)) => ed::changes(o, n),
        _ => vec![],
    };
    if files.landing != base.landing {
        changes.push("changes how the project lands".into());
    }
    let by_step = tasks_by_step(d, pid);
    let affected: Vec<String> = match (&old, &def) {
        (Some(o), Some(n)) => {
            let mut v = vec![];
            for st in &o.steps {
                let touched = n.step(&st.name).is_none_or(|x| x != st);
                if let (true, Some(c)) = (touched, by_step.get(&st.name)) {
                    v.push(format!("{c} task(s) in `{}` will finish on the old version", st.name));
                }
            }
            v
        }
        _ => vec![],
    };
    let protected = crate::selfguard::is_self(p);
    json!({
        "project": p.id,
        "lanes": pic.lanes.iter().map(|(c, t)| json!({"category": c, "title": t})).collect::<Vec<_>>(),
        "nodes": pic.nodes.iter().map(|n| json!({
            "name": n.name, "kind": n.kind, "category": n.category, "lane": n.lane, "x": n.x, "y": n.y,
            "placed": n.placed, "problems": n.problems, "summary": n.summary, "is_start": n.is_start,
        })).collect::<Vec<_>>(),
        "edges": pic.edges.iter().map(|e| json!({"from": e.from, "to": e.to, "kind": e.kind, "bound": e.bound})).collect::<Vec<_>>(),
        "problems": pic.general.iter().map(|p| if p.path.is_empty() { p.message.clone() } else { format!("{}: {}", p.path, p.message) }).collect::<Vec<_>>(),
        "grid": {
            "columns": pic.grid.columns,
            "levels": pic.grid.levels,
            "rows": pic.grid.rows.iter().map(|r| json!({"suite": r.suite, "level": r.level, "cells": r.cells})).collect::<Vec<_>>(),
        },
        "landing": landing_edit::form(&files.landing),
        "inspector": inspector,
        "palette": ed::palette(&caps),
        "steps": def.as_ref().map(|d| d.steps.iter().map(|s| s.name.clone()).collect::<Vec<_>>()).unwrap_or_default(),
        "suites": def.as_ref().map(|d| d.testing.suites.iter().map(|s| s.name.clone()).collect::<Vec<_>>()).unwrap_or_default(),
        "dirty": dirty,
        "can_undo": can_undo,
        "can_redo": can_redo,
        "diff": diff,
        "changes": changes,
        "affected": affected,
        "simulation": simulation,
        "last_error": last_error,
        "selected": selected,
        "protected": protected,
        "text": files.definition,
    })
}

/// Commit the draft. Only the owner may. Refused if the draft has problems, changes
/// nothing, or the base moved under the files being edited. In a direct mode it lands
/// on the base branch (fast-forward) and the definition reloads; where the landing mode
/// uses pull requests, a branch is pushed and a PR opened, and nothing changes until it
/// merges.
pub fn apply(d: &std::sync::Arc<Daemon>, pid: ProjectId, who: &str, message: &str) -> Result<Value, (u16, String)> {
    if !crate::attention::is_owner(d, who) {
        crate::attention::intervene(d, "refused_answer", None, who, "not the owner's identity (editor apply)", "");
        return Err((403, format!("{who:?} is not the owner's identity; only the owner can apply an edit to the pipeline")));
    }
    let p = d.project(pid).ok_or((404, "no such project".to_string()))?.clone();
    let (files, base) = d.with_draft(pid, |st| (st.current().clone(), st.base.clone())).ok_or((404, "no such project".to_string()))?;
    if files == base {
        return Err((409, "there is nothing to apply".into()));
    }
    let problems = ed::problems_of(&files.definition, &Capabilities::now());
    if !problems.is_empty() {
        return Err((422, format!("the draft still has problems: {}", problems.iter().map(|p| p.message.clone()).collect::<Vec<_>>().join("; "))));
    }
    if let Err(e) = mason_landing::config::parse(&files.landing) {
        return Err((422, format!("the landing settings have problems: {}", e.join("; "))));
    }
    let cfg = crate::landing::load_config(d, pid).ok();
    let (remote, uses_pr) = match &cfg {
        Some(c) => (if c.mode.local_only() { None } else { p.remote.clone() }, c.mode.uses_pr()),
        None => (p.remote.clone(), false),
    };
    let lock = d.repo_lock(pid);
    let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(r) = &remote {
        let _ = git::run(&p.path, &["fetch", "--prune", r]);
    }
    // The base must still hold what the draft was made from.
    let now = base_files(d, &p, pid);
    if now.landing != base.landing || now.layout != base.layout || (now.definition != base.definition && !matches!(d.current_origin(pid), crate::defs::Origin::Builtin)) {
        return Err((409, "the pipeline changed on the base branch since this draft was started; discard the draft and start again".into()));
    }
    let base_ref = match &remote {
        Some(r) => format!("{r}/{}", p.base),
        None => p.base.clone(),
    };
    let tip = git::rev_parse(&p.path, &base_ref).map_err(|e| (500, e.to_string()))?;
    let wt = d.cfg.landing_dir().join(format!("{}-edit", p.id));
    git::remove_worktree(&p.path, &wt);
    git::run(&p.path, &["worktree", "add", "--detach", &wt.display().to_string(), &tip]).map_err(|e| (500, e.to_string()))?;
    let result = (|| -> Result<Value, String> {
        std::fs::create_dir_all(wt.join(".mason")).map_err(|e| e.to_string())?;
        for (path, text) in [(".mason/mason.toml", &files.definition), (".mason/landing.toml", &files.landing), (".mason/layout.toml", &files.layout)] {
            if text.is_empty() && !wt.join(path).exists() {
                continue;
            }
            std::fs::write(wt.join(path), text).map_err(|e| e.to_string())?;
        }
        git::run(&wt, &["add", "-A", ".mason"]).map_err(|e| e.to_string())?;
        let msg = if message.trim().is_empty() { "Edit the pipeline".to_string() } else { message.trim().to_string() };
        let (name, email) = (format!("user.name={who}"), format!("user.email={who}@localhost"));
        git::run(&wt, &["-c", &name, "-c", &email, "commit", "-q", "-m", &format!("{msg}\n\nEdited in the MASON monitor by {who}.")]).map_err(|e| e.to_string())?;
        let commit = git::rev_parse(&wt, "HEAD").map_err(|e| e.to_string())?;
        if uses_pr {
            let r = p.remote.clone().ok_or("pull requests need a remote")?;
            let host = crate::services::host(&p)?.ok_or("the landing mode uses pull requests but the project has no [host]")?;
            let branch = format!("mason/edit/{}-{}", p.id, now_ms());
            git::run(&wt, &["push", "--porcelain", &r, &format!("{commit}:refs/heads/{branch}")]).map_err(|e| e.to_string())?;
            let pr = host.open_pr(&p.base, &branch, &msg, "A pipeline edit from the MASON monitor.").map_err(|e| e.to_string())?;
            return Ok(json!({"applied": false, "pull_request": pr.url, "number": pr.number, "note": "a pull request was opened; the pipeline changes when it merges"}));
        }
        let mut tries = 0;
        loop {
            match mason_landing::promote::promote(&p.path, &wt, remote.as_deref(), &p.base, &base_ref, &commit) {
                Ok(()) => break,
                Err(mason_landing::promote::PromoteError::Moved) if tries < 2 => {
                    tries += 1;
                    return Err("the base branch moved while applying; try again".into());
                }
                Err(mason_landing::promote::PromoteError::LocalBaseCheckedOut(path)) => {
                    return Err(format!("the base branch is checked out at {}; it cannot be updated under you", path.display()))
                }
                Err(e) => return Err(format!("{e:?}")),
            }
        }
        Ok(json!({"applied": true, "commit": commit}))
    })();
    git::remove_worktree(&p.path, &wt);
    drop(_g);
    let v = result.map_err(|e| (500, e))?;
    crate::attention::intervene(d, "editor_apply", None, who, message, "");
    if v["applied"] == true {
        d.reload_definition(pid);
    }
    discard(d, pid);
    d.poke();
    Ok(v)
}
