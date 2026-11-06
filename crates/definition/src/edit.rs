// SPDX-License-Identifier: GPL-3.0-or-later

//! Writing a definition with `toml_edit`, so a person's comments, blank lines
//! and ordering survive a machine's edit and the diff stays small.

use crate::model::Problem;
use crate::validate::{validate, Capabilities};
use toml_edit::{value, Array, DocumentMut, Item};

/// A value to set.
#[derive(Clone, Debug)]
pub enum Val {
    Str(String),
    Int(i64),
    Bool(bool),
    List(Vec<String>),
}

fn to_item(v: &Val) -> Item {
    match v {
        Val::Str(s) => value(s.as_str()),
        Val::Int(n) => value(*n),
        Val::Bool(b) => value(*b),
        Val::List(l) => {
            let mut a = Array::new();
            for s in l {
                a.push(s.as_str());
            }
            value(a)
        }
    }
}

fn doc(text: &str) -> Result<DocumentMut, String> {
    text.parse()
        .map_err(|e: toml_edit::TomlError| format!("not valid TOML: {e}"))
}

/// Set `step.<name>.<key>`.
pub fn set_step_key(text: &str, step: &str, key: &str, v: &Val) -> Result<String, String> {
    let mut d = doc(text)?;
    let t = d
        .get_mut("step")
        .and_then(|s| s.get_mut(step))
        .and_then(|s| s.as_table_like_mut())
        .ok_or_else(|| format!("there is no step `{step}`"))?;
    t.insert(key, to_item(v));
    Ok(d.to_string())
}

/// Remove `step.<name>.<key>`; removing what is not there is not an error.
pub fn remove_step_key(text: &str, step: &str, key: &str) -> Result<String, String> {
    let mut d = doc(text)?;
    let t = d
        .get_mut("step")
        .and_then(|s| s.get_mut(step))
        .and_then(|s| s.as_table_like_mut())
        .ok_or_else(|| format!("there is no step `{step}`"))?;
    t.remove(key);
    Ok(d.to_string())
}

/// Set a top-level key such as `start` or `explored_entry`.
pub fn set_top_key(text: &str, key: &str, v: &Val) -> Result<String, String> {
    let mut d = doc(text)?;
    d.insert(key, to_item(v));
    Ok(d.to_string())
}

/// Add a step as its own `[step.<name>]` table at the end of the file.
pub fn add_step(text: &str, name: &str, kind: &str, category: &str) -> Result<String, String> {
    let mut d = doc(text)?;
    if d.get("step").and_then(|s| s.get(name)).is_some() {
        return Err(format!("there is already a step `{name}`"));
    }
    let steps = d.entry("step").or_insert_with(|| {
        let mut t = toml_edit::Table::new();
        t.set_implicit(true);
        Item::Table(t)
    });
    let steps = steps.as_table_mut().ok_or("`step` is not a table")?;
    let mut t = toml_edit::Table::new();
    t.insert("type", value(kind));
    t.insert("category", value(category));
    steps.insert(name, Item::Table(t));
    Ok(d.to_string())
}

pub fn remove_step(text: &str, name: &str) -> Result<String, String> {
    let mut d = doc(text)?;
    let steps = d
        .get_mut("step")
        .and_then(|s| s.as_table_like_mut())
        .ok_or("there are no steps")?;
    steps
        .remove(name)
        .ok_or_else(|| format!("there is no step `{name}`"))?;
    Ok(d.to_string())
}

/// Apply an edit and refuse the result if it is not a valid definition, so a
/// bad edit never reaches a file.
pub fn edit_checked(
    text: &str,
    caps: &Capabilities,
    f: impl FnOnce(&str) -> Result<String, String>,
) -> Result<String, Vec<Problem>> {
    let new = f(text).map_err(|e| vec![Problem::new("", e)])?;
    let def = crate::parse(&new)?;
    let problems = validate(&def, caps);
    if problems.is_empty() {
        Ok(new)
    } else {
        Err(problems)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "\
# The pipeline for web-api. Edited by hand; keep the comments.
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

# landing is last
[step.land]
type = \"landing\"
category = \"landing\"
";

    fn caps() -> Capabilities {
        Capabilities::now()
    }

    #[test]
    fn an_edit_keeps_every_comment_and_the_order_and_changes_one_line() {
        let out = set_step_key(TEXT, "test", "max_rounds", &Val::Int(5)).unwrap();
        for kept in [
            "# The pipeline for web-api. Edited by hand; keep the comments.",
            "# no exploring here",
            "# --- steps ---",
            "# then test",
            "# landing is last",
        ] {
            assert!(out.contains(kept), "lost {kept:?}:\n{out}");
        }
        let diff: Vec<(&str, &str)> = TEXT
            .lines()
            .zip(out.lines())
            .filter(|(a, b)| a != b)
            .collect();
        assert_eq!(
            diff,
            vec![("max_rounds = 2", "max_rounds = 5")],
            "a one-line diff"
        );
        assert_eq!(TEXT.lines().count(), out.lines().count());
        assert!(crate::parse(&out).is_ok());
    }

    #[test]
    fn adding_removing_and_setting_other_kinds_of_value() {
        let out = set_step_key(
            TEXT,
            "develop",
            "tools",
            &Val::List(vec!["submit_change".into(), "run_check".into()]),
        )
        .unwrap();
        assert!(
            out.contains("tools = [\"submit_change\", \"run_check\"]"),
            "{out}"
        );
        let out = set_step_key(&out, "develop", "read_only", &Val::Bool(false)).unwrap();
        assert!(out.contains("read_only = false"));
        let out = remove_step_key(&out, "develop", "read_only").unwrap();
        assert!(!out.contains("read_only"));
        let out = set_top_key(&out, "explored_entry", &Val::Str("develop".into())).unwrap();
        assert!(out.contains("explored_entry = \"develop\""));
        let out = add_step(&out, "extra", "command", "testing").unwrap();
        assert!(out.contains("[step.extra]") && out.trim_end().ends_with("category = \"testing\""));
        assert!(add_step(&out, "extra", "command", "testing").is_err());
        let out = remove_step(&out, "extra").unwrap();
        assert!(!out.contains("extra"));
        assert!(out.contains("# landing is last"), "{out}");
        assert!(remove_step(&out, "ghost").is_err());
        assert!(set_step_key(TEXT, "ghost", "x", &Val::Int(1)).is_err());
    }

    #[test]
    fn a_checked_edit_refuses_what_would_make_the_definition_invalid() {
        // Removing the bound on a loop.
        let e =
            edit_checked(TEXT, &caps(), |t| remove_step_key(t, "test", "max_rounds")).unwrap_err();
        assert!(e.iter().any(|p| p.path == "step.test.max_rounds"), "{e:?}");
        // Pointing next at nothing.
        let e = edit_checked(TEXT, &caps(), |t| {
            set_step_key(t, "develop", "next", &Val::Str("nowhere".into()))
        })
        .unwrap_err();
        assert!(
            e.iter()
                .any(|p| p.message.contains("`nowhere` is not a step")),
            "{e:?}"
        );
        // A good edit goes through.
        let ok = edit_checked(TEXT, &caps(), |t| {
            set_step_key(t, "test", "max_rounds", &Val::Int(4))
        })
        .unwrap();
        assert!(ok.contains("max_rounds = 4"));
    }
}
