// SPDX-License-Identifier: GPL-3.0-or-later

//! The graphical editor's engine (Part 18), with no screen in it. Clients send
//! small operations ("insert a step after `develop`", "connect `test` on red to
//! `develop`", "set `max_rounds`"); this applies them to the text of the
//! definition with `toml_edit` (so comments and ordering survive), refuses any
//! that make the definition worse, and computes everything a canvas needs: the
//! lanes, nodes and edges with a layout, the problems on each node, the test
//! grid, an inspector form's schema and values, the diff and what the change
//! affects.

use crate::model::*;
use crate::validate::{validate, Capabilities};
use mason_core::State;
use mason_plugins::PluginType;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use toml_edit::{value, Array, DocumentMut, InlineTable, Item, Table};

/// The three files an edit may change: the pipeline, how it lands, and where
/// the person put things on the canvas.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Files {
    pub definition: String,
    pub landing: String,
    pub layout: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    /// On success: where the step goes next.
    Next,
    /// On red: the step to go back to.
    OnRed,
}

impl Edge {
    pub fn key(self) -> &'static str {
        match self {
            Edge::Next => "next",
            Edge::OnRed => "on_red",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    /// A new step after `after`, taking over what `after` led to. `extends` copies
    /// another step's settings.
    InsertStep {
        after: String,
        name: String,
        kind: String,
        category: String,
        extends: Option<String>,
        fields: Vec<(String, Value)>,
    },
    /// Remove a step; what led to it leads to what it led to.
    RemoveStep {
        name: String,
    },
    /// Make `from`'s edge go to `to` alone (or nowhere).
    SetEdge {
        from: String,
        edge: Edge,
        to: Option<String>,
    },
    /// Another success edge (fan-out).
    AddNext {
        from: String,
        to: String,
    },
    SetField {
        step: String,
        key: String,
        value: Value,
    },
    UnsetField {
        step: String,
        key: String,
    },
    RenameStep {
        from: String,
        to: String,
    },
    SetStart {
        name: String,
    },
    /// Run (or stop running) a level at a point: the test grid.
    SetBinding {
        level: String,
        when: String,
        on: bool,
    },
    SetSuiteLevel {
        suite: String,
        level: String,
    },
    /// Where a node sits on the canvas (the sidecar, not the pipeline).
    Move {
        step: String,
        x: i64,
        y: i64,
    },
}

fn doc(text: &str) -> Result<DocumentMut, String> {
    text.parse()
        .map_err(|e: toml_edit::TomlError| format!("not valid TOML: {e}"))
}

fn good_name(n: &str) -> bool {
    n.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && n.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

fn steps_mut(d: &mut DocumentMut) -> Result<&mut dyn toml_edit::TableLike, String> {
    d.get_mut("step")
        .and_then(|s| s.as_table_like_mut())
        .ok_or_else(|| "the definition has no steps".to_string())
}

fn step_mut<'a>(
    d: &'a mut DocumentMut,
    name: &str,
) -> Result<&'a mut dyn toml_edit::TableLike, String> {
    steps_mut(d)?
        .get_mut(name)
        .and_then(|s| s.as_table_like_mut())
        .ok_or_else(|| format!("there is no step `{name}`"))
}

fn list_of(item: Option<&Item>) -> Vec<String> {
    match item {
        Some(i) => match i.as_str() {
            Some(s) => vec![s.to_string()],
            None => i
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
        },
        None => vec![],
    }
}

/// Put `item` at `key`, keeping the comment and spacing that were on the old value.
fn put(t: &mut dyn toml_edit::TableLike, key: &str, mut item: Item) {
    if let Some(old) = t.get_mut(key) {
        if let (Some(o), Some(n)) = (old.as_value(), item.as_value_mut()) {
            *n.decor_mut() = o.decor().clone();
        }
        *old = item;
        return;
    }
    t.insert(key, item);
}

fn set_list(t: &mut dyn toml_edit::TableLike, key: &str, items: &[String]) {
    match items.len() {
        0 => {
            t.remove(key);
        }
        1 => put(t, key, value(items[0].as_str())),
        _ => {
            let mut a = Array::new();
            for s in items {
                a.push(s.as_str());
            }
            put(t, key, value(a));
        }
    }
}

fn put_top(d: &mut DocumentMut, key: &str, v: &str) {
    put(d.as_table_mut(), key, value(v));
}

fn json_item(v: &Value) -> Result<Item, String> {
    Ok(match v {
        Value::String(s) => value(s.as_str()),
        Value::Bool(b) => value(*b),
        Value::Number(n) => value(n.as_i64().ok_or("numbers here are whole numbers")?),
        Value::Array(a) => {
            let mut arr = Array::new();
            for x in a {
                arr.push(x.as_str().ok_or("lists here hold text")?);
            }
            value(arr)
        }
        Value::Object(o) => {
            let mut t = InlineTable::new();
            for (k, x) in o {
                t.insert(
                    k,
                    x.as_i64()
                        .ok_or("a table here maps names to whole numbers")?
                        .into(),
                );
            }
            value(t)
        }
        Value::Null => return Err("null unsets a key; use UnsetField".into()),
    })
}

fn layout_doc(text: &str) -> Result<DocumentMut, String> {
    if text.trim().is_empty() {
        Ok(
            "# Where the editor put things on the canvas. Safe to delete.\n"
                .parse()
                .expect("valid"),
        )
    } else {
        doc(text)
    }
}

fn apply_to_text(op: &Op, f: &Files) -> Result<Files, String> {
    let mut out = f.clone();
    match op {
        Op::Move { step, x, y } => {
            let mut l = layout_doc(&f.layout)?;
            let nodes = l.entry("node").or_insert_with(|| {
                let mut t = Table::new();
                t.set_implicit(true);
                Item::Table(t)
            });
            let nodes = nodes.as_table_mut().ok_or("`node` is not a table")?;
            let mut t = Table::new();
            t.insert("x", value(*x));
            t.insert("y", value(*y));
            nodes.insert(step, Item::Table(t));
            out.layout = l.to_string();
            return Ok(out);
        }
        Op::SetBinding { level, when, on } => {
            if mason_testing::When::from_name(when).is_none() {
                return Err(format!("unknown `{when}`; use development, test_step, batch, after_landing or schedule"));
            }
            let mut d = doc(&f.definition)?;
            let b = d.entry("binding").or_insert_with(|| {
                let mut t = Table::new();
                t.set_implicit(true);
                Item::Table(t)
            });
            let b = b.as_table_mut().ok_or("`binding` is not a table")?;
            let mut whens = list_of(b.get(level).and_then(|t| t.get("when")));
            whens.retain(|w| w != when);
            if *on {
                whens.push(when.clone());
            }
            if whens.is_empty() {
                b.remove(level);
            } else {
                if b.get(level).is_none() {
                    b.insert(level, Item::Table(Table::new()));
                }
                let mut a = Array::new();
                for w in &whens {
                    a.push(w.as_str());
                }
                b.get_mut(level)
                    .and_then(|t| t.as_table_like_mut())
                    .ok_or("not a table")?
                    .insert("when", value(a));
            }
            out.definition = d.to_string();
            return Ok(out);
        }
        Op::SetSuiteLevel { suite, level } => {
            let mut d = doc(&f.definition)?;
            let s = d
                .get_mut("suite")
                .and_then(|s| s.get_mut(suite))
                .and_then(|s| s.as_table_like_mut())
                .ok_or_else(|| format!("there is no suite `{suite}`"))?;
            s.insert("level", value(level.as_str()));
            out.definition = d.to_string();
            return Ok(out);
        }
        _ => {}
    }
    let mut d = doc(&f.definition)?;
    match op {
        Op::InsertStep {
            after,
            name,
            kind,
            category,
            extends,
            fields,
        } => {
            if !good_name(name) {
                return Err(
                    "step names are lowercase letters, digits, - and _, starting with a letter"
                        .into(),
                );
            }
            let steps = steps_mut(&mut d)?;
            if steps.contains_key(name) {
                return Err(format!("there is already a step `{name}`"));
            }
            let after_next = list_of(
                steps
                    .get(after)
                    .and_then(|s| s.as_table_like())
                    .ok_or_else(|| format!("there is no step `{after}`"))?
                    .get("next"),
            );
            let mut t = Table::new();
            match extends {
                Some(parent) => {
                    if !steps.contains_key(parent) {
                        return Err(format!("there is no step `{parent}` to extend"));
                    }
                    t.insert("extends", value(parent.as_str()));
                    t.insert("type", value(kind.as_str()));
                    t.insert("category", value(category.as_str()));
                }
                None => {
                    t.insert("type", value(kind.as_str()));
                    t.insert("category", value(category.as_str()));
                }
            }
            for (k, v) in fields {
                if matches!(
                    k.as_str(),
                    "next" | "on_red" | "type" | "category" | "extends"
                ) {
                    return Err(format!("`{k}` is not set that way"));
                }
                t.insert(k, json_item(v)?);
            }
            if !after_next.is_empty() {
                set_list(&mut t, "next", &after_next);
            }
            let ts = d
                .get_mut("step")
                .and_then(|s| s.as_table_mut())
                .ok_or("`step` is not a table")?;
            ts.insert(name, Item::Table(t));
            let a = step_mut(&mut d, after)?;
            set_list(a, "next", std::slice::from_ref(name));
        }
        Op::RemoveStep { name } => {
            let steps = steps_mut(&mut d)?;
            let gone = steps
                .remove(name)
                .ok_or_else(|| format!("there is no step `{name}`"))?;
            let onward = list_of(gone.as_table_like().and_then(|t| t.get("next")));
            let names: Vec<String> = steps.iter().map(|(k, _)| k.to_string()).collect();
            for n in names {
                let t = steps
                    .get_mut(&n)
                    .and_then(|s| s.as_table_like_mut())
                    .ok_or("not a table")?;
                let next = list_of(t.get("next"));
                if next.contains(name) {
                    let mut v: Vec<String> = vec![];
                    for x in next {
                        if &x == name {
                            for o in &onward {
                                if !v.contains(o) {
                                    v.push(o.clone());
                                }
                            }
                        } else if !v.contains(&x) {
                            v.push(x);
                        }
                    }
                    set_list(t, "next", &v);
                }
                if t.get("on_red").and_then(|v| v.as_str()) == Some(name) {
                    t.remove("on_red");
                }
                if t.get("extends").and_then(|v| v.as_str()) == Some(name) {
                    return Err(format!("step `{n}` extends `{name}`; change that first"));
                }
            }
            if d.get("start").and_then(|v| v.as_str()) == Some(name) {
                match onward.first() {
                    Some(o) => put_top(&mut d, "start", o),
                    None => return Err("that is the only way in, and it leads nowhere".into()),
                }
            }
            if d.get("explored_entry").and_then(|v| v.as_str()) == Some(name) {
                d.remove("explored_entry");
            }
        }
        Op::SetEdge { from, edge, to } => {
            let t = step_mut(&mut d, from)?;
            match to {
                Some(to) => set_list(t, edge.key(), std::slice::from_ref(to)),
                None => {
                    t.remove(edge.key());
                }
            }
        }
        Op::AddNext { from, to } => {
            let t = step_mut(&mut d, from)?;
            let mut v = list_of(t.get("next"));
            if !v.contains(to) {
                v.push(to.clone());
            }
            set_list(t, "next", &v);
        }
        Op::SetField {
            step,
            key,
            value: v,
        } => {
            if matches!(key.as_str(), "next" | "on_red") {
                return Err(format!("`{key}` is an edge: connect it on the canvas"));
            }
            if v.is_null() {
                step_mut(&mut d, step)?.remove(key);
            } else {
                put(step_mut(&mut d, step)?, key, json_item(v)?);
            }
        }
        Op::UnsetField { step, key } => {
            step_mut(&mut d, step)?.remove(key);
        }
        Op::RenameStep { from, to } => {
            if !good_name(to) {
                return Err(
                    "step names are lowercase letters, digits, - and _, starting with a letter"
                        .into(),
                );
            }
            let steps = steps_mut(&mut d)?;
            if steps.contains_key(to) {
                return Err(format!("there is already a step `{to}`"));
            }
            let item = steps
                .remove(from)
                .ok_or_else(|| format!("there is no step `{from}`"))?;
            let pos = item.as_table().and_then(|t| t.position());
            let mut item = item;
            if let (Some(p), Some(t)) = (pos, item.as_table_mut()) {
                t.set_position(Some(p));
            }
            steps.insert(to, item);
            let names: Vec<String> = steps.iter().map(|(k, _)| k.to_string()).collect();
            for n in names {
                let t = steps
                    .get_mut(&n)
                    .and_then(|s| s.as_table_like_mut())
                    .ok_or("not a table")?;
                let next: Vec<String> = list_of(t.get("next"))
                    .into_iter()
                    .map(|x| if &x == from { to.clone() } else { x })
                    .collect();
                if !next.is_empty() {
                    set_list(t, "next", &next);
                }
                if t.get("on_red").and_then(|v| v.as_str()) == Some(from) {
                    put(t, "on_red", value(to.as_str()));
                }
                if t.get("extends").and_then(|v| v.as_str()) == Some(from) {
                    put(t, "extends", value(to.as_str()));
                }
            }
            for key in ["start", "explored_entry"] {
                if d.get(key).and_then(|v| v.as_str()) == Some(from) {
                    put_top(&mut d, key, to);
                }
            }
            // The canvas position follows the step.
            if let Ok(mut l) = layout_doc(&f.layout) {
                if let Some(nodes) = l.get_mut("node").and_then(|n| n.as_table_mut()) {
                    if let Some(item) = nodes.remove(from) {
                        nodes.insert(to, item);
                        out.layout = l.to_string();
                    }
                }
            }
        }
        Op::SetStart { name } => put_top(&mut d, "start", name),
        _ => unreachable!("handled above"),
    }
    out.definition = d.to_string();
    // A removed step's canvas position goes with it.
    if let Op::RemoveStep { name } = op {
        if let Ok(mut l) = layout_doc(&f.layout) {
            if let Some(nodes) = l.get_mut("node").and_then(|n| n.as_table_mut()) {
                if nodes.remove(name).is_some() {
                    out.layout = l.to_string();
                }
            }
        }
    }
    Ok(out)
}

fn problem_keys(ps: &[Problem]) -> BTreeSet<(String, String)> {
    ps.iter()
        .map(|p| (p.path.clone(), p.message.clone()))
        .collect()
}

/// Problems of a definition text: parse problems if it will not parse, else validation's.
pub fn problems_of(text: &str, caps: &Capabilities) -> Vec<Problem> {
    match crate::parse_with(text, crate::Plugins::Named(&caps.plugin_names())) {
        Err(p) => p,
        Ok(def) => validate(&def, caps),
    }
}

/// Apply one operation. It is **refused** if the result does not parse, or has a
/// problem the text did not already have: an edit never makes things worse, though
/// a definition that already has problems can be edited toward fixing them.
pub fn apply(op: &Op, files: &Files, caps: &Capabilities) -> Result<Files, Vec<Problem>> {
    let new = apply_to_text(op, files).map_err(|e| vec![Problem::new("", e)])?;
    if new == *files {
        return Ok(new);
    }
    if new.definition != files.definition {
        let before = problem_keys(&problems_of(&files.definition, caps));
        let after = problems_of(&new.definition, caps);
        let fresh: Vec<Problem> = after
            .into_iter()
            .filter(|p| !before.contains(&(p.path.clone(), p.message.clone())))
            .collect();
        if !fresh.is_empty() {
            return Err(fresh);
        }
    }
    Ok(new)
}

// ---- the picture --------------------------------------------------------------------------------

pub const LANE_H: i64 = 140;
pub const COL_W: i64 = 200;
const MARGIN: i64 = 40;

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub name: String,
    pub kind: String,
    pub category: String,
    pub lane: u32,
    pub x: i64,
    pub y: i64,
    /// Placed by hand (it is in the sidecar).
    pub placed: bool,
    pub problems: Vec<String>,
    /// A short line: the agent, the suites, the command.
    pub summary: String,
    pub is_start: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EdgeView {
    pub from: String,
    pub to: String,
    /// `next` or `on_red`.
    pub kind: &'static str,
    /// For a loop back: its bound.
    pub bound: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GridRow {
    pub suite: String,
    pub level: String,
    /// One per column of [`Grid::columns`].
    pub cells: Vec<bool>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Grid {
    pub columns: Vec<String>,
    pub rows: Vec<GridRow>,
    pub levels: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Picture {
    pub lanes: Vec<(String, String)>,
    pub nodes: Vec<Node>,
    pub edges: Vec<EdgeView>,
    /// Problems that belong to no node (the whole definition, a suite, a binding).
    pub general: Vec<Problem>,
    pub grid: Grid,
    pub start: String,
}

const LANES: [(State, &str); 5] = [
    (State::Exploring, "Exploring"),
    (State::Developing, "Developing"),
    (State::Testing, "Testing"),
    (State::Review, "Review"),
    (State::Landing, "Landing"),
];

fn layout_positions(layout: &str) -> BTreeMap<String, (i64, i64)> {
    let mut m = BTreeMap::new();
    if let Ok(d) = layout.parse::<DocumentMut>() {
        if let Some(nodes) = d.get("node").and_then(|n| n.as_table_like()) {
            for (k, v) in nodes.iter() {
                if let (Some(x), Some(y)) = (
                    v.get("x").and_then(|x| x.as_integer()),
                    v.get("y").and_then(|y| y.as_integer()),
                ) {
                    m.insert(k.to_string(), (x, y));
                }
            }
        }
    }
    m
}

/// The picture of a draft: nodes in swimlanes, laid out by how far each is along
/// the graph (loops back do not count), unless the person placed them by hand.
pub fn picture(files: &Files, caps: &Capabilities) -> Picture {
    let problems = problems_of(&files.definition, caps);
    let Ok(def) = crate::parse_with(
        &files.definition,
        crate::Plugins::Named(&caps.plugin_names()),
    ) else {
        return Picture {
            lanes: LANES
                .iter()
                .map(|(s, t)| (category_name(*s).into(), (*t).into()))
                .collect(),
            nodes: vec![],
            edges: vec![],
            general: problems,
            grid: Grid::default(),
            start: String::new(),
        };
    };
    let placed = layout_positions(&files.layout);
    // Depth: breadth-first from the start along success edges.
    let mut depth: BTreeMap<String, usize> = BTreeMap::new();
    let mut q = VecDeque::new();
    if def.step(&def.start).is_some() {
        depth.insert(def.start.clone(), 0);
        q.push_back(def.start.clone());
    }
    while let Some(n) = q.pop_front() {
        let d = depth[&n];
        for nx in def.step(&n).map(|s| s.next.clone()).unwrap_or_default() {
            if def.step(&nx).is_some() && !depth.contains_key(&nx) {
                depth.insert(nx.clone(), d + 1);
                q.push_back(nx);
            }
        }
    }
    let mut nodes: Vec<Node> = vec![];
    let mut per_lane: BTreeMap<u32, usize> = BTreeMap::new();
    let mut order: Vec<&Step> = def.steps.iter().collect();
    order.sort_by_key(|s| {
        (
            depth.get(&s.name).copied().unwrap_or(usize::MAX),
            s.name.clone(),
        )
    });
    for s in order {
        let lane = LANES
            .iter()
            .position(|(c, _)| *c == s.category)
            .unwrap_or(0) as u32;
        let col = per_lane.entry(lane).or_insert(0);
        let (x, y, hand) = match placed.get(&s.name) {
            Some((x, y)) => (*x, *y, true),
            None => (
                MARGIN + *col as i64 * COL_W,
                MARGIN + lane as i64 * LANE_H,
                false,
            ),
        };
        *col += 1;
        let mut mine: Vec<String> = vec![];
        for p in &problems {
            if p.path == format!("step.{}", s.name)
                || p.path.starts_with(&format!("step.{}.", s.name))
            {
                mine.push(if p.path.contains('.') && p.path.matches('.').count() > 1 {
                    format!("{}: {}", p.path.rsplit('.').next().unwrap_or(""), p.message)
                } else {
                    p.message.clone()
                });
            }
        }
        let summary = match s.kind {
            StepType::Agent => s
                .agent
                .clone()
                .or_else(|| s.model.clone())
                .unwrap_or_default(),
            StepType::Test => s.suites.join(", "),
            StepType::Plugin => s
                .config
                .values()
                .filter_map(|v| v.as_str())
                .next()
                .unwrap_or_default()
                .to_string(),
            StepType::Command | StepType::Approval | StepType::Gate => s
                .command
                .clone()
                .or_else(|| s.policy.clone())
                .unwrap_or_default(),
            _ => String::new(),
        };
        nodes.push(Node {
            name: s.name.clone(),
            kind: s.type_name().into(),
            category: category_name(s.category).into(),
            lane,
            x,
            y,
            placed: hand,
            problems: mine,
            summary,
            is_start: s.name == def.start,
        });
    }
    let mut edges = vec![];
    for s in &def.steps {
        for n in &s.next {
            edges.push(EdgeView {
                from: s.name.clone(),
                to: n.clone(),
                kind: "next",
                bound: None,
            });
        }
        if let Some(r) = &s.on_red {
            edges.push(EdgeView {
                from: s.name.clone(),
                to: r.clone(),
                kind: "on_red",
                bound: s.max_rounds,
            });
        }
    }
    let general: Vec<Problem> = problems
        .iter()
        .filter(|p| {
            !def.steps.iter().any(|s| {
                p.path == format!("step.{}", s.name)
                    || p.path.starts_with(&format!("step.{}.", s.name))
            })
        })
        .cloned()
        .collect();
    Picture {
        lanes: LANES
            .iter()
            .map(|(s, t)| (category_name(*s).into(), (*t).into()))
            .collect(),
        nodes,
        edges,
        general,
        grid: grid_of(&def),
        start: def.start.clone(),
    }
}

const WHENS: [mason_testing::When; 5] = [
    mason_testing::When::Development,
    mason_testing::When::TestStep,
    mason_testing::When::Batch,
    mason_testing::When::AfterLanding,
    mason_testing::When::Schedule,
];

/// The test grid: a row per suite with its level, a column per point where a level
/// can run, and a cell checked when that suite's level is bound there. (With no
/// bindings at all, every suite runs in the test step, as the engine does.)
pub fn grid_of(def: &Definition) -> Grid {
    let t = &def.testing;
    let columns: Vec<String> = WHENS.iter().map(|w| w.name().to_string()).collect();
    let mut levels: Vec<String> = t.suites.iter().map(|s| s.level.clone()).collect();
    levels.extend(t.bindings.iter().map(|b| b.level.clone()));
    levels.sort();
    levels.dedup();
    let rows = t
        .suites
        .iter()
        .map(|s| GridRow {
            suite: s.name.clone(),
            level: s.level.clone(),
            cells: WHENS
                .iter()
                .map(|w| t.bound(*w).iter().any(|x| x.name == s.name))
                .collect(),
        })
        .collect();
    Grid {
        columns,
        rows,
        levels,
    }
}

// ---- palette, inspector -------------------------------------------------------------------------

/// What can be dragged from the palette: each built-in step type with the
/// categories it may sit in and what it is for.
pub fn palette(caps: &Capabilities) -> Vec<Value> {
    let mut entries: Vec<Value> = StepType::ALL
        .iter()
        .map(|k| {
            json!({
                "type": k.name(),
                "categories": k.categories().iter().map(|c| category_name(*c)).collect::<Vec<_>>(),
                "available": caps.step_types.contains(k),
                // What a new step of this type needs to be valid at once.
                "fields": match k {
                    StepType::Agent => json!({"agent": "light"}),
                    StepType::Command | StepType::Gate => json!({"command": "true"}),
                    _ => json!({}),
                },
                "about": match k {
                    StepType::Agent => "A coding agent works on the task.",
                    StepType::Command => "Run a command in the task's worktree.",
                    StepType::Test => "Run the suites bound to the test step.",
                    StepType::Gate => "A check from the gate library.",
                    StepType::Approval => "Wait for you to approve.",
                    StepType::Ci => "Run CI on the change.",
                    StepType::Pr => "Open a pull request.",
                    StepType::Landing => "Land the change.",
                    StepType::Plugin => "",
                },
            })
        })
        .collect();
    // A plugin's step type sits beside the built-in ones; its form comes from `inspector.schema`.
    for pt in &caps.plugins {
        entries.push(json!({
            "type": pt.name,
            "plugin": pt.plugin,
            "categories": pt.categories,
            "available": true,
            "fields": mason_plugins::schema::sample_config(&pt.config_schema),
            "about": pt.about,
            "schema": plugin_schema(pt, &Definition::empty()),
        }));
    }
    entries
}

/// A JSON Schema for a step of this kind in this definition, so a client can build its
/// form with no knowledge of the kind (a plugin's step type gets a form the same way).
pub fn step_schema(kind: StepType, def: &Definition) -> Value {
    let cats: Vec<&str> = kind
        .categories()
        .iter()
        .map(|c| category_name(*c))
        .collect();
    let mut props = serde_json::Map::new();
    props.insert("category".into(), json!({"type": "string", "title": "Category", "enum": cats, "description": "The swimlane: which stage of a task this step belongs to."}));
    props.insert("max_rounds".into(), json!({"type": "integer", "minimum": 1, "title": "Rounds", "description": "How many times this step may loop back to `on_red` before the task fails. Required when it has an on_red edge."}));
    let profiles: Vec<String> = def
        .agents
        .keys()
        .cloned()
        .chain(
            crate::builtin::builtin_profile_names()
                .iter()
                .filter(|s| **s != "fast")
                .map(|s| s.to_string()),
        )
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let suites: Vec<String> = def.testing.suites.iter().map(|s| s.name.clone()).collect();
    match kind {
        StepType::Agent => {
            props.insert(
                "agent".into(),
                json!({"type": "string", "title": "Agent profile", "enum": profiles}),
            );
            props.insert("model".into(), json!({"type": "string", "title": "Model", "description": "Overrides the profile's model."}));
            props.insert(
                "effort".into(),
                json!({"type": "string", "title": "Effort"}),
            );
            props.insert("budget".into(), json!({"type": "string", "title": "Time budget", "description": "Like 4h, 30m or 90s."}));
            props.insert("tools".into(), json!({"type": "array", "title": "Contract tools", "items": {"type": "string"}, "description": "Narrow the tools the step's contract offers."}));
            props.insert(
                "read_only".into(),
                json!({"type": "boolean", "title": "Read only"}),
            );
        }
        StepType::Command | StepType::Gate => {
            props.insert(
                "command".into(),
                json!({"type": "string", "title": "Command"}),
            );
        }
        StepType::Test => {
            props.insert("suites".into(), json!({"type": "array", "title": "Suites", "items": {"type": "string", "enum": suites}, "uniqueItems": true}));
            props.insert("command".into(), json!({"type": "string", "title": "Command", "description": "A bare command with an exit-code verdict, when there are no suites."}));
            props.insert("build".into(), json!({"type": "string", "title": "Build", "description": "Builds the tests first, holding one compile slot; the command then runs holding the step's resources without compile."}));
        }
        StepType::Approval => {
            props.insert(
                "policy".into(),
                json!({"type": "string", "title": "Policy"}),
            );
            props.insert(
                "command".into(),
                json!({"type": "string", "title": "Command"}),
            );
        }
        StepType::Ci | StepType::Pr | StepType::Landing | StepType::Plugin => {}
    }
    props.insert("uses".into(), json!({"type": "object", "title": "Resources", "additionalProperties": {"type": "integer", "minimum": 1}, "x-choices": def.resources.keys().collect::<Vec<_>>(), "description": "Resources the step holds while it runs, like compile = 1."}));
    // The form shows fields in a deliberate order, not the alphabetical one a JSON object keeps.
    let rank = |k: &str| {
        [
            "category",
            "agent",
            "model",
            "effort",
            "budget",
            "command",
            "build",
            "suites",
            "tools",
            "read_only",
            "policy",
            "max_rounds",
            "uses",
        ]
        .iter()
        .position(|x| *x == k)
        .unwrap_or(99)
    };
    let mut order: Vec<String> = props.keys().cloned().collect();
    order.sort_by_key(|k| rank(k));
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": format!("{} step", kind.name()),
        "type": "object",
        "properties": props,
        "x-order": order,
    })
}

/// The same, for a step of a type a plugin provides: the step's own keys, then the
/// settings the plugin's `config_schema` describes, so the form needs no client code.
pub fn plugin_schema(pt: &PluginType, def: &Definition) -> Value {
    let mut props = serde_json::Map::new();
    props.insert("category".into(), json!({"type": "string", "title": "Category", "enum": pt.categories, "description": "The swimlane: which stage of a task this step belongs to."}));
    let mut order = vec!["category".to_string()];
    if let Some(own) = pt.config_schema["properties"].as_object() {
        for (k, v) in own {
            props.insert(k.clone(), v.clone());
            order.push(k.clone());
        }
    }
    props.insert("budget".into(), json!({"type": "string", "title": "Time budget", "description": "Like 10m or 90s. How long the plugin may take."}));
    props.insert("max_rounds".into(), json!({"type": "integer", "minimum": 1, "title": "Rounds", "description": "How many times this step may loop back to `on_red` before the task fails. Required when it has an on_red edge."}));
    props.insert("uses".into(), json!({"type": "object", "title": "Resources", "additionalProperties": {"type": "integer", "minimum": 1}, "x-choices": def.resources.keys().collect::<Vec<_>>(), "description": "Resources the step holds while it runs, like compile = 1."}));
    order.extend(["budget".into(), "max_rounds".into(), "uses".into()]);
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": if pt.config_schema["title"].is_string() { pt.config_schema["title"].clone() } else { json!(format!("{} step", pt.name)) },
        "type": "object",
        "properties": props,
        "required": pt.config_schema["required"].clone(),
        "x-order": order,
        "x-plugin": pt.plugin,
    })
}

/// The values a form shows for a step: only what the text sets (a step that extends
/// another shows its own keys; the rest is inherited).
pub fn step_values(text: &str, step: &str) -> Value {
    let Ok(d) = text.parse::<DocumentMut>() else {
        return json!({});
    };
    let Some(t) = d
        .get("step")
        .and_then(|s| s.get(step))
        .and_then(|s| s.as_table_like())
    else {
        return json!({});
    };
    let mut m = serde_json::Map::new();
    for (k, v) in t.iter() {
        if matches!(k, "type" | "next" | "on_red" | "extends") {
            continue;
        }
        let j = if let Some(s) = v.as_str() {
            json!(s)
        } else if let Some(i) = v.as_integer() {
            json!(i)
        } else if let Some(b) = v.as_bool() {
            json!(b)
        } else if let Some(a) = v.as_array() {
            json!(a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>())
        } else if let Some(o) = v.as_table_like() {
            json!(o
                .iter()
                .filter_map(|(k, x)| x.as_integer().map(|n| (k.to_string(), n)))
                .collect::<BTreeMap<_, _>>())
        } else {
            continue;
        };
        m.insert(k.to_string(), j);
    }
    Value::Object(m)
}

// ---- the diff and what a change affects ---------------------------------------------------------

/// A unified diff, three lines of context, of two texts.
pub fn unified_diff(name: &str, old: &str, new: &str) -> String {
    if old == new {
        return String::new();
    }
    let (a, b): (Vec<&str>, Vec<&str>) = (old.lines().collect(), new.lines().collect());
    // Longest common subsequence by dynamic programming (the files are small).
    let (n, m) = (a.len(), b.len());
    let mut l = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            l[i][j] = if a[i] == b[j] {
                l[i + 1][j + 1] + 1
            } else {
                l[i + 1][j].max(l[i][j + 1])
            };
        }
    }
    let mut ops: Vec<(char, &str)> = vec![];
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if a[i] == b[j] {
            ops.push((' ', a[i]));
            i += 1;
            j += 1;
        } else if l[i + 1][j] >= l[i][j + 1] {
            ops.push(('-', a[i]));
            i += 1;
        } else {
            ops.push(('+', b[j]));
            j += 1;
        }
    }
    ops.extend(a[i..].iter().map(|s| ('-', *s)));
    ops.extend(b[j..].iter().map(|s| ('+', *s)));
    let keep: Vec<bool> = (0..ops.len())
        .map(|k| (k.saturating_sub(3)..(k + 4).min(ops.len())).any(|x| ops[x].0 != ' '))
        .collect();
    let mut out = format!("--- a/{name}\n+++ b/{name}\n");
    let mut k = 0;
    while k < ops.len() {
        if !keep[k] {
            k += 1;
            continue;
        }
        out.push_str("@@\n");
        while k < ops.len() && keep[k] {
            out.push(ops[k].0);
            out.push_str(ops[k].1);
            out.push('\n');
            k += 1;
        }
    }
    out
}

/// What changed between two definitions, in words a person can read before applying.
pub fn changes(old: &Definition, new: &Definition) -> Vec<String> {
    let mut out = vec![];
    let names = |d: &Definition| {
        d.steps
            .iter()
            .map(|s| s.name.clone())
            .collect::<BTreeSet<_>>()
    };
    let (o, n) = (names(old), names(new));
    for s in n.difference(&o) {
        out.push(format!("adds step `{s}`"));
    }
    for s in o.difference(&n) {
        out.push(format!(
            "removes step `{s}`: tasks now in it finish on the old definition"
        ));
    }
    for s in new.steps.iter().filter(|s| o.contains(&s.name)) {
        let before = old.step(&s.name).expect("present");
        if before != s {
            let mut what = vec![];
            if before.next != s.next {
                what.push("its onward steps");
            }
            if before.on_red != s.on_red {
                what.push("its red edge");
            }
            if before.max_rounds != s.max_rounds {
                what.push("its round limit");
            }
            if before.suites != s.suites || before.command != s.command {
                what.push("what it runs");
            }
            if before.agent != s.agent
                || before.model != s.model
                || before.budget != s.budget
                || before.tools != s.tools
            {
                what.push("its agent settings");
            }
            if what.is_empty() {
                what.push("its settings");
            }
            out.push(format!("changes `{}`: {}", s.name, what.join(", ")));
        }
    }
    if old.start != new.start {
        out.push(format!(
            "tasks now start at `{}` instead of `{}`",
            new.start, old.start
        ));
    }
    let suites = |d: &Definition| {
        d.testing
            .suites
            .iter()
            .map(|s| (s.name.clone(), (s.level.clone(), s.command.clone())))
            .collect::<BTreeMap<_, _>>()
    };
    let (os, ns) = (suites(old), suites(new));
    for k in ns.keys().filter(|k| !os.contains_key(*k)) {
        out.push(format!("adds suite `{k}`"));
    }
    for k in os.keys().filter(|k| !ns.contains_key(*k)) {
        out.push(format!("removes suite `{k}`"));
    }
    for (k, v) in &ns {
        if os.get(k).is_some_and(|x| x != v) {
            out.push(format!("changes suite `{k}`"));
        }
    }
    let (go, gn) = (grid_of(old), grid_of(new));
    for row in &gn.rows {
        if let Some(prev) = go.rows.iter().find(|r| r.suite == row.suite) {
            for (i, c) in row.cells.iter().enumerate() {
                if prev.cells[i] != *c {
                    out.push(format!(
                        "{} `{}` {} ({})",
                        if *c { "runs" } else { "stops running" },
                        row.suite,
                        gn.columns[i].replace('_', " "),
                        row.level
                    ));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "\
# The pipeline for web-api. Keep the comments.
schema = 1
start = \"develop\"   # no exploring here

# --- steps ---
[step.develop]
type = \"agent\"
category = \"developing\"
next = \"test\"   # then test

[step.test]
type = \"test\"
category = \"testing\"
on_red = \"develop\"
max_rounds = 2
next = \"land\"
suites = [\"unit\"]

# landing is last
[step.land]
type = \"landing\"
category = \"landing\"

[suite.unit]
command = \"cargo test\"
parser = \"tap\"
min_tests = 1
level = \"unit\"

[suite.e2e]
command = \"sh e2e.sh\"
parser = \"tap\"
min_tests = 1
level = \"e2e\"

[binding.unit]
when = [\"test_step\"]

[resource.compile]
count = 1
";

    fn files() -> Files {
        Files {
            definition: TEXT.into(),
            ..Default::default()
        }
    }

    fn caps() -> Capabilities {
        Capabilities::now()
    }

    fn ok(op: Op, f: &Files) -> Files {
        apply(&op, f, &caps()).unwrap_or_else(|e| panic!("{op:?} refused: {e:?}"))
    }

    fn kept_comments(t: &str) {
        for c in [
            "# The pipeline for web-api. Keep the comments.",
            "# no exploring here",
            "# --- steps ---",
            "# then test",
            "# landing is last",
        ] {
            assert!(t.contains(c), "lost {c:?}:\n{t}");
        }
    }

    #[test]
    fn inserting_a_step_splices_the_graph_and_keeps_comments() {
        let f = ok(
            Op::InsertStep {
                after: "develop".into(),
                name: "lint".into(),
                kind: "command".into(),
                category: "developing".into(),
                extends: None,
                fields: vec![("command".into(), json!("cargo fmt --check"))],
            },
            &files(),
        );
        kept_comments(&f.definition);
        let d = crate::parse(&f.definition).unwrap();
        assert_eq!(d.step("develop").unwrap().next, ["lint"]);
        assert_eq!(
            d.step("lint").unwrap().next,
            ["test"],
            "it took over what develop led to"
        );
        assert!(validate(&d, &caps()).is_empty());
        // The old lines are all still there: the diff is additions and one changed edge.
        let diff = unified_diff(".mason/mason.toml", TEXT, &f.definition);
        assert!(
            diff.contains("+[step.lint]") && diff.contains("+next = \"lint\"   # then test"),
            "{diff}"
        );
    }

    #[test]
    fn a_step_can_be_created_with_extends() {
        let f = ok(
            Op::InsertStep {
                after: "test".into(),
                name: "test-slow".into(),
                kind: "test".into(),
                category: "testing".into(),
                extends: Some("test".into()),
                fields: vec![],
            },
            &files(),
        );
        assert!(f.definition.contains("extends = \"test\""));
        let d = crate::parse(&f.definition).unwrap();
        assert_eq!(
            d.step("test-slow").unwrap().on_red.as_deref(),
            Some("develop"),
            "inherited"
        );
        assert_eq!(d.step("test").unwrap().next, ["test-slow"]);
    }

    #[test]
    fn removing_a_step_reconnects_what_led_to_it() {
        let f = ok(
            Op::InsertStep {
                after: "develop".into(),
                name: "lint".into(),
                kind: "command".into(),
                category: "developing".into(),
                extends: None,
                fields: vec![("command".into(), json!("cargo fmt --check"))],
            },
            &files(),
        );
        let f = ok(
            Op::RemoveStep {
                name: "lint".into(),
            },
            &f,
        );
        kept_comments(&f.definition);
        let d = crate::parse(&f.definition).unwrap();
        assert_eq!(d.step("develop").unwrap().next, ["test"]);
        assert!(d.step("lint").is_none());
        // Removing the red target takes the red edge with it, and that is refused: the loop would have no way back.
        let e = apply(
            &Op::RemoveStep {
                name: "develop".into(),
            },
            &files(),
            &caps(),
        )
        .unwrap_err();
        assert!(!e.is_empty(), "{e:?}");
    }

    #[test]
    fn connecting_and_disconnecting_edges() {
        let f = ok(
            Op::SetEdge {
                from: "test".into(),
                edge: Edge::OnRed,
                to: Some("develop".into()),
            },
            &files(),
        );
        assert_eq!(f, files(), "already so: no change");
        // Pointing an edge at nothing is refused.
        let e = apply(
            &Op::SetEdge {
                from: "develop".into(),
                edge: Edge::Next,
                to: Some("nowhere".into()),
            },
            &files(),
            &caps(),
        )
        .unwrap_err();
        assert!(
            e.iter()
                .any(|p| p.message.contains("`nowhere` is not a step")),
            "{e:?}"
        );
        // Cutting the only way to land is refused: not every path reaches a terminal.
        let e = apply(
            &Op::SetEdge {
                from: "test".into(),
                edge: Edge::Next,
                to: None,
            },
            &files(),
            &caps(),
        )
        .unwrap_err();
        assert!(!e.is_empty());
    }

    #[test]
    fn editing_fields_keeps_comments_and_a_bad_value_is_refused() {
        let f = ok(
            Op::SetField {
                step: "test".into(),
                key: "max_rounds".into(),
                value: json!(5),
            },
            &files(),
        );
        kept_comments(&f.definition);
        let changed: Vec<_> = TEXT
            .lines()
            .zip(f.definition.lines())
            .filter(|(a, b)| a != b)
            .collect();
        assert_eq!(
            changed,
            vec![("max_rounds = 2", "max_rounds = 5")],
            "a one-line diff"
        );
        // Zero rounds, an unknown key, an unknown suite: refused with the reason.
        let e = apply(
            &Op::SetField {
                step: "test".into(),
                key: "max_rounds".into(),
                value: json!(0),
            },
            &files(),
            &caps(),
        )
        .unwrap_err();
        assert!(e.iter().any(|p| p.path == "step.test.max_rounds"), "{e:?}");
        let e = apply(
            &Op::SetField {
                step: "test".into(),
                key: "nxt".into(),
                value: json!("x"),
            },
            &files(),
            &caps(),
        )
        .unwrap_err();
        assert!(e.iter().any(|p| p.message.contains("unknown key")), "{e:?}");
        let e = apply(
            &Op::SetField {
                step: "test".into(),
                key: "suites".into(),
                value: json!(["ghost"]),
            },
            &files(),
            &caps(),
        )
        .unwrap_err();
        assert!(!e.is_empty());
        // Edges are not form fields.
        assert!(apply(
            &Op::SetField {
                step: "test".into(),
                key: "next".into(),
                value: json!("land")
            },
            &files(),
            &caps()
        )
        .is_err());
        // Removing the bound of a loop is refused.
        let e = apply(
            &Op::UnsetField {
                step: "test".into(),
                key: "max_rounds".into(),
            },
            &files(),
            &caps(),
        )
        .unwrap_err();
        assert!(e.iter().any(|p| p.path == "step.test.max_rounds"), "{e:?}");
    }

    #[test]
    fn renaming_updates_every_reference_and_keeps_the_place() {
        let f = ok(
            Op::RenameStep {
                from: "develop".into(),
                to: "build".into(),
            },
            &files(),
        );
        kept_comments(&f.definition);
        let d = crate::parse(&f.definition).unwrap();
        assert_eq!(d.start, "build");
        assert_eq!(d.step("test").unwrap().on_red.as_deref(), Some("build"));
        assert!(
            f.definition.find("[step.build]").unwrap() < f.definition.find("[step.test]").unwrap(),
            "it kept its place:\n{}",
            f.definition
        );
        assert!(
            apply(
                &Op::RenameStep {
                    from: "build".into(),
                    to: "test".into()
                },
                &f,
                &caps()
            )
            .is_err(),
            "a name in use"
        );
        assert!(apply(
            &Op::RenameStep {
                from: "build".into(),
                to: "Bad Name".into()
            },
            &f,
            &caps()
        )
        .is_err());
    }

    #[test]
    fn the_test_grid_edits_bindings_and_suite_levels() {
        let g = grid_of(&crate::parse(TEXT).unwrap());
        assert_eq!(
            g.columns,
            [
                "development",
                "test_step",
                "batch",
                "after_landing",
                "schedule"
            ]
        );
        let unit = g.rows.iter().find(|r| r.suite == "unit").unwrap();
        assert_eq!(unit.cells, [false, true, false, false, false]);
        let e2e = g.rows.iter().find(|r| r.suite == "e2e").unwrap();
        assert_eq!(e2e.cells, [false, false, false, false, false]);
        let f = ok(
            Op::SetBinding {
                level: "e2e".into(),
                when: "batch".into(),
                on: true,
            },
            &files(),
        );
        assert!(f.definition.contains("[binding.e2e]"), "{}", f.definition);
        let g = grid_of(&crate::parse(&f.definition).unwrap());
        assert_eq!(
            g.rows.iter().find(|r| r.suite == "e2e").unwrap().cells,
            [false, false, true, false, false]
        );
        kept_comments(&f.definition);
        let f = ok(
            Op::SetBinding {
                level: "unit".into(),
                when: "development".into(),
                on: true,
            },
            &f,
        );
        let f = ok(
            Op::SetBinding {
                level: "unit".into(),
                when: "test_step".into(),
                on: false,
            },
            &f,
        );
        let u = grid_of(&crate::parse(&f.definition).unwrap())
            .rows
            .into_iter()
            .find(|r| r.suite == "unit")
            .unwrap();
        assert_eq!(u.cells, [true, false, false, false, false]);
        // Unbinding the last place drops the binding table.
        let f = ok(
            Op::SetBinding {
                level: "e2e".into(),
                when: "batch".into(),
                on: false,
            },
            &f,
        );
        assert!(!f.definition.contains("[binding.e2e]"));
        assert!(apply(
            &Op::SetBinding {
                level: "unit".into(),
                when: "tuesday".into(),
                on: true
            },
            &f,
            &caps()
        )
        .is_err());
        let f = ok(
            Op::SetSuiteLevel {
                suite: "e2e".into(),
                level: "device".into(),
            },
            &f,
        );
        assert!(f.definition.contains("level = \"device\""));
        assert!(apply(
            &Op::SetSuiteLevel {
                suite: "ghost".into(),
                level: "x".into()
            },
            &f,
            &caps()
        )
        .is_err());
    }

    #[test]
    fn moving_a_node_writes_the_sidecar_and_not_the_pipeline() {
        let f = ok(
            Op::Move {
                step: "develop".into(),
                x: 300,
                y: 200,
            },
            &files(),
        );
        assert_eq!(f.definition, TEXT, "the pipeline is untouched");
        let p = picture(&f, &caps());
        let n = p.nodes.iter().find(|n| n.name == "develop").unwrap();
        assert_eq!((n.x, n.y, n.placed), (300, 200, true));
        assert!(!p.nodes.iter().find(|n| n.name == "test").unwrap().placed);
        // A renamed step keeps its place; a removed one loses it.
        let f2 = ok(
            Op::RenameStep {
                from: "develop".into(),
                to: "build".into(),
            },
            &f,
        );
        assert!(
            picture(&f2, &caps())
                .nodes
                .iter()
                .find(|n| n.name == "build")
                .unwrap()
                .placed
        );
        let f3 = ok(
            Op::InsertStep {
                after: "develop".into(),
                name: "extra".into(),
                kind: "approval".into(),
                category: "developing".into(),
                extends: None,
                fields: vec![],
            },
            &f,
        );
        let f3 = ok(
            Op::Move {
                step: "extra".into(),
                x: 1,
                y: 2,
            },
            &f3,
        );
        let f3 = ok(
            Op::RemoveStep {
                name: "extra".into(),
            },
            &f3,
        );
        assert!(!f3.layout.contains("extra"));
    }

    #[test]
    fn the_picture_has_lanes_edges_and_problems_on_the_node() {
        let p = picture(&files(), &caps());
        assert_eq!(
            p.lanes.iter().map(|l| l.0.as_str()).collect::<Vec<_>>(),
            ["exploring", "developing", "testing", "review", "landing"]
        );
        let by = |n: &str| p.nodes.iter().find(|x| x.name == n).unwrap().clone();
        assert_eq!(
            (by("develop").lane, by("test").lane, by("land").lane),
            (1, 2, 4)
        );
        assert!(by("develop").is_start);
        assert_eq!(by("test").summary, "unit");
        assert!(p.edges.contains(&EdgeView {
            from: "test".into(),
            to: "develop".into(),
            kind: "on_red",
            bound: Some(2)
        }));
        assert!(p.edges.contains(&EdgeView {
            from: "develop".into(),
            to: "test".into(),
            kind: "next",
            bound: None
        }));
        // Two steps in one lane sit side by side.
        let f = ok(
            Op::InsertStep {
                after: "test".into(),
                name: "test-b".into(),
                kind: "test".into(),
                category: "testing".into(),
                extends: Some("test".into()),
                fields: vec![],
            },
            &files(),
        );
        let p = picture(&f, &caps());
        let (a, b) = (
            p.nodes.iter().find(|n| n.name == "test").unwrap(),
            p.nodes.iter().find(|n| n.name == "test-b").unwrap(),
        );
        assert_eq!(a.y, b.y);
        assert_ne!(a.x, b.x);
        // A problem shows on its node.
        let bad = Files {
            definition: TEXT.replace("max_rounds = 2\n", ""),
            ..Default::default()
        };
        let p = picture(&bad, &caps());
        let t = p.nodes.iter().find(|n| n.name == "test").unwrap();
        assert!(
            t.problems
                .iter()
                .any(|m| m.contains("max_rounds") || m.contains("bound")),
            "{:?}",
            t.problems
        );
    }

    #[test]
    fn an_edit_may_leave_old_problems_but_not_add_new_ones() {
        let bad = Files {
            definition: TEXT.replace("max_rounds = 2\n", ""),
            ..Default::default()
        };
        // A different, harmless edit to a definition that already has a problem is allowed.
        let f = apply(
            &Op::SetField {
                step: "develop".into(),
                key: "budget".into(),
                value: json!("2h"),
            },
            &bad,
            &caps(),
        )
        .unwrap();
        assert!(f.definition.contains("budget = \"2h\""));
        // The fix is allowed too.
        let fixed = apply(
            &Op::SetField {
                step: "test".into(),
                key: "max_rounds".into(),
                value: json!(2),
            },
            &bad,
            &caps(),
        )
        .unwrap();
        assert!(problems_of(&fixed.definition, &caps()).is_empty());
        // A new problem is refused.
        assert!(apply(
            &Op::SetField {
                step: "develop".into(),
                key: "budget".into(),
                value: json!("a while")
            },
            &bad,
            &caps()
        )
        .is_err());
    }

    #[test]
    fn schemas_describe_each_kind_and_values_read_back() {
        let d = crate::parse(TEXT).unwrap();
        let s = step_schema(StepType::Test, &d);
        assert_eq!(
            s["properties"]["suites"]["items"]["enum"],
            json!(["unit", "e2e"])
        );
        assert_eq!(s["properties"]["category"]["enum"], json!(["testing"]));
        let a = step_schema(StepType::Agent, &d);
        assert!(a["properties"]["agent"]["enum"]
            .as_array()
            .unwrap()
            .contains(&json!("deep")));
        assert!(a["properties"]["budget"].is_object());
        assert!(step_schema(StepType::Landing, &d)["properties"]["suites"].is_null());
        let v = step_values(TEXT, "test");
        assert_eq!(
            v,
            json!({"category": "testing", "max_rounds": 2, "suites": ["unit"]})
        );
        assert_eq!(step_values(TEXT, "ghost"), json!({}));
        let f = ok(
            Op::SetField {
                step: "develop".into(),
                key: "uses".into(),
                value: json!({"compile": 1}),
            },
            &files(),
        );
        assert!(
            f.definition.contains("uses = { compile = 1 }"),
            "{}",
            f.definition
        );
    }

    #[test]
    fn the_diff_is_small_and_changes_read_in_words() {
        let f = ok(
            Op::SetField {
                step: "test".into(),
                key: "max_rounds".into(),
                value: json!(5),
            },
            &files(),
        );
        let diff = unified_diff(".mason/mason.toml", TEXT, &f.definition);
        assert!(
            diff.starts_with("--- a/.mason/mason.toml\n+++ b/.mason/mason.toml\n@@\n"),
            "{diff}"
        );
        assert!(diff.contains("-max_rounds = 2\n+max_rounds = 5\n"));
        assert_eq!(
            diff.lines()
                .filter(|l| l.starts_with('-') || l.starts_with('+'))
                .count(),
            4,
            "two headers and one change each way:\n{diff}"
        );
        assert_eq!(unified_diff("x", TEXT, TEXT), "");
        let (o, n) = (
            crate::parse(TEXT).unwrap(),
            crate::parse(&f.definition).unwrap(),
        );
        assert_eq!(changes(&o, &n), ["changes `test`: its round limit"]);
        let g = ok(
            Op::SetBinding {
                level: "e2e".into(),
                when: "batch".into(),
                on: true,
            },
            &files(),
        );
        let c = changes(&o, &crate::parse(&g.definition).unwrap());
        assert_eq!(c, ["runs `e2e` batch (e2e)"]);
    }
}
