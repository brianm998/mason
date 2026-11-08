// SPDX-License-Identifier: GPL-3.0-or-later

//! Reading `.mason/mason.toml`. Every mistake is reported, not just the first,
//! and each names the key it is about. Unknown keys are mistakes: a typo must
//! not silently turn a setting off.

use crate::model::*;
use std::collections::BTreeMap;
use toml_edit::{DocumentMut, Item};

const TOP_KEYS: &[&str] = &[
    "schema",
    "start",
    "explored_entry",
    "step",
    "agent",
    "resource",
    "suite",
    "environment",
    "binding",
    "impact",
    "signature",
];
pub const STEP_KEYS: &[&str] = &[
    "type",
    "category",
    "next",
    "on_red",
    "max_rounds",
    "agent",
    "model",
    "effort",
    "budget",
    "tools",
    "read_only",
    "command",
    "suites",
    "when",
    "uses",
    "join",
    "policy",
    "gates",
    "extends",
];

fn distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i];
        for j in 1..=b.len() {
            let sub = prev[j - 1] + usize::from(a[i - 1] != b[j - 1]);
            cur.push(sub.min(prev[j] + 1).min(cur[j - 1] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// "unknown key `nxt`" plus "did you mean `next`?" when one is close.
pub fn unknown_key(path: &str, key: &str, allowed: &[&str]) -> Problem {
    let close = allowed
        .iter()
        .filter(|a| distance(key, a) <= 2)
        .min_by_key(|a| distance(key, a));
    let hint = close.map_or(String::new(), |c| format!("; did you mean `{c}`?"));
    Problem::new(path, format!("unknown key `{key}`{hint}"))
}

fn valid_name(s: &str) -> bool {
    let mut c = s.chars();
    c.next().is_some_and(|f| f.is_ascii_lowercase())
        && c.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

fn string(item: &Item, path: &str, out: &mut Vec<Problem>) -> Option<String> {
    match item.as_str() {
        Some(s) => Some(s.to_string()),
        None => {
            out.push(Problem::new(path, "must be a string"));
            None
        }
    }
}

fn strings(item: &Item, path: &str, out: &mut Vec<Problem>) -> Option<Vec<String>> {
    let Some(a) = item.as_array() else {
        out.push(Problem::new(path, "must be a list of strings"));
        return None;
    };
    let mut v = vec![];
    for (i, x) in a.iter().enumerate() {
        match x.as_str() {
            Some(s) => v.push(s.to_string()),
            None => out.push(Problem::new(format!("{path}[{i}]"), "must be a string")),
        }
    }
    Some(v)
}

/// Which step type names beyond the built-in ones a definition may use.
#[derive(Clone, Copy)]
pub enum Plugins<'a> {
    /// Exactly these (the types the project's approved-or-not extensions provide).
    Named(&'a [String]),
    /// Any name: for a definition already validated and stored (a pinned one).
    Any,
}

impl Plugins<'_> {
    fn allows(&self, name: &str) -> bool {
        match self {
            Plugins::Named(n) => n.iter().any(|x| x == name),
            Plugins::Any => true,
        }
    }
}

/// A TOML value as JSON, for a plugin step's settings.
fn to_json(item: &Item) -> Option<serde_json::Value> {
    use serde_json::Value;
    use toml_edit::Value as T;
    if let Some(t) = item.as_table_like() {
        return Some(Value::Object(
            t.iter()
                .map(|(k, v)| Some((k.to_string(), to_json(v)?)))
                .collect::<Option<_>>()?,
        ));
    }
    Some(match item.as_value()? {
        T::String(s) => Value::String(s.value().clone()),
        T::Integer(i) => Value::from(*i.value()),
        T::Float(f) => serde_json::Number::from_f64(*f.value()).map(Value::Number)?,
        T::Boolean(b) => Value::Bool(*b.value()),
        T::Array(a) => Value::Array(
            a.iter()
                .map(|v| to_json(&Item::Value(v.clone())))
                .collect::<Option<_>>()?,
        ),
        T::InlineTable(t) => Value::Object(
            t.iter()
                .map(|(k, v)| Some((k.to_string(), to_json(&Item::Value(v.clone()))?)))
                .collect::<Option<_>>()?,
        ),
        T::Datetime(d) => Value::String(d.value().to_string()),
    })
}

fn parse_step(
    name: &str,
    item: &Item,
    plugins: Plugins<'_>,
    out: &mut Vec<Problem>,
) -> Option<Step> {
    let base = format!("step.{name}");
    let Some(t) = item.as_table_like() else {
        out.push(Problem::new(&base, "must be a table"));
        return None;
    };
    if !valid_name(name) {
        out.push(Problem::new(
            &base,
            "step names are lowercase letters, digits, - and _, starting with a letter",
        ));
    }
    let mut kind = None;
    let mut category = None;
    let mut step = Step::new(name, StepType::Agent, mson_cat());
    // A type that is not built in but a plugin provides: its other keys are its settings.
    let plugin_type = t
        .get("type")
        .and_then(|v| v.as_str())
        .filter(|s| StepType::from_name(s).is_none() && plugins.allows(s))
        .map(String::from);
    if let Some(pt) = &plugin_type {
        kind = Some(StepType::Plugin);
        step.plugin = Some(pt.clone());
    }
    for (k, v) in t.iter() {
        let path = format!("{base}.{k}");
        match k {
            "type" => {
                if let Some(s) = string(v, &path, out) {
                    match StepType::from_name(&s) {
                        Some(t) => kind = Some(t),
                        None if plugin_type.is_some() => {}
                        None => out.push(Problem::new(
                            &path,
                            format!(
                                "unknown step type `{s}`; the built-in types are {}{}",
                                StepType::ALL.map(StepType::name).join(", "),
                                match plugins {
                                    Plugins::Named(n) if !n.is_empty() => {
                                        format!(" and the plugins' {}", n.join(", "))
                                    }
                                    _ => String::new(),
                                }
                            ),
                        )),
                    }
                }
            }
            "category" => {
                if let Some(s) = string(v, &path, out) {
                    match category_from_name(&s) {
                        Some(c) => category = Some(c),
                        None => out.push(Problem::new(&path, format!("unknown category `{s}`; use exploring, developing, testing, review or landing"))),
                    }
                }
            }
            "next" => {
                if let Some(s) = v.as_str() {
                    step.next = vec![s.to_string()];
                } else if v.as_array().is_some() {
                    step.next = strings(v, &path, out).unwrap_or_default();
                } else {
                    out.push(Problem::new(
                        &path,
                        "must be a step name or a list of step names",
                    ));
                }
            }
            "on_red" => step.on_red = string(v, &path, out),
            "max_rounds" => match v.as_integer() {
                Some(n) if n >= 1 && n <= i64::from(u32::MAX) => step.max_rounds = Some(n as u32),
                _ => out.push(Problem::new(&path, "must be a whole number, 1 or more")),
            },
            "agent" => step.agent = string(v, &path, out),
            "model" => step.model = string(v, &path, out),
            "effort" => step.effort = string(v, &path, out),
            "budget" => {
                if let Some(s) = string(v, &path, out) {
                    match parse_duration(&s) {
                        Some(d) => step.budget = Some(d),
                        None => out.push(Problem::new(
                            &path,
                            format!(
                                "`{s}` is not a duration; use forms like \"4h\", \"30m\", \"90s\""
                            ),
                        )),
                    }
                }
            }
            "tools" => step.tools = strings(v, &path, out),
            "read_only" => match v.as_bool() {
                Some(b) => step.read_only = Some(b),
                None => out.push(Problem::new(&path, "must be true or false")),
            },
            "command" => step.command = string(v, &path, out),
            "suites" => step.suites = strings(v, &path, out).unwrap_or_default(),
            "when" => step.when = string(v, &path, out),
            "policy" => step.policy = string(v, &path, out),
            "gates" => step.gates = strings(v, &path, out).unwrap_or_default(),
            "join" => {
                if let Some(s) = string(v, &path, out) {
                    match s.as_str() {
                        "all-green" => step.join = Some(Join::AllGreen),
                        "first-red" => step.join = Some(Join::FirstRed),
                        _ => out.push(Problem::new(
                            &path,
                            "must be \"all-green\" or \"first-red\"",
                        )),
                    }
                }
            }
            // Resolved before the step is read (see `resolve_extends`): by now the step
            // already holds what it took from the one it extends.
            "extends" => {
                string(v, &path, out);
            }
            "uses" => match v.as_table_like() {
                Some(u) => {
                    for (r, n) in u.iter() {
                        match n.as_integer() {
                            Some(n) if n >= 1 => {
                                step.uses.insert(r.to_string(), n as u32);
                            }
                            _ => out.push(Problem::new(
                                format!("{path}.{r}"),
                                "must be a whole number, 1 or more",
                            )),
                        }
                    }
                }
                None => out.push(Problem::new(&path, "must be a table like { phone = 1 }")),
            },
            other if plugin_type.is_some() => match to_json(v) {
                Some(j) => {
                    step.config.insert(other.to_string(), j);
                }
                None => out.push(Problem::new(&path, "this value cannot be a plugin setting")),
            },
            other => out.push(unknown_key(&base, other, STEP_KEYS)),
        }
    }
    if kind.is_none() && t.get("type").is_none() {
        out.push(Problem::new(format!("{base}.type"), "required"));
    }
    if category.is_none() && t.get("category").is_none() {
        out.push(Problem::new(format!("{base}.category"), "required"));
    }
    match (kind, category) {
        (Some(k), Some(c)) => {
            step.kind = k;
            step.category = c;
            Some(step)
        }
        _ => None,
    }
}

fn mson_cat() -> mason_core::State {
    mason_core::State::Exploring
}

/// A step that `extends` another takes every key the other has (and what that one
/// extends) that it does not set itself. One level of inheritance at a time, followed
/// to the end; a missing step or a loop is a problem on the `extends` key.
fn resolve_extends(
    steps: &dyn toml_edit::TableLike,
    name: &str,
    chain: &mut Vec<String>,
) -> Result<toml_edit::Table, Problem> {
    let path = format!("step.{name}.extends");
    let Some(item) = steps.get(name) else {
        return Err(Problem::new(&path, format!("`{name}` is not a step")));
    };
    let Some(own) = item.as_table_like() else {
        return Err(Problem::new(format!("step.{name}"), "must be a table"));
    };
    let mut merged = toml_edit::Table::new();
    if let Some(parent) = own.get("extends").and_then(|v| v.as_str()) {
        if chain.iter().any(|c| c == parent) || parent == name {
            return Err(Problem::new(
                &path,
                format!("`{name}` extends itself through `{parent}`"),
            ));
        }
        chain.push(name.to_string());
        let p = resolve_extends(steps, parent, chain).map_err(|e| {
            if e.path.ends_with(".extends") && e.message.contains("is not a step") {
                Problem::new(&path, e.message)
            } else {
                e
            }
        })?;
        chain.pop();
        for (k, v) in p.iter() {
            merged.insert(k, v.clone());
        }
    }
    for (k, v) in own.iter() {
        merged.insert(k, v.clone());
    }
    Ok(merged)
}

/// Read a definition. `Err` lists every problem found. Only the built-in step types
/// are known; see [`parse_with`] for plugins.
pub fn parse(text: &str) -> Result<Definition, Vec<Problem>> {
    parse_with(text, Plugins::Named(&[]))
}

/// Read a definition that may use the step types plugins provide.
pub fn parse_with(text: &str, plugins: Plugins<'_>) -> Result<Definition, Vec<Problem>> {
    let doc: DocumentMut = text.parse().map_err(|e: toml_edit::TomlError| {
        vec![Problem::new("", format!("not valid TOML: {e}"))]
    })?;
    let mut out = vec![];
    let mut schema = None;
    let mut start = None;
    let mut explored_entry = None;
    let mut steps: Vec<Step> = vec![];
    let mut agents = BTreeMap::new();
    let mut resources = BTreeMap::new();

    for (k, v) in doc.iter() {
        match k {
            "schema" => match v.as_integer() {
                Some(n) if n >= 0 => schema = Some(n as u32),
                _ => out.push(Problem::new("schema", "must be a whole number")),
            },
            "start" => start = string(v, "start", &mut out),
            "explored_entry" => explored_entry = string(v, "explored_entry", &mut out),
            "step" => match v.as_table_like() {
                Some(t) => {
                    for (name, item) in t.iter() {
                        let has_parent = item
                            .as_table_like()
                            .is_some_and(|x| x.get("extends").is_some());
                        let resolved = if has_parent {
                            match resolve_extends(t, name, &mut vec![]) {
                                Ok(table) => Item::Table(table),
                                Err(p) => {
                                    out.push(p);
                                    continue;
                                }
                            }
                        } else {
                            item.clone()
                        };
                        if let Some(s) = parse_step(name, &resolved, plugins, &mut out) {
                            steps.push(s);
                        }
                    }
                }
                None => out.push(Problem::new(
                    "step",
                    "must be a table of steps, like [step.develop]",
                )),
            },
            "agent" => match v.as_table_like() {
                Some(t) => {
                    for (name, item) in t.iter() {
                        let base = format!("agent.{name}");
                        let mut p = AgentProfile::default();
                        match item.as_table_like() {
                            Some(pt) => {
                                for (pk, pv) in pt.iter() {
                                    match pk {
                                        "tier" => {
                                            p.tier = string(pv, &format!("{base}.tier"), &mut out)
                                        }
                                        "model" => {
                                            p.model = string(pv, &format!("{base}.model"), &mut out)
                                        }
                                        "effort" => {
                                            p.effort =
                                                string(pv, &format!("{base}.effort"), &mut out)
                                        }
                                        other => out.push(unknown_key(
                                            &base,
                                            other,
                                            &["tier", "model", "effort"],
                                        )),
                                    }
                                }
                                agents.insert(name.to_string(), p);
                            }
                            None => out.push(Problem::new(&base, "must be a table")),
                        }
                    }
                }
                None => out.push(Problem::new(
                    "agent",
                    "must be a table of profiles, like [agent.light]",
                )),
            },
            "resource" => match v.as_table_like() {
                Some(t) => {
                    for (name, item) in t.iter() {
                        let count = item
                            .as_table_like()
                            .and_then(|r| r.get("count"))
                            .and_then(|c| c.as_integer())
                            .unwrap_or(1);
                        resources.insert(name.to_string(), count.max(1) as u32);
                    }
                }
                None => out.push(Problem::new(
                    "resource",
                    "must be a table, like [resource.phone]",
                )),
            },
            // Read together below.
            "suite" | "environment" | "binding" | "impact" | "signature" => {}
            other => out.push(unknown_key("", other, TOP_KEYS)),
        }
    }
    let testing = crate::testing_parse::parse(&doc, &mut out);
    if schema.is_none() && doc.get("schema").is_none() {
        out.push(Problem::new(
            "schema",
            "required; this version reads `schema = 1`",
        ));
    }
    if start.is_none() && doc.get("start").is_none() {
        out.push(Problem::new(
            "start",
            "required: the name of the first step",
        ));
    }
    if !out.is_empty() {
        return Err(out);
    }
    Ok(Definition {
        schema: schema.unwrap_or(0),
        start: start.unwrap_or_default(),
        explored_entry,
        steps,
        agents,
        resources,
        testing,
        hash: hash(text),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mason_core::State;

    const OK: &str = r#"
schema = 1
start = "explore"
explored_entry = "develop"

[agent.fast]
model = "haiku"

[resource.phone]
count = 2

[step.explore]
type = "agent"
category = "exploring"
agent = "fast"
next = "develop"

[step.develop]
type = "agent"
category = "developing"
budget = "4h"
tools = ["submit_change", "run_check"]
next = "test"

[step.test]
type = "test"
category = "testing"
command = "make test"
max_rounds = 2
on_red = "develop"
uses = { phone = 1 }
next = "land"

[step.land]
type = "landing"
category = "landing"
"#;

    #[test]
    fn a_full_definition_parses_into_typed_steps_in_file_order() {
        let d = parse(OK).unwrap();
        assert_eq!(
            (d.schema, d.start.as_str(), d.explored_entry.as_deref()),
            (1, "explore", Some("develop"))
        );
        assert_eq!(
            d.steps.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            ["explore", "develop", "test", "land"]
        );
        let t = d.step("test").unwrap();
        assert_eq!(
            (t.kind, t.category, t.max_rounds, t.on_red.as_deref()),
            (StepType::Test, State::Testing, Some(2), Some("develop"))
        );
        assert_eq!(t.uses["phone"], 1);
        assert_eq!(
            d.step("develop").unwrap().budget,
            Some(std::time::Duration::from_secs(14400))
        );
        assert_eq!(d.step("explore").unwrap().next, vec!["develop"]);
        assert_eq!(d.agents["fast"].model.as_deref(), Some("haiku"));
        assert_eq!(d.resources["phone"], 2);
        assert_eq!(d.hash, hash(OK));
    }

    #[test]
    fn mistakes_are_all_reported_with_the_key_they_are_about() {
        let bad = r#"
schema = 1
start = "a"
strat = "typo"
[step.a]
type = "robot"
category = "sideways"
nxt = "b"
max_rounds = 0
budget = "soon"
tools = "submit_plan"
read_only = "yes"
[step.B]
type = "agent"
category = "exploring"
"#;
        let e = parse(bad).unwrap_err();
        let has =
            |path: &str, frag: &str| e.iter().any(|p| p.path == path && p.message.contains(frag));
        assert!(
            has("", "unknown key `strat`; did you mean `start`?"),
            "{e:?}"
        );
        assert!(has("step.a.type", "unknown step type `robot`"), "{e:?}");
        assert!(
            has("step.a.category", "unknown category `sideways`"),
            "{e:?}"
        );
        assert!(
            has("step.a", "unknown key `nxt`; did you mean `next`?"),
            "{e:?}"
        );
        assert!(has("step.a.max_rounds", "1 or more"), "{e:?}");
        assert!(has("step.a.budget", "not a duration"), "{e:?}");
        assert!(has("step.a.tools", "list of strings"), "{e:?}");
        assert!(has("step.a.read_only", "true or false"), "{e:?}");
        assert!(has("step.B", "lowercase"), "{e:?}");
    }

    #[test]
    fn required_things_and_broken_toml() {
        let e = parse("[step.a]\ntype = \"agent\"\n").unwrap_err();
        assert!(e.iter().any(|p| p.path == "schema"));
        assert!(e.iter().any(|p| p.path == "start"));
        assert!(e
            .iter()
            .any(|p| p.path == "step.a.category" && p.message == "required"));
        let e = parse("schema = = 1").unwrap_err();
        assert!(e[0].message.contains("not valid TOML"));
    }
}
