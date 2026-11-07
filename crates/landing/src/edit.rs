// SPDX-License-Identifier: GPL-3.0-or-later

//! Editing `.mason/landing.toml` for the graphical editor (Part 18): mode, batch
//! rules, tiers and the red ladder, written with `toml_edit` so comments and
//! ordering survive, and refused if the result is not a valid configuration.

use crate::config::{self, LandingConfig, Mode, Rung};
use toml_edit::{value, Array, ArrayOfTables, DocumentMut, Item, Table};

#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    /// A top-level key: `mode`, `base`, `window`, `max_size`, `parallel`, `max_fold`, `resolver`.
    Set { key: String, value: serde_json::Value },
    Unset { key: String },
    AddTier { name: String, suites: Vec<String> },
    RemoveTier { name: String },
    /// A key of a tier: `suites` (a list), `ci` or `on_red` (text).
    SetTier { name: String, key: String, value: serde_json::Value },
    /// The red ladder, in order.
    SetLadder { rungs: Vec<String> },
}

fn doc(text: &str) -> Result<DocumentMut, String> {
    text.parse().map_err(|e: toml_edit::TomlError| format!("not valid TOML: {e}"))
}

fn item(v: &serde_json::Value) -> Result<Item, String> {
    use serde_json::Value::*;
    Ok(match v {
        String(s) => value(s.as_str()),
        Bool(b) => value(*b),
        Number(n) => value(n.as_i64().ok_or("numbers here are whole numbers")?),
        Array(a) => {
            let mut arr = toml_edit::Array::new();
            for x in a {
                arr.push(x.as_str().ok_or("lists here hold text")?);
            }
            value(arr)
        }
        _ => return Err("not a value a landing file holds".into()),
    })
}

/// Set `key`, in place when it is there: the key's own spacing (alignment) and the
/// value's comment stay.
fn keep(t: &mut dyn toml_edit::TableLike, key: &str, mut new: Item) {
    if let Some(old) = t.get_mut(key) {
        if let (Some(o), Some(n)) = (old.as_value(), new.as_value_mut()) {
            *n.decor_mut() = o.decor().clone();
        }
        *old = new;
        return;
    }
    t.insert(key, new);
}

const KEYS: &[&str] = &["mode", "base", "window", "max_size", "parallel", "max_fold", "resolver"];

fn tiers_mut(d: &mut DocumentMut) -> &mut ArrayOfTables {
    if d.get("tier").and_then(|t| t.as_array_of_tables()).is_none() {
        d.insert("tier", Item::ArrayOfTables(ArrayOfTables::new()));
    }
    d.get_mut("tier").and_then(|t| t.as_array_of_tables_mut()).expect("just made")
}

/// The text after the operation. Whether it is *valid* is [`apply`]'s concern.
fn change(op: &Op, text: &str) -> Result<String, String> {
    let mut d = doc(text)?;
    match op {
        Op::Set { key, value: v } => {
            if !KEYS.contains(&key.as_str()) {
                return Err(format!("`{key}` is not a landing setting; they are {}", KEYS.join(", ")));
            }
            keep(d.as_table_mut(), key, item(v)?);
        }
        Op::Unset { key } => {
            d.remove(key);
        }
        Op::AddTier { name, suites } => {
            let tiers = tiers_mut(&mut d);
            if tiers.iter().any(|t| t.get("name").and_then(|n| n.as_str()) == Some(name)) {
                return Err(format!("there is already a tier `{name}`"));
            }
            let mut t = Table::new();
            t.insert("name", value(name.as_str()));
            let mut a = Array::new();
            for s in suites {
                a.push(s.as_str());
            }
            t.insert("suites", value(a));
            tiers.push(t);
        }
        Op::RemoveTier { name } => {
            let tiers = tiers_mut(&mut d);
            let i = tiers
                .iter()
                .position(|t| t.get("name").and_then(|n| n.as_str()) == Some(name))
                .ok_or_else(|| format!("there is no tier `{name}`"))?;
            tiers.remove(i);
            if tiers.is_empty() {
                d.remove("tier");
            }
        }
        Op::SetTier { name, key, value: v } => {
            if !matches!(key.as_str(), "suites" | "ci" | "on_red") {
                return Err(format!("`{key}` is not a tier setting; they are suites, ci and on_red"));
            }
            let tiers = tiers_mut(&mut d);
            let t = tiers
                .iter_mut()
                .find(|t| t.get("name").and_then(|n| n.as_str()) == Some(name))
                .ok_or_else(|| format!("there is no tier `{name}`"))?;
            if v.is_null() {
                t.remove(key);
            } else {
                keep(t, key, item(v)?);
            }
        }
        Op::SetLadder { rungs } => {
            for r in rungs {
                if Rung::from_name(r).is_none() {
                    return Err(format!("unknown rung `{r}`; the rungs are rerun-failed, blame-by-impact, bisect, ask"));
                }
            }
            let mut a = Array::new();
            for r in rungs {
                a.push(r.as_str());
            }
            if d.get("on_red").is_none() {
                d.insert("on_red", Item::Table(Table::new()));
            }
            let t = d.get_mut("on_red").and_then(|t| t.as_table_like_mut()).ok_or("`on_red` is not a table")?;
            keep(t, "ladder", value(a));
        }
    }
    Ok(d.to_string())
}

fn problems(text: &str) -> Vec<String> {
    match config::parse(text) {
        Ok(_) => vec![],
        Err(p) => p,
    }
}

/// Apply an operation. Refused if the result is not a valid configuration, or has a
/// problem the text did not have (editing a file that already has problems toward
/// a fix is allowed).
pub fn apply(op: &Op, text: &str) -> Result<String, Vec<String>> {
    let new = change(op, text).map_err(|e| vec![e])?;
    if new == text {
        return Ok(new);
    }
    let before = problems(text);
    let fresh: Vec<String> = problems(&new).into_iter().filter(|p| !before.contains(p)).collect();
    if fresh.is_empty() {
        Ok(new)
    } else {
        Err(fresh)
    }
}

/// What the landing editor shows: every setting, with the choices it may take.
pub fn form(text: &str) -> serde_json::Value {
    let (cfg, errors) = match config::parse(text) {
        Ok(c) => (c, vec![]),
        Err(e) => (LandingConfig::default(), e),
    };
    serde_json::json!({
        "mode": cfg.mode.name(),
        "modes": Mode::ALL.iter().map(|m| m.name()).collect::<Vec<_>>(),
        "base": cfg.base,
        "window_s": cfg.window.as_secs(),
        "max_size": cfg.max_size,
        "parallel": cfg.parallel,
        "max_fold": cfg.max_fold,
        "resolver": cfg.resolver,
        "tiers": cfg.tiers.iter().map(|t| serde_json::json!({"name": t.name, "suites": t.suites, "ci": t.ci, "on_red": t.on_red})).collect::<Vec<_>>(),
        "ladder": cfg.ladder.iter().map(|r| r.name()).collect::<Vec<_>>(),
        "rungs": ["rerun-failed", "blame-by-impact", "bisect", "ask"],
        "problems": errors,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const TEXT: &str = "\
# How this project lands.
mode     = \"direct\"   # no PRs
window   = \"20m\"
max_size = 5

[[tier]]
name   = \"batch\"
suites = [\"integration\"]

[on_red]
ladder = [\"rerun-failed\", \"bisect\", \"ask\"]
";

    #[test]
    fn a_setting_changes_in_place_and_comments_survive() {
        let t = apply(&Op::Set { key: "max_size".into(), value: json!(8) }, TEXT).unwrap();
        assert!(t.contains("# How this project lands.") && t.contains("# no PRs"), "{t}");
        let changed: Vec<_> = TEXT.lines().zip(t.lines()).filter(|(a, b)| a != b).collect();
        assert_eq!(changed, vec![("max_size = 5", "max_size = 8")]);
        let t = apply(&Op::Set { key: "mode".into(), value: json!("local-only") }, &t).unwrap();
        assert!(t.contains("mode     = \"local-only\"   # no PRs"), "{t}");
    }

    #[test]
    fn invalid_edits_are_refused_with_the_reason() {
        let e = apply(&Op::Set { key: "mode".into(), value: json!("yolo") }, TEXT).unwrap_err();
        assert!(e[0].contains("unknown `yolo`"), "{e:?}");
        let e = apply(&Op::Set { key: "parallel".into(), value: json!(0) }, TEXT).unwrap_err();
        assert!(e[0].contains("parallel"), "{e:?}");
        let e = apply(&Op::Set { key: "windo".into(), value: json!("1m") }, TEXT).unwrap_err();
        assert!(e[0].contains("not a landing setting"), "{e:?}");
        let e = apply(&Op::Set { key: "window".into(), value: json!("soon") }, TEXT).unwrap_err();
        assert!(e[0].contains("window"), "{e:?}");
        // A CI tier under a mode without CI is refused, pointing at the CI mode.
        let e = apply(&Op::SetTier { name: "batch".into(), key: "ci".into(), value: json!("github-actions") }, TEXT).unwrap_err();
        assert!(e[0].contains("direct-ci"), "{e:?}");
        // Switching the mode first makes it fine.
        let t = apply(&Op::Set { key: "mode".into(), value: json!("direct-ci") }, TEXT).unwrap();
        assert!(apply(&Op::SetTier { name: "batch".into(), key: "ci".into(), value: json!("github-actions") }, &t).is_ok());
    }

    #[test]
    fn tiers_and_the_ladder() {
        let t = apply(&Op::AddTier { name: "post-land".into(), suites: vec!["smoke".into()] }, TEXT).unwrap();
        assert!(t.contains("name = \"post-land\"") && t.contains("# How this project lands."), "{t}");
        assert!(apply(&Op::AddTier { name: "post-land".into(), suites: vec![] }, &t).is_err());
        let t = apply(&Op::SetTier { name: "post-land".into(), key: "on_red".into(), value: json!("freeze") }, &t).unwrap();
        assert!(t.contains("on_red = \"freeze\""));
        assert!(apply(&Op::SetTier { name: "post-land".into(), key: "on_red".into(), value: json!("explode") }, &t).is_err());
        let t = apply(&Op::SetTier { name: "batch".into(), key: "suites".into(), value: json!(["integration", "e2e"]) }, &t).unwrap();
        assert!(t.contains("suites = [\"integration\", \"e2e\"]"));
        let t = apply(&Op::RemoveTier { name: "post-land".into() }, &t).unwrap();
        assert!(!t.contains("post-land"));
        assert!(apply(&Op::RemoveTier { name: "ghost".into() }, &t).is_err());
        let t = apply(&Op::SetLadder { rungs: vec!["ask".into(), "bisect".into()] }, &t).unwrap();
        assert!(t.contains("ladder = [\"ask\", \"bisect\"]"), "{t}");
        assert!(apply(&Op::SetLadder { rungs: vec!["pray".into()] }, &t).is_err());
        // Starting from nothing works too.
        let t = apply(&Op::SetLadder { rungs: vec!["ask".into()] }, "").unwrap();
        assert!(t.contains("[on_red]"));
    }

    #[test]
    fn the_form_lists_every_setting_and_the_choices() {
        let f = form(TEXT);
        assert_eq!(f["mode"], "direct");
        assert_eq!(f["window_s"], 1200);
        assert_eq!(f["max_size"], 5);
        assert_eq!(f["tiers"][0]["suites"], json!(["integration"]));
        assert_eq!(f["ladder"], json!(["rerun-failed", "bisect", "ask"]));
        assert_eq!(f["modes"].as_array().unwrap().len(), 7);
        assert!(f["problems"].as_array().unwrap().is_empty());
        assert_eq!(form("mode = \"yolo\"")["problems"].as_array().unwrap().len(), 1);
        assert_eq!(form("")["mode"], "direct", "no file is the default");
    }
}
