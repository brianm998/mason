// SPDX-License-Identifier: GPL-3.0-or-later

//! The rules a definition must satisfy before it can be applied (Part 5):
//!
//! * every path reaches a terminal state (a landing step), and every loop has a bound;
//! * fan-out and joins behave;
//! * every referenced step, agent profile, resource and tool exists;
//! * nothing is used that this version cannot run.
//!
//! The rules are written so that a valid definition can never run forever or
//! strand a task: whatever the steps report, a walk through it ends in
//! `landed` or `failed` (the property tests check exactly that, through the
//! real reducer).

use crate::builtin::builtin_profile_names;
use crate::model::*;
use mason_adapter_api::contract::StepKind;
use mason_core::State;
use mason_plugins::PluginType;
use std::collections::{BTreeMap, BTreeSet};

/// What this version of MASON can actually run.
#[derive(Clone, Debug)]
pub struct Capabilities {
    pub step_types: Vec<StepType>,
    pub fan_out: bool,
    pub conditions: bool,
    /// The step types plugins provide (Stage 19).
    pub plugins: Vec<PluginType>,
}

impl Capabilities {
    /// What the daemon can run now: agent, command, test, approval and landing
    /// steps in a straight line with loops back. (`gate` and the suites behind
    /// `test` arrive in Stage 9, `ci` and `pr` in Stage 15, fan-out and
    /// conditions later still.)
    pub fn now() -> Capabilities {
        use StepType::*;
        Capabilities {
            step_types: vec![Agent, Command, Test, Approval, Landing],
            fan_out: false,
            conditions: false,
            plugins: vec![],
        }
    }

    /// The same, plus the step types these plugins provide.
    pub fn with_plugins(mut self, plugins: Vec<PluginType>) -> Capabilities {
        self.plugins = plugins;
        self
    }

    /// The names of the plugin step types, for [`crate::parse_with`].
    pub fn plugin_names(&self) -> Vec<String> {
        self.plugins.iter().map(|p| p.name.clone()).collect()
    }

    pub fn plugin(&self, name: &str) -> Option<&PluginType> {
        self.plugins.iter().find(|p| p.name == name)
    }

    /// Everything the language can express, for checking a definition's shape
    /// independently of what can run yet.
    pub fn everything() -> Capabilities {
        Capabilities {
            step_types: StepType::ALL.to_vec(),
            fan_out: true,
            conditions: true,
            plugins: vec![],
        }
    }
}

/// Gates a test step may name.
pub const GATES: &[&str] = &["negative_control", "assertion_scan", "acceptance"];

/// Suites, environments, bindings, the impact map and signatures all refer to
/// each other and to the definition's resources; every reference must exist.
fn validate_testing(def: &Definition, p: &mut Vec<Problem>) {
    let t = &def.testing;
    let suite_names: BTreeSet<&str> = t.suites.iter().map(|s| s.name.as_str()).collect();
    let levels: BTreeSet<&str> = t.suites.iter().map(|s| s.level.as_str()).collect();
    for s in &t.suites {
        let base = format!("suite.{}", s.name);
        if !s.env.is_empty() && t.environment(&s.env).is_none() {
            p.push(Problem::new(
                format!("{base}.env"),
                format!(
                    "there is no environment `{}`; define [environment.{}]",
                    s.env, s.env
                ),
            ));
        }
        for r in s.uses.keys() {
            if !def.resources.contains_key(r) {
                p.push(Problem::new(
                    format!("{base}.uses.{r}"),
                    format!("there is no resource `{r}`; declare it as [resource.{r}]"),
                ));
            }
        }
        if let Some(r) = &s.rerun {
            if !r.contains("{tests}") {
                p.push(Problem::new(
                    format!("{base}.rerun"),
                    "must contain `{tests}`, where the failing test names go",
                ));
            }
        }
        if s.parser == mason_testing::ParserKind::Regex
            && s.pass_regex.is_none()
            && s.fail_regex.is_none()
        {
            p.push(Problem::new(
                format!("{base}.parser"),
                "the regex parser needs pass_regex or fail_regex",
            ));
        }
        for (k, v) in [("pass_regex", &s.pass_regex), ("fail_regex", &s.fail_regex)] {
            if let Some(Err(e)) = v.as_deref().map(mason_testing::parse::check_regex) {
                p.push(Problem::new(format!("{base}.{k}"), e));
            }
        }
    }
    for e in &t.environments {
        for r in e.uses.keys() {
            if !def.resources.contains_key(r) {
                p.push(Problem::new(
                    format!("environment.{}.uses.{r}", e.name),
                    format!("there is no resource `{r}`; declare it as [resource.{r}]"),
                ));
            }
        }
        if e.kind == mason_testing::EnvKind::Compose && e.file.is_none() {
            p.push(Problem::new(
                format!("environment.{}.file", e.name),
                "a compose environment needs its compose file",
            ));
        }
    }
    for b in &t.bindings {
        if !levels.contains(b.level.as_str()) {
            p.push(Problem::new(
                format!("binding.{}", b.level),
                format!("no suite has level `{}`", b.level),
            ));
        }
    }
    for (glob, suites) in &t.impact.entries {
        for s in suites {
            if !suite_names.contains(s.as_str()) {
                p.push(Problem::new(
                    format!("impact.{glob}"),
                    format!("there is no suite `{s}`"),
                ));
            }
        }
    }
    for g in &t.signatures {
        if let Err(e) = mason_testing::parse::check_regex(&g.pattern) {
            p.push(Problem::new(format!("signature.{}.pattern", g.name), e));
        }
    }
    for s in &def.steps {
        let base = format!("step.{}", s.name);
        for (i, name) in s.suites.iter().enumerate() {
            if !suite_names.contains(name.as_str()) {
                p.push(Problem::new(
                    format!("{base}.suites[{i}]"),
                    format!("there is no suite `{name}`; define [suite.{name}]"),
                ));
            }
        }
        for (i, g) in s.gates.iter().enumerate() {
            if !GATES.contains(&g.as_str()) {
                p.push(Problem::new(
                    format!("{base}.gates[{i}]"),
                    format!("unknown gate `{g}`; the gates are {}", GATES.join(", ")),
                ));
            }
        }
        if !s.suites.is_empty() && s.command.is_some() {
            p.push(Problem::new(
                format!("{base}.command"),
                "a step runs either its suites or one command, not both",
            ));
        }
        if !s.gates.is_empty() && s.suites.is_empty() {
            p.push(Problem::new(
                format!("{base}.gates"),
                "gates judge suites; this step names none",
            ));
        }
    }
}

/// The contract tools for a category's agent steps.
pub fn contract_kind(c: State) -> Option<StepKind> {
    match c {
        State::Exploring => Some(StepKind::Explore),
        State::Developing => Some(StepKind::Develop),
        State::Review => Some(StepKind::Review),
        _ => None,
    }
}

/// Which optional keys each step type accepts, beyond the ones every step has
/// (`type`, `category`, `next`, `on_red`, `max_rounds`, `uses`, `when`, `join`).
fn type_keys(k: StepType) -> &'static [&'static str] {
    match k {
        StepType::Agent => &["agent", "model", "effort", "budget", "tools", "read_only"],
        StepType::Command => &["command", "budget"],
        StepType::Test => &["command", "suites", "budget", "gates"],
        StepType::Gate => &["command", "suites", "budget"],
        StepType::Approval => &[],
        StepType::Ci => &["suites", "budget", "policy"],
        StepType::Pr => &["budget", "policy"],
        StepType::Landing => &["policy", "budget"],
        // Settings are checked against the plugin's schema instead.
        StepType::Plugin => &["budget"],
    }
}

fn set_keys(s: &Step) -> Vec<&'static str> {
    let mut v = vec![];
    if s.agent.is_some() {
        v.push("agent");
    }
    if s.model.is_some() {
        v.push("model");
    }
    if s.effort.is_some() {
        v.push("effort");
    }
    if s.budget.is_some() {
        v.push("budget");
    }
    if s.tools.is_some() {
        v.push("tools");
    }
    if s.read_only.is_some() {
        v.push("read_only");
    }
    if s.command.is_some() {
        v.push("command");
    }
    if !s.suites.is_empty() {
        v.push("suites");
    }
    if s.policy.is_some() {
        v.push("policy");
    }
    if !s.gates.is_empty() {
        v.push("gates");
    }
    v
}

fn stage_for(k: StepType) -> &'static str {
    match k {
        StepType::Gate => "Stage 9",
        StepType::Ci | StepType::Pr => "Stage 15",
        _ => "a later stage",
    }
}

pub fn validate(def: &Definition, caps: &Capabilities) -> Vec<Problem> {
    let mut p: Vec<Problem> = vec![];
    if def.schema != 1 {
        p.push(Problem::new(
            "schema",
            format!(
                "schema {} is not supported; this version reads schema 1",
                def.schema
            ),
        ));
    }

    // Entry points.
    if def.step(&def.start).is_none() {
        p.push(Problem::new(
            "start",
            format!("`{}` is not a step", def.start),
        ));
    }
    if let Some(e) = &def.explored_entry {
        match def.step(e) {
            None => p.push(Problem::new("explored_entry", format!("`{e}` is not a step"))),
            Some(s) if s.category == State::Exploring => p.push(Problem::new(
                "explored_entry",
                format!("`{e}` is an exploring step; this is where tasks that skip exploring start, so it must come after exploring"),
            )),
            Some(_) => {}
        }
    }
    if def.steps.is_empty() {
        p.push(Problem::new("step", "a definition needs at least one step"));
    }

    validate_testing(def, &mut p);

    let names: BTreeSet<&str> = def.steps.iter().map(|s| s.name.as_str()).collect();
    let max_rounds_ok = |s: &Step| s.max_rounds.is_some();

    for s in &def.steps {
        let base = format!("step.{}", s.name);

        let plugin = match s.kind {
            StepType::Plugin => {
                let name = s.plugin.as_deref().unwrap_or_default();
                match caps.plugin(name) {
                    Some(pt) => Some(pt),
                    None => {
                        p.push(Problem::new(
                            format!("{base}.type"),
                            format!(
                                "no extension provides the step type `{name}`; it must be in .mason/extensions/ on the base branch, and valid"
                            ),
                        ));
                        continue;
                    }
                }
            }
            _ => None,
        };
        if let Some(pt) = plugin {
            if !pt
                .categories
                .iter()
                .any(|c| *c == category_name(s.category))
            {
                p.push(Problem::new(
                    format!("{base}.category"),
                    format!(
                        "a {} step cannot be in `{}`; use {}",
                        pt.name,
                        category_name(s.category),
                        pt.categories.join(" or ")
                    ),
                ));
            }
            let settings = serde_json::Value::Object(s.config.clone().into_iter().collect());
            for m in mason_plugins::schema::validate(&pt.config_schema, &settings) {
                p.push(Problem::new(
                    format!("{base}.settings"),
                    format!("{m} (the {} plugin's settings)", pt.name),
                ));
            }
        } else if !s.kind.categories().contains(&s.category) {
            let allowed: Vec<&str> = s
                .kind
                .categories()
                .iter()
                .map(|c| category_name(*c))
                .collect();
            p.push(Problem::new(
                format!("{base}.category"),
                format!(
                    "a {} step cannot be in `{}`; use {}",
                    s.type_name(),
                    category_name(s.category),
                    allowed.join(" or ")
                ),
            ));
        }
        if s.kind != StepType::Plugin && !caps.step_types.contains(&s.kind) {
            p.push(Problem::new(
                format!("{base}.type"),
                format!(
                    "step type `{}` cannot run yet (it arrives in {})",
                    s.type_name(),
                    stage_for(s.kind)
                ),
            ));
        }
        for k in set_keys(s) {
            if !type_keys(s.kind).contains(&k) {
                p.push(Problem::new(
                    format!("{base}.{k}"),
                    format!("`{k}` does not apply to {} steps", s.type_name()),
                ));
            }
        }

        // Agent steps: profile and tools.
        if s.kind == StepType::Agent {
            if let Some(a) = &s.agent {
                if !def.agents.contains_key(a) && !builtin_profile_names().contains(&a.as_str()) {
                    p.push(Problem::new(
                        format!("{base}.agent"),
                        format!(
                            "there is no agent profile `{a}`; define [agent.{a}] or use one of {}",
                            builtin_profile_names().join(", ")
                        ),
                    ));
                }
            }
            if let Some(kind) = contract_kind(s.category) {
                if let Some(tools) = &s.tools {
                    for (i, t) in tools.iter().enumerate() {
                        if !kind.tools().contains(&t.as_str()) {
                            p.push(Problem::new(
                                format!("{base}.tools[{i}]"),
                                format!(
                                    "`{t}` is not available in a {} step; available: {}",
                                    category_name(s.category),
                                    kind.tools().join(", ")
                                ),
                            ));
                        }
                    }
                    if !tools
                        .iter()
                        .any(|t| kind.terminal_tools().contains(&t.as_str()))
                    {
                        p.push(Problem::new(
                            format!("{base}.tools"),
                            format!(
                                "the step could never finish: it needs {}",
                                kind.terminal_tools().join(" or ")
                            ),
                        ));
                    }
                }
                if s.read_only == Some(false) && kind.read_only() {
                    p.push(Problem::new(
                        format!("{base}.read_only"),
                        format!("{} steps are always read-only", category_name(s.category)),
                    ));
                }
            }
        }
        if s.kind == StepType::Command && s.command.as_deref().is_none_or(|c| c.trim().is_empty()) {
            p.push(Problem::new(
                format!("{base}.command"),
                "a command step needs a `command`",
            ));
        }

        // Loops.
        match (&s.on_red, s.max_rounds) {
            (Some(r), mr) => {
                if !matches!(s.category, State::Testing | State::Review | State::Landing) {
                    p.push(Problem::new(format!("{base}.on_red"), format!("only testing, review and landing steps can be sent back; this one is `{}`", category_name(s.category))));
                }
                match def.step(r) {
                    None => p.push(Problem::new(
                        format!("{base}.on_red"),
                        format!("`{r}` is not a step"),
                    )),
                    Some(t) if t.category != State::Developing => p.push(Problem::new(
                        format!("{base}.on_red"),
                        format!(
                            "`{r}` is a {} step; red work goes back to a developing step",
                            category_name(t.category)
                        ),
                    )),
                    Some(_) => {}
                }
                if mr.is_none() {
                    p.push(Problem::new(format!("{base}.max_rounds"), "this step loops back with on_red, so it needs max_rounds to bound the loop"));
                }
            }
            (None, Some(_)) => p.push(Problem::new(
                format!("{base}.max_rounds"),
                "max_rounds is only meaningful with on_red",
            )),
            (None, None) => {}
        }

        // Where it goes next.
        let mut seen = BTreeSet::new();
        for (i, n) in s.next.iter().enumerate() {
            let path = format!(
                "{base}.next{}",
                if s.next.len() > 1 {
                    format!("[{i}]")
                } else {
                    String::new()
                }
            );
            if !seen.insert(n.as_str()) {
                p.push(Problem::new(&path, format!("`{n}` is listed twice")));
            }
            match def.step(n) {
                None => p.push(Problem::new(&path, format!("`{n}` is not a step"))),
                Some(t) => {
                    if t.name == s.name {
                        p.push(Problem::new(
                            &path,
                            "a step cannot be its own next step; loop back with on_red",
                        ));
                    } else if category_order(t.category) < category_order(s.category) {
                        p.push(Problem::new(
                            &path,
                            format!("`{n}` is a {} step, which is before this {} step; going back is what on_red is for", category_name(t.category), category_name(s.category)),
                        ));
                    }
                }
            }
        }
        if s.next.len() > 1 && !caps.fan_out {
            p.push(Problem::new(
                format!("{base}.next"),
                "fan-out cannot run yet: a step can have only one next step",
            ));
        }
        if s.next.is_empty() && s.kind != StepType::Landing {
            p.push(Problem::new(format!("{base}.next"), format!("this {} step ends the path but is not a landing step, so tasks would be stranded here; give it a `next`", s.type_name())));
        }
        if !s.next.is_empty() && s.kind == StepType::Landing {
            p.push(Problem::new(
                format!("{base}.next"),
                "nothing can follow a landing step yet: landing ends the task",
            ));
        }

        for r in s.uses.keys() {
            if !def.resources.contains_key(r) {
                p.push(Problem::new(
                    format!("{base}.uses.{r}"),
                    format!("there is no resource `{r}`; declare it as [resource.{r}]"),
                ));
            }
        }
        if let Some(w) = &s.when {
            if !caps.conditions {
                p.push(Problem::new(
                    format!("{base}.when"),
                    "conditions cannot run yet",
                ));
            } else if !(w.starts_with("plan.") || w.starts_with("task.")) || w.len() < 6 {
                p.push(Problem::new(
                    format!("{base}.when"),
                    format!("`{w}` is not a condition; use plan.<name> or task.<name>"),
                ));
            }
        }
    }

    // Graph-wide rules need every reference to be sound first.
    let refs_sound = !p.iter().any(|x| x.message.contains("is not a step"));
    if refs_sound && !def.steps.is_empty() && def.step(&def.start).is_some() {
        graph_rules(def, caps, &names, &mut p, max_rounds_ok);
    }
    p
}

fn graph_rules(
    def: &Definition,
    _caps: &Capabilities,
    _names: &BTreeSet<&str>,
    p: &mut Vec<Problem>,
    _mr: impl Fn(&Step) -> bool,
) {
    // Reachability from the entry points, along next and on_red.
    let mut reached: BTreeSet<String> = BTreeSet::new();
    let mut stack: Vec<String> = vec![def.start.clone()];
    if let Some(e) = &def.explored_entry {
        if def.step(e).is_some() {
            stack.push(e.clone());
        }
    }
    while let Some(n) = stack.pop() {
        if !reached.insert(n.clone()) {
            continue;
        }
        if let Some(s) = def.step(&n) {
            stack.extend(s.next.iter().cloned());
            stack.extend(s.on_red.iter().cloned());
        }
    }
    for s in &def.steps {
        if !reached.contains(&s.name) {
            p.push(Problem::new(
                format!("step.{}", s.name),
                "not reachable from `start` or `explored_entry`; remove it or connect it",
            ));
        }
    }

    // `next` must be acyclic: every loop goes through on_red, which is bounded.
    let mut state: BTreeMap<&str, u8> = BTreeMap::new(); // 1 visiting, 2 done
    fn dfs<'a>(
        def: &'a Definition,
        n: &'a str,
        state: &mut BTreeMap<&'a str, u8>,
        p: &mut Vec<Problem>,
    ) {
        state.insert(n, 1);
        if let Some(s) = def.step(n) {
            for m in &s.next {
                match state.get(m.as_str()) {
                    Some(1) => p.push(Problem::new(format!("step.{n}.next"), format!("`{n}` leads back to `{m}`, an unbounded loop; loops must use on_red with max_rounds"))),
                    Some(_) => {}
                    None => dfs(def, m, state, p),
                }
            }
        }
        state.insert(n, 2);
    }
    for s in &def.steps {
        if !state.contains_key(s.name.as_str()) {
            dfs(def, &s.name, &mut state, p);
        }
    }

    fan_out_rules(def, p);
}

/// Forward closure from `from` along `next`, not expanding past join steps.
/// Returns (steps inside, join steps reached).
fn closure(def: &Definition, from: &str) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut inside = BTreeSet::new();
    let mut joins = BTreeSet::new();
    let mut stack = vec![from.to_string()];
    while let Some(n) = stack.pop() {
        let Some(s) = def.step(&n) else { continue };
        if s.join.is_some() {
            joins.insert(n);
            continue;
        }
        if inside.insert(n) {
            stack.extend(s.next.iter().cloned());
        }
    }
    (inside, joins)
}

fn fan_out_rules(def: &Definition, p: &mut Vec<Problem>) {
    let mut preds: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for s in &def.steps {
        for n in &s.next {
            preds.entry(n.as_str()).or_default().insert(s.name.as_str());
        }
    }
    let mut joins_used: BTreeSet<String> = BTreeSet::new();
    for f in def.steps.iter().filter(|s| s.next.len() >= 2) {
        let base = format!("step.{}.next", f.name);
        let mut branch_sets = vec![];
        let mut ok = true;
        for b in &f.next {
            let (inside, joins) = closure(def, b);
            if joins.len() != 1 {
                p.push(Problem::new(&base, format!("the branch starting at `{b}` must reach exactly one join step (a step with `join`), but reaches {}", joins.len())));
                ok = false;
            }
            for n in &inside {
                let s = def.step(n).expect("exists");
                if s.next.len() >= 2 {
                    p.push(Problem::new(
                        &base,
                        format!(
                            "`{n}` fans out again inside a branch; nested fan-out is not supported"
                        ),
                    ));
                    ok = false;
                }
                if s.kind == StepType::Landing {
                    p.push(Problem::new(&base, format!("the branch starting at `{b}` reaches the landing step `{n}` before it rejoins")));
                    ok = false;
                }
            }
            branch_sets.push((inside, joins));
        }
        if !ok {
            continue;
        }
        let join_names: BTreeSet<&String> =
            branch_sets.iter().flat_map(|(_, j)| j.iter()).collect();
        if join_names.len() != 1 {
            p.push(Problem::new(
                &base,
                "the branches end at different join steps; they must all meet at one",
            ));
            continue;
        }
        let j = (*join_names.iter().next().expect("one")).clone();
        joins_used.insert(j.clone());
        let mut all: BTreeSet<&str> = BTreeSet::new();
        for (i, (inside, _)) in branch_sets.iter().enumerate() {
            for n in inside {
                if !all.insert(n.as_str()) {
                    p.push(Problem::new(&base, format!("branches overlap at `{n}` (branch {i}); each step may belong to only one branch")));
                }
            }
        }
        // Nothing may enter a branch from outside, and only the branches may enter the join.
        for n in &all {
            for pr in preds.get(n).into_iter().flatten() {
                if *pr != f.name && !all.contains(pr) {
                    p.push(Problem::new(
                        format!("step.{n}"),
                        format!(
                            "`{n}` is inside the fan-out at `{}` but is also reached from `{pr}`",
                            f.name
                        ),
                    ));
                }
            }
        }
        for pr in preds.get(j.as_str()).into_iter().flatten() {
            if !all.contains(pr) {
                p.push(Problem::new(format!("step.{j}"), format!("the join `{j}` is also reached from `{pr}`, which is outside the fan-out at `{}`", f.name)));
            }
        }
    }
    for s in def.steps.iter().filter(|s| s.join.is_some()) {
        if !joins_used.contains(&s.name)
            && !p
                .iter()
                .any(|x| x.message.contains(&format!("`{}`", s.name)))
        {
            p.push(Problem::new(
                format!("step.{}.join", s.name),
                "this step is marked as a join but no fan-out leads to it",
            ));
        }
    }
}
