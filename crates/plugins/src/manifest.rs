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
//! Unknown keys are mistakes. Protocol 3 adds what else a plugin may provide:
//! `adapter:tracker:<name>`, `adapter:host:<name>`, `adapter:ci:<name>`,
//! `adapter:agent:<name>` (recorded, not yet run), `parser:<name>` and
//! `environment:<name>`. A protocol 1 or 2 manifest that lists one is refused.

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
/// The adapter kinds a plugin may provide. `agent` is accepted in a manifest but nothing
/// loads it yet (decision 0286).
pub const ADAPTER_KINDS: &[&str] = &["tracker", "host", "ci", "agent"];
pub const CATEGORIES: &[&str] = &["exploring", "developing", "testing", "review", "landing"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    pub protocol: u32,
    pub description: String,
    /// The step types it provides (the names after `step-type:`).
    pub step_types: Vec<String>,
    /// The other things it provides (protocol 3), as written: `adapter:tracker:<name>`,
    /// `parser:<name>`, `environment:<name>` and so on.
    pub extras: Vec<String>,
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
            .chain(self.extras.iter().cloned())
            .collect()
    }

    /// Does it provide exactly this (`parser:junit`, `adapter:tracker:acme`)?
    pub fn provides_extra(&self, what: &str) -> bool {
        self.extras.iter().any(|e| e == what)
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
        extras: vec![],
        run: vec![],
        config_schema: None,
        categories: vec!["developing".into(), "testing".into()],
    };
    let mut provides_seen = false;
    let mut extras: Vec<String> = vec![];
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
                                Some(("parser" | "environment", n)) if valid_name(n) => {
                                    extras.push(s.to_string())
                                }
                                Some(("adapter", rest))
                                    if rest.split_once(':').is_some_and(|(k, n)| {
                                        ADAPTER_KINDS.contains(&k) && valid_name(n)
                                    }) =>
                                {
                                    extras.push(s.to_string())
                                }
                                Some(("adapter", _)) => bad.push(format!(
                                    "`{s}`: an adapter is `adapter:<kind>:<name>` with kind one of {}",
                                    ADAPTER_KINDS.join(", ")
                                )),
                                Some(("parser" | "environment", _)) => bad.push(format!(
                                    "`{s}`: a name is lowercase letters, digits, - and _"
                                )),
                                _ => bad.push(format!(
                                    "`{s}` is not something a plugin can provide; use \"step-type:<name>\", \"adapter:<kind>:<name>\", \"parser:<name>\" or \"environment:<name>\""
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
    let mut seen = std::collections::BTreeSet::new();
    for e in &extras {
        if !seen.insert(e.clone()) {
            bad.push(format!("`provides` lists `{e}` twice"));
        }
    }
    if !extras.is_empty() && m.protocol != 0 && m.protocol < 3 {
        bad.push(format!(
            "`{}`: providing an adapter, parser or environment needs protocol 3; the manifest says protocol {}",
            extras[0], m.protocol
        ));
    }
    m.extras = extras;
    if m.name.is_empty() && doc.get("name").is_none() {
        bad.push("`name` is required".into());
    }
    if !provides_seen {
        bad.push("`provides` is required".into());
    } else if m.step_types.is_empty() && m.extras.is_empty() && bad.is_empty() {
        bad.push("`provides` must list at least one thing".into());
    }
    if m.run.is_empty() && doc.get("run").is_none() {
        bad.push("`run` is required: the program to start, like \"./check.py\"".into());
    }
    match doc.get("protocol") {
        None => {
            bad.push("`protocol` is required (this version speaks protocols 1, 2 and 3)".into())
        }
        Some(_) if m.protocol != 0 && !crate::SUPPORTED.contains(&m.protocol) => bad.push(format!(
            "protocol {} is not supported; this version speaks protocols 1, 2 and 3",
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
        let e = parse_manifest("nme = 1\nprotocol = 4\nprovides = []\n").unwrap_err();
        let has = |f: &str| e.iter().any(|x| x.contains(f));
        assert!(has("unknown key `nme`"), "{e:?}");
        assert!(has("protocol 4 is not supported"), "{e:?}");
        assert!(has("`name` is required"), "{e:?}");
        assert!(has("`run` is required"), "{e:?}");
        let e = parse_manifest("{{").unwrap_err();
        assert!(e[0].contains("not valid TOML"));
    }

    #[test]
    fn parsers_environments_and_adapters_need_protocol_3() {
        let all = [
            "parser:junit",
            "environment:vm",
            "adapter:tracker:jira",
            "adapter:host:gitlab",
            "adapter:ci:buildkite",
            "adapter:agent:local",
        ];
        for what in all {
            // Protocol 1 and 2 refuse each, with the reason.
            for old in ["protocol = 1", "protocol = 2"] {
                let t = OK
                    .replace("step-type:migration-check", what)
                    .replace("protocol = 1", old);
                let e = parse_manifest(&t).unwrap_err();
                assert!(
                    e.iter().any(|x| x.contains("needs protocol 3")),
                    "{what}: {e:?}"
                );
            }
            let t = OK
                .replace("step-type:migration-check", what)
                .replace("protocol = 1", "protocol = 3");
            let m = parse_manifest(&t).unwrap();
            assert_eq!(m.provides(), [what]);
            assert!(m.provides_extra(what));
            assert!(m.step_types.is_empty());
        }
        let v3 = OK.replace("protocol = 1", "protocol = 3");
        for bad in [
            "magic:x",
            "adapter:jira",
            "adapter:storage:x",
            "adapter:tracker:Bad",
            "parser:",
        ] {
            let e = parse_manifest(&v3.replace("step-type:migration-check", bad)).unwrap_err();
            assert!(!e.is_empty(), "{bad} must be refused");
        }
        let e = parse_manifest(&v3.replace("step-type:migration-check", "magic:x")).unwrap_err();
        assert!(e[0].contains("not something a plugin can provide"));
        let two = v3.replace(
            "[\"step-type:migration-check\"]",
            "[\"parser:a\", \"parser:a\"]",
        );
        assert!(parse_manifest(&two).is_err());
    }

    #[test]
    fn schema_paths_cannot_leave_the_folder() {
        for bad in ["../x.json", "/etc/passwd"] {
            let e = parse_manifest(&OK.replace("schema.json", bad)).unwrap_err();
            assert!(e[0].contains("inside the extension's folder"), "{e:?}");
        }
    }
}
