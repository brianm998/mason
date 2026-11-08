// SPDX-License-Identifier: GPL-3.0-or-later

//! The five configuration layers, lowest to highest (Part 5):
//!
//! 1. built-in defaults from the running version,
//! 2. the project's `.mason/`, read from the base branch,
//! 3. the local overlay (`~/.config/mason/projects/<id>.toml`): machine facts,
//! 4. runtime overrides from the monitor or CLI, each with an owner and an expiry,
//! 5. per-task overrides.
//!
//! Layers 1 and 2 are the definition (the graph); layers 3 to 5 may only set
//! attributes of steps, never change the graph. Every resolved value remembers
//! which layer it came from, so "why is this using opus?" has an answer.

use crate::builtin::default_profile;
use crate::model::*;
use crate::tiers::{Level, Tier};
use std::collections::BTreeMap;
use std::time::Duration;
use toml_edit::DocumentMut;

/// The step attributes the upper layers may set.
pub const OVERRIDABLE: &[&str] = &["model", "effort", "budget", "max_rounds", "command"];

#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    Builtin,
    Project,
    Overlay,
    Runtime { owner: String, expires_at: u64 },
    Task,
}

impl Source {
    pub fn label(&self) -> String {
        match self {
            Source::Builtin => "built-in default".into(),
            Source::Project => "the project's .mason/mason.toml".into(),
            Source::Overlay => "the local overlay".into(),
            Source::Runtime { owner, expires_at } => {
                format!("a runtime override by {owner}, expiring at {expires_at}")
            }
            Source::Task => "a per-task override".into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Attr<T> {
    pub value: T,
    pub source: Source,
}

/// What the local overlay file says about the pipeline (the rest of that file,
/// paths and remotes, is the daemon's own business).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Overlay {
    /// Model for every agent step on this machine.
    pub model: Option<String>,
    /// The test command for test steps that name none.
    pub test_command: Option<String>,
    /// Per-step attributes: `[step.<name>] model = "opus"`.
    pub steps: BTreeMap<String, BTreeMap<String, String>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RuntimeOverride {
    pub key: String,
    pub value: String,
    pub owner: String,
    pub expires_at: u64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Resolved {
    /// The tier asked for (agent steps only).
    pub tier: Option<Attr<Tier>>,
    /// The level asked for: `medium` unless something says otherwise.
    pub level: Option<Attr<Level>>,
    /// A real model named explicitly (profile, step, overlay, override). With
    /// none, the provider's mapping decides (see [`crate::tiers::launch`]).
    pub model: Option<Attr<String>>,
    /// A real, provider-specific effort named explicitly. Abstract levels go to `level`.
    pub effort: Option<Attr<String>>,
    pub budget: Option<Attr<Duration>>,
    pub max_rounds: Option<Attr<u32>>,
    pub command: Option<Attr<String>>,
}

/// Is `value` acceptable for `attr`?
pub fn check_value(attr: &str, value: &str) -> Result<(), String> {
    match attr {
        "model" | "effort" if value.trim().is_empty() => Err(format!("{attr} must not be empty")),
        "command" if value.trim().is_empty() => Err("command must not be empty".into()),
        "budget" if parse_duration(value).is_none() => Err(format!(
            "`{value}` is not a duration; use forms like 4h, 30m, 90s"
        )),
        "max_rounds" if !value.parse::<u32>().is_ok_and(|n| n >= 1) => {
            Err(format!("`{value}` is not a whole number, 1 or more"))
        }
        _ => Ok(()),
    }
}

/// Validate an override key (`step.<name>.<attribute>`) and value against a
/// definition. Anything that would change the graph is refused.
pub fn check_override(
    def: &Definition,
    key: &str,
    value: &str,
) -> Result<(String, String), String> {
    let parts: Vec<&str> = key.split('.').collect();
    let ["step", step, attr] = parts[..] else {
        return Err(format!(
            "`{key}` is not an override key; use step.<name>.<attribute>, with attribute one of {}",
            OVERRIDABLE.join(", ")
        ));
    };
    if def.step(step).is_none() {
        let names: Vec<&str> = def.steps.iter().map(|s| s.name.as_str()).collect();
        return Err(format!(
            "there is no step `{step}`; steps are {}",
            names.join(", ")
        ));
    }
    if !OVERRIDABLE.contains(&attr) {
        let why = if matches!(
            attr,
            "type" | "category" | "next" | "on_red" | "join" | "tools" | "when" | "uses" | "gates"
        ) {
            " (it changes the graph or the gates; edit .mason/mason.toml and land the change)"
        } else {
            ""
        };
        return Err(format!(
            "`{attr}` cannot be overridden{why}; overridable: {}",
            OVERRIDABLE.join(", ")
        ));
    }
    check_value(attr, value)?;
    Ok((step.to_string(), attr.to_string()))
}

/// Read the layer-3 keys out of a project's overlay file. Other keys are the
/// daemon's and are ignored here. A `[step.x]` table may set attributes only.
pub fn parse_overlay(text: &str) -> Result<Overlay, Vec<Problem>> {
    let doc: DocumentMut = text.parse().map_err(|e: toml_edit::TomlError| {
        vec![Problem::new("", format!("not valid TOML: {e}"))]
    })?;
    let mut o = Overlay::default();
    let mut out = vec![];
    o.model = doc.get("model").and_then(|v| v.as_str()).map(String::from);
    o.test_command = doc.get("test").and_then(|v| v.as_str()).map(String::from);
    if let Some(steps) = doc.get("step").and_then(|s| s.as_table_like()) {
        for (name, item) in steps.iter() {
            let Some(t) = item.as_table_like() else {
                out.push(Problem::new(format!("step.{name}"), "must be a table"));
                continue;
            };
            for (k, v) in t.iter() {
                let path = format!("step.{name}.{k}");
                if !OVERRIDABLE.contains(&k) {
                    let structural = matches!(
                        k,
                        "type"
                            | "category"
                            | "next"
                            | "on_red"
                            | "join"
                            | "tools"
                            | "when"
                            | "uses"
                            | "agent"
                            | "suites"
                            | "gates"
                            | "read_only"
                            | "policy"
                    );
                    let why = if structural {
                        "the local overlay cannot change the graph or the gates; that belongs in .mason/mason.toml"
                    } else {
                        "unknown key"
                    };
                    out.push(Problem::new(
                        &path,
                        format!("{why}; overlay keys are {}", OVERRIDABLE.join(", ")),
                    ));
                    continue;
                }
                let text = match (v.as_str(), v.as_integer()) {
                    (Some(s), _) => s.to_string(),
                    (_, Some(n)) => n.to_string(),
                    _ => {
                        out.push(Problem::new(&path, "must be a string or a number"));
                        continue;
                    }
                };
                if let Err(e) = check_value(k, &text) {
                    out.push(Problem::new(&path, e));
                    continue;
                }
                o.steps
                    .entry(name.to_string())
                    .or_default()
                    .insert(k.to_string(), text);
            }
        }
    }
    if out.is_empty() {
        Ok(o)
    } else {
        Err(out)
    }
}

/// An abstract level sets the requested level; anything else is a real effort
/// of the running provider and replaces whatever was asked for below it.
fn set_effort(r: &mut Resolved, value: &str, source: Source) {
    match Level::parse(value) {
        Some(l) => {
            r.effort = None;
            set(&mut r.level, l, source);
        }
        None => {
            set(&mut r.effort, value.to_string(), source);
        }
    }
}

fn set<T>(slot: &mut Option<Attr<T>>, value: T, source: Source) {
    *slot = Some(Attr { value, source });
}

/// Merge the layers for one step. Later layers win, attribute by attribute.
pub fn resolve(
    def: &Definition,
    step: &Step,
    overlay: &Overlay,
    runtime: &[RuntimeOverride],
    task: &[(String, String)],
    now_ms: u64,
) -> Result<Resolved, Vec<Problem>> {
    let mut r = Resolved::default();
    let mut problems = vec![];

    // 1 and 2: the profile an agent step uses, then the step's own keys.
    // A profile's or step's real model is explicit: it beats the project's
    // `model` key (the overlay's blanket model) but not the owner's overrides.
    let mut explicit_model = false;
    if step.kind == StepType::Agent {
        let profile = step
            .agent
            .clone()
            .unwrap_or_else(|| default_profile(step.category).to_string());
        let own = def.agents.get(&profile);
        let tier = own
            .and_then(|p| p.tier.as_deref())
            .and_then(Tier::parse)
            .or_else(|| Tier::parse(&profile))
            .or_else(|| Tier::parse(default_profile(step.category)))
            .unwrap_or(Tier::Light);
        let src = if own.is_some() {
            Source::Project
        } else {
            Source::Builtin
        };
        set(&mut r.tier, tier, src.clone());
        set(&mut r.level, Level::Medium, Source::Builtin);
        if let Some(p) = own {
            if let Some(m) = &p.model {
                set(&mut r.model, m.clone(), Source::Project);
                explicit_model = true;
            }
            if let Some(e) = &p.effort {
                set_effort(&mut r, e, Source::Project);
            }
        }
    }
    if let Some(m) = &step.model {
        set(&mut r.model, m.clone(), Source::Project);
        explicit_model = true;
    }
    if let Some(e) = &step.effort {
        set_effort(&mut r, e, Source::Project);
    }
    if let Some(b) = step.budget {
        set(&mut r.budget, b, Source::Project);
    }
    if let Some(n) = step.max_rounds {
        set(&mut r.max_rounds, n, Source::Project);
    }
    if let Some(c) = &step.command {
        set(&mut r.command, c.clone(), Source::Project);
    }

    // 3: the local overlay.
    if step.kind == StepType::Agent {
        if let (Some(m), false) = (&overlay.model, explicit_model) {
            set(&mut r.model, m.clone(), Source::Overlay);
        }
    }
    if step.kind == StepType::Test && step.command.is_none() {
        if let Some(c) = &overlay.test_command {
            set(&mut r.command, c.clone(), Source::Overlay);
        }
    }
    let apply =
        |attr: &str, value: &str, source: Source, r: &mut Resolved, problems: &mut Vec<Problem>| {
            if let Err(e) = check_value(attr, value) {
                problems.push(Problem::new(format!("step.{}.{attr}", step.name), e));
                return;
            }
            match attr {
                "model" => set(&mut r.model, value.to_string(), source),
                "effort" => set_effort(r, value, source),
                "command" => set(&mut r.command, value.to_string(), source),
                "budget" => set(
                    &mut r.budget,
                    parse_duration(value).expect("checked"),
                    source,
                ),
                "max_rounds" => set(&mut r.max_rounds, value.parse().expect("checked"), source),
                _ => {}
            }
        };
    if let Some(t) = overlay.steps.get(&step.name) {
        for (k, v) in t {
            apply(k, v, Source::Overlay, &mut r, &mut problems);
        }
    }

    // 4: runtime overrides still in force, oldest first so a newer one wins.
    for o in runtime.iter().filter(|o| o.expires_at > now_ms) {
        if let Ok((s, attr)) = check_override(def, &o.key, &o.value) {
            if s == step.name {
                apply(
                    &attr,
                    &o.value,
                    Source::Runtime {
                        owner: o.owner.clone(),
                        expires_at: o.expires_at,
                    },
                    &mut r,
                    &mut problems,
                );
            }
        }
    }

    // 5: this task's own overrides.
    for (k, v) in task {
        if let Ok((s, attr)) = check_override(def, k, v) {
            if s == step.name {
                apply(&attr, v, Source::Task, &mut r, &mut problems);
            }
        }
    }

    if problems.is_empty() {
        Ok(r)
    } else {
        Err(problems)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin::builtin;

    fn rt(key: &str, v: &str, owner: &str, exp: u64) -> RuntimeOverride {
        RuntimeOverride {
            key: key.into(),
            value: v.into(),
            owner: owner.into(),
            expires_at: exp,
        }
    }

    #[test]
    fn each_layer_beats_the_one_below_and_remembers_where_it_came_from() {
        let d = builtin();
        let dev = d.step("develop").unwrap();
        let none = Overlay::default();

        // 1: the built-in "deep" profile.
        let r = resolve(&d, dev, &none, &[], &[], 0).unwrap();
        assert_eq!(r.model, None, "no real model is named; the mapping decides");
        assert_eq!(r.tier.as_ref().unwrap().value, Tier::Deep);
        let l = crate::tiers::launch(&r, &crate::tiers::Mapping::builtin("claude"), &[]);
        assert_eq!(l.model.as_deref(), Some("sonnet"));
        assert_eq!(l.effort, None);
        // The definition's budget is the project's.
        assert_eq!(r.budget.as_ref().unwrap().source, Source::Project);

        // 3: the overlay's blanket model, then its step table.
        let overlay = parse_overlay("model = \"opus\"\n[step.develop]\nbudget = \"2h\"\n").unwrap();
        let r = resolve(&d, dev, &overlay, &[], &[], 0).unwrap();
        assert_eq!(
            r.model,
            Some(Attr {
                value: "opus".into(),
                source: Source::Overlay
            })
        );
        assert_eq!(r.budget.unwrap().value, Duration::from_secs(7200));

        // 4: a runtime override beats the overlay, until it expires.
        let ro = [rt("step.develop.model", "haiku", "brian", 1_000)];
        let r = resolve(&d, dev, &overlay, &ro, &[], 500).unwrap();
        assert_eq!(r.model.as_ref().unwrap().value, "haiku");
        assert!(
            matches!(&r.model.unwrap().source, Source::Runtime { owner, expires_at: 1_000 } if owner == "brian")
        );
        let r = resolve(&d, dev, &overlay, &ro, &[], 1_000).unwrap();
        assert_eq!(
            r.model.unwrap().value,
            "opus",
            "expired: the layer below shows through again"
        );

        // 5: per-task beats everything.
        let task = [("step.develop.model".to_string(), "sonnet-x".to_string())];
        let r = resolve(&d, dev, &overlay, &ro, &task, 500).unwrap();
        assert_eq!(
            r.model,
            Some(Attr {
                value: "sonnet-x".into(),
                source: Source::Task
            })
        );
    }

    #[test]
    fn a_project_profile_overrides_the_builtin_one_and_a_step_model_overrides_the_profile() {
        let d = crate::parse(&format!(
            "{}\n[agent.deep]\nmodel = \"big\"\n",
            crate::builtin::BUILTIN
                .replace("budget = \"4h\"", "budget = \"4h\"\nmodel = \"special\"")
        ))
        .unwrap();
        let r = resolve(
            &d,
            d.step("develop").unwrap(),
            &Overlay::default(),
            &[],
            &[],
            0,
        )
        .unwrap();
        assert_eq!(
            r.model,
            Some(Attr {
                value: "special".into(),
                source: Source::Project
            })
        );
        let d2 = crate::parse(&format!(
            "{}\n[agent.deep]\nmodel = \"big\"\n",
            crate::builtin::BUILTIN
        ))
        .unwrap();
        let r2 = resolve(
            &d2,
            d2.step("develop").unwrap(),
            &Overlay::default(),
            &[],
            &[],
            0,
        )
        .unwrap();
        assert_eq!(
            r2.model,
            Some(Attr {
                value: "big".into(),
                source: Source::Project
            })
        );
    }

    #[test]
    fn the_overlay_supplies_the_test_command_only_where_the_step_names_none() {
        let d = builtin();
        let overlay = parse_overlay("test = \"cargo test\"\n").unwrap();
        let r = resolve(&d, d.step("test").unwrap(), &overlay, &[], &[], 0).unwrap();
        assert_eq!(
            r.command,
            Some(Attr {
                value: "cargo test".into(),
                source: Source::Overlay
            })
        );
        let with = crate::parse(
            &crate::builtin::BUILTIN
                .replace("max_rounds = 3", "max_rounds = 3\ncommand = \"make check\""),
        )
        .unwrap();
        let r = resolve(&with, with.step("test").unwrap(), &overlay, &[], &[], 0).unwrap();
        assert_eq!(
            r.command.unwrap().value,
            "make check",
            "the project's own command wins over a machine default"
        );
    }

    #[test]
    fn upper_layers_can_never_change_the_graph() {
        let d = builtin();
        for (k, why) in [
            ("step.test.next", "changes the graph"),
            ("step.test.on_red", "changes the graph"),
            ("step.develop.tools", "gates"),
            ("step.test.type", "changes the graph"),
        ] {
            let e = check_override(&d, k, "x").unwrap_err();
            assert!(
                e.contains("cannot be overridden") && e.contains(why),
                "{k}: {e}"
            );
        }
        assert!(check_override(&d, "step.nope.model", "x")
            .unwrap_err()
            .contains("there is no step"));
        assert!(check_override(&d, "model", "x")
            .unwrap_err()
            .contains("not an override key"));
        assert!(check_override(&d, "step.test.max_rounds", "0").is_err());
        assert!(check_override(&d, "step.develop.budget", "soon").is_err());
        assert!(check_override(&d, "step.develop.budget", "30m").is_ok());

        let e = parse_overlay("[step.test]\nnext = \"develop\"\n").unwrap_err();
        assert!(e[0].message.contains("cannot change the graph"), "{e:?}");
        let e = parse_overlay("[step.test]\nbogus = 1\n").unwrap_err();
        assert!(e[0].message.contains("unknown key"));
        assert!(
            parse_overlay("path = \"/r\"\nbase = \"main\"\nremote = \"origin\"\n").is_ok(),
            "the daemon's own keys are not ours to judge"
        );
    }

    #[test]
    fn invalid_values_in_a_layer_are_reported_not_applied() {
        let d = builtin();
        let task = [("step.test.max_rounds".to_string(), "0".to_string())];
        // check_override filters it, so resolve ignores it rather than applying nonsense.
        let r = resolve(
            &d,
            d.step("test").unwrap(),
            &Overlay::default(),
            &[],
            &task,
            0,
        )
        .unwrap();
        assert_eq!(r.max_rounds.unwrap().value, 3);
    }

    #[test]
    fn effort_is_a_level_or_a_real_effort_and_fast_is_light() {
        let d = crate::parse(
            &crate::builtin::BUILTIN
                .replace("agent = \"light\"", "agent = \"fast\"\neffort = \"high\""),
        )
        .unwrap();
        let ex = d.step("explore").unwrap();
        let r = resolve(&d, ex, &Overlay::default(), &[], &[], 0).unwrap();
        assert_eq!(r.tier.as_ref().unwrap().value, Tier::Light);
        assert_eq!(r.level.as_ref().unwrap().value, Level::High);
        assert_eq!(r.effort, None);
        // A provider's own effort is kept as a real effort and wins over the mapped one.
        let ro = [rt("step.explore.effort", "max", "brian", 9)];
        let r = resolve(&d, ex, &Overlay::default(), &ro, &[], 0).unwrap();
        assert_eq!(r.effort.as_ref().unwrap().value, "max");
        // A later abstract level replaces it.
        let task = [("step.explore.effort".to_string(), "low".to_string())];
        let r = resolve(&d, ex, &Overlay::default(), &ro, &task, 0).unwrap();
        assert_eq!((r.effort, r.level.unwrap().value), (None, Level::Low));
    }

    #[test]
    fn a_step_model_beats_the_project_model_key_and_both_beat_the_mapping() {
        let d = crate::parse(
            &crate::builtin::BUILTIN
                .replace("budget = \"4h\"", "budget = \"4h\"\nmodel = \"special\""),
        )
        .unwrap();
        let overlay = parse_overlay("model = \"keyed\"\n").unwrap();
        let dev = d.step("develop").unwrap();
        let m = crate::tiers::Mapping::builtin("claude");
        let go = |r: &Resolved| crate::tiers::launch(r, &m, &["bottom".to_string()]).model;
        let r = resolve(&d, dev, &overlay, &[], &[], 0).unwrap();
        assert_eq!(go(&r).as_deref(), Some("special"), "step beats project key");
        let ro = [rt("step.develop.model", "owner", "brian", 9)];
        let r = resolve(&d, dev, &overlay, &ro, &[], 0).unwrap();
        assert_eq!(
            go(&r).as_deref(),
            Some("owner"),
            "runtime override beats the step"
        );
        let b = builtin();
        let dev = b.step("develop").unwrap();
        let r = resolve(&b, dev, &overlay, &[], &[], 0).unwrap();
        assert_eq!(
            go(&r).as_deref(),
            Some("keyed"),
            "project key beats the mapping"
        );
        let r = resolve(&b, dev, &Overlay::default(), &[], &[], 0).unwrap();
        assert_eq!(
            go(&r).as_deref(),
            Some("sonnet"),
            "mapping beats the ladder"
        );
        let none = crate::tiers::Mapping::default();
        assert_eq!(
            crate::tiers::launch(&r, &none, &["bottom".to_string()])
                .model
                .as_deref(),
            Some("bottom"),
            "the ladder's bottom when nothing maps"
        );
        assert_eq!(crate::tiers::launch(&r, &none, &[]).model, None);
    }
}
