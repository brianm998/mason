// SPDX-License-Identifier: GPL-3.0-or-later

//! `manifest.toml`: what an extension folder says about itself.
//!
//! ```toml
//! name = "data-migration-check"
//! version = "1.0.0"
//! protocol = 1
//! provides = ["step-type:migration-check"]
//! run = "./check.py"
//! config_schema = "schema.json"   # the editor builds its form from this
//! categories = ["testing"]
//! ```
//!
//! Unknown keys are mistakes. A plugin that provides something this version cannot
//! run (a parser, an environment or an adapter) is refused, with that reason.

use serde_json::Value;
use toml_edit::DocumentMut;

pub const MANIFEST_FILE: &str = "manifest.toml";
const KEYS: &[&str] = &[
    "name",
    "version",
    "protocol",
    "description",
    "provides",
    "run",
    "config_schema",
    "categories",
];
/// The categories a plugin step may sit in.
pub const CATEGORIES: &[&str] = &["exploring", "developing", "testing", "review", "landing"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    pub protocol: u32,
    pub description: String,
    /// The step types it provides (the names after `step-type:`).
    pub step_types: Vec<String>,
    /// The command line, program first, run in the extension's folder.
    pub run: Vec<String>,
    /// File name of the JSON Schema for the step's settings, if any.
    pub config_schema: Option<String>,
    pub categories: Vec<String>,
}

impl Manifest {
    /// The `provides` list as written.
    pub fn provides(&self) -> Vec<String> {
        self.step_types
            .iter()
            .map(|t| format!("step-type:{t}"))
            .collect()
    }
}

/// A step type a plugin adds, as the definition and the editor see it.
#[derive(Clone, Debug, PartialEq)]
pub struct PluginType {
    /// The step type's name, as written in `type = "..."`.
    pub name: String,
    /// The extension that provides it (its folder name).
    pub plugin: String,
    pub categories: Vec<String>,
    /// JSON Schema for the step's settings (`{"type":"object"}` if it has none).
    pub config_schema: Value,
    pub about: String,
}

fn valid_name(s: &str) -> bool {
    let mut c = s.chars();
    c.next().is_some_and(|f| f.is_ascii_lowercase())
        && c.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// Read a manifest. `Err` lists every problem.
pub fn parse_manifest(text: &str) -> Result<Manifest, Vec<String>> {
    let doc: DocumentMut = text
        .parse()
        .map_err(|e: toml_edit::TomlError| vec![format!("manifest.toml is not valid TOML: {e}")])?;
    let mut bad: Vec<String> = vec![];
    let mut m = Manifest {
        name: String::new(),
        version: "0".into(),
        protocol: 0,
        description: String::new(),
        step_types: vec![],
        run: vec![],
        config_schema: None,
        categories: vec!["developing".into(), "testing".into()],
    };
    let mut provides_seen = false;
    for (k, v) in doc.iter() {
        let string = |bad: &mut Vec<String>| match v.as_str() {
            Some(s) => Some(s.to_string()),
            None => {
                bad.push(format!("`{k}` must be a string"));
                None
            }
        };
        match k {
            "name" => {
                if let Some(s) = string(&mut bad) {
                    if valid_name(&s) {
                        m.name = s;
                    } else {
                        bad.push(
                            "`name` is lowercase letters, digits, - and _, starting with a letter"
                                .into(),
                        );
                    }
                }
            }
            "version" => m.version = string(&mut bad).unwrap_or_default(),
            "description" => m.description = string(&mut bad).unwrap_or_default(),
            "protocol" => match v.as_integer() {
                Some(n) if n >= 0 => m.protocol = n as u32,
                _ => bad.push("`protocol` must be a whole number".into()),
            },
            "run" => {
                if let Some(s) = string(&mut bad) {
                    m.run = s.split_whitespace().map(String::from).collect();
                }
            }
            "config_schema" => {
                if let Some(s) = string(&mut bad) {
                    if s.is_empty() || s.starts_with('/') || s.split('/').any(|p| p == "..") {
                        bad.push(
                            "`config_schema` is a file name inside the extension's folder".into(),
                        );
                    } else {
                        m.config_schema = Some(s);
                    }
                }
            }
            "categories" => match v.as_array() {
                Some(a) => {
                    let mut cats = vec![];
                    for x in a.iter() {
                        match x.as_str() {
                            Some(c) if CATEGORIES.contains(&c) => cats.push(c.to_string()),
                            _ => bad.push(format!(
                                "`categories` holds {}; each must be one of {}",
                                x,
                                CATEGORIES.join(", ")
                            )),
                        }
                    }
                    if cats.is_empty() {
                        bad.push("`categories` must name at least one category".into());
                    }
                    m.categories = cats;
                }
                None => bad.push("`categories` must be a list of strings".into()),
            },
            "provides" => {
                provides_seen = true;
                match v.as_array() {
                    Some(a) => {
                        for x in a.iter() {
                            let Some(s) = x.as_str() else {
                                bad.push("`provides` must be a list of strings".into());
                                continue;
                            };
                            match s.split_once(':') {
                                Some(("step-type", t)) if valid_name(t) => {
                                    if m.step_types.iter().any(|x| x == t) {
                                        bad.push(format!("`provides` lists `{s}` twice"));
                                    } else {
                                        m.step_types.push(t.to_string());
                                    }
                                }
                                Some(("step-type", _)) => bad.push(format!(
                                    "`{s}`: a step type's name is lowercase letters, digits, - and _"
                                )),
                                Some((kind @ ("parser" | "environment" | "adapter"), _)) => bad
                                    .push(format!(
                                    "`{s}`: providing a {kind} is not yet supported; only step-type plugins run in this version"
                                )),
                                _ => bad.push(format!(
                                    "`{s}` is not something a plugin can provide; use \"step-type:<name>\""
                                )),
                            }
                        }
                    }
                    None => bad.push("`provides` must be a list of strings".into()),
                }
            }
            other => bad.push(format!(
                "unknown key `{other}`; the keys are {}",
                KEYS.join(", ")
            )),
        }
    }
    if m.name.is_empty() && doc.get("name").is_none() {
        bad.push("`name` is required".into());
    }
    if !provides_seen {
        bad.push("`provides` is required".into());
    } else if m.step_types.is_empty() && bad.is_empty() {
        bad.push("`provides` must list at least one thing".into());
    }
    if m.run.is_empty() && doc.get("run").is_none() {
        bad.push("`run` is required: the program to start, like \"./check.py\"".into());
    }
    match doc.get("protocol") {
        None => bad.push("`protocol` is required (this version speaks protocols 1 and 2)".into()),
        Some(_) if m.protocol != 0 && !crate::SUPPORTED.contains(&m.protocol) => bad.push(format!(
            "protocol {} is not supported; this version speaks protocols 1 and 2",
            m.protocol
        )),
        _ => {}
    }
    if bad.is_empty() {
        Ok(m)
    } else {
        Err(bad)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OK: &str = r#"
name = "data-migration-check"
version = "1.2.0"
protocol = 1
provides = ["step-type:migration-check"]
run = "./check.py"
config_schema = "schema.json"
categories = ["testing"]
"#;

    #[test]
    fn a_manifest_parses() {
        let m = parse_manifest(OK).unwrap();
        assert_eq!(m.name, "data-migration-check");
        assert_eq!(m.step_types, ["migration-check"]);
        assert_eq!(m.run, ["./check.py"]);
        assert_eq!(m.config_schema.as_deref(), Some("schema.json"));
        assert_eq!(m.categories, ["testing"]);
        assert_eq!(m.provides(), ["step-type:migration-check"]);
    }

    #[test]
    fn every_mistake_is_reported() {
        let e = parse_manifest("nme = 1\nprotocol = 3\nprovides = []\n").unwrap_err();
        let has = |f: &str| e.iter().any(|x| x.contains(f));
        assert!(has("unknown key `nme`"), "{e:?}");
        assert!(has("protocol 3 is not supported"), "{e:?}");
        assert!(has("`name` is required"), "{e:?}");
        assert!(has("`run` is required"), "{e:?}");
        let e = parse_manifest("{{").unwrap_err();
        assert!(e[0].contains("not valid TOML"));
    }

    #[test]
    fn parsers_environments_and_adapters_are_refused_as_not_yet_supported() {
        for what in ["parser:junit", "environment:vm", "adapter:jira"] {
            let t = OK.replace("step-type:migration-check", what);
            let e = parse_manifest(&t).unwrap_err();
            assert!(
                e.iter().any(|x| x.contains("not yet supported")),
                "{what}: {e:?}"
            );
        }
        let e = parse_manifest(&OK.replace("step-type:migration-check", "magic:x")).unwrap_err();
        assert!(e[0].contains("not something a plugin can provide"));
    }

    #[test]
    fn schema_paths_cannot_leave_the_folder() {
        for bad in ["../x.json", "/etc/passwd"] {
            let e = parse_manifest(&OK.replace("schema.json", bad)).unwrap_err();
            assert!(e[0].contains("inside the extension's folder"), "{e:?}");
        }
    }
}
