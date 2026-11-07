// SPDX-License-Identifier: GPL-3.0-or-later
//! Step types that plugins provide (Stage 19): they parse only when a plugin provides
//! them, are checked against the plugin's categories and `config_schema`, and show up
//! in the editor's palette with a form made from the schema.

use mason_definition::editor::{palette, plugin_schema};
use mason_definition::{parse, parse_with, validate, Capabilities, Plugins, StepType};
use mason_plugins::PluginType;
use serde_json::json;

const TEXT: &str = r#"
schema = 1
start = "develop"

[step.develop]
type = "agent"
category = "developing"
next = "check"

[step.check]
type = "migration-check"
category = "testing"
directory = "migrations"
on_red = "develop"
max_rounds = 2
next = "land"

[step.land]
type = "landing"
category = "landing"
"#;

fn plugin() -> PluginType {
    PluginType {
        name: "migration-check".into(),
        plugin: "migration-check".into(),
        categories: vec!["testing".into(), "review".into()],
        config_schema: json!({
            "type": "object", "additionalProperties": false, "required": ["directory"],
            "properties": {"directory": {"type": "string", "title": "Directory"}, "strict": {"type": "boolean"}}
        }),
        about: "Checks migrations.".into(),
    }
}

fn caps() -> Capabilities {
    Capabilities::now().with_plugins(vec![plugin()])
}

fn names(d: &mason_definition::Definition) -> Vec<String> {
    validate(d, &caps()).iter().map(|p| p.to_string()).collect()
}

#[test]
fn a_plugin_type_parses_only_when_a_plugin_provides_it() {
    let e = parse(TEXT).unwrap_err();
    assert!(
        e.iter().any(|p| p.path == "step.check.type"
            && p.message.contains("unknown step type `migration-check`")),
        "{e:?}"
    );
    let d = parse_with(TEXT, Plugins::Named(&["migration-check".to_string()])).unwrap();
    let s = d.step("check").unwrap();
    assert_eq!(
        (s.kind, s.plugin.as_deref(), s.type_name()),
        (StepType::Plugin, Some("migration-check"), "migration-check")
    );
    assert_eq!(s.config["directory"], json!("migrations"));
    assert!(parse_with(TEXT, Plugins::Any).is_ok());
    assert!(names(&d).is_empty(), "{:?}", names(&d));
}

#[test]
fn settings_and_categories_are_checked_against_the_plugin() {
    let d = parse_with(
        &TEXT.replace("directory = \"migrations\"", "directory = 3"),
        Plugins::Any,
    )
    .unwrap();
    assert!(
        names(&d)
            .iter()
            .any(|p| p.contains("directory must be a string")),
        "{:?}",
        names(&d)
    );
    let d = parse_with(
        &TEXT.replace("directory = \"migrations\"", "dir = \"x\""),
        Plugins::Any,
    )
    .unwrap();
    let n = names(&d);
    assert!(
        n.iter().any(|p| p.contains("directory is required"))
            && n.iter().any(|p| p.contains("unknown setting `dir`")),
        "{n:?}"
    );
    let d = parse_with(
        &TEXT.replace("category = \"testing\"", "category = \"developing\""),
        Plugins::Any,
    )
    .unwrap();
    assert!(
        names(&d)
            .iter()
            .any(|p| p.contains("cannot be in `developing`; use testing or review")),
        "{:?}",
        names(&d)
    );
    // With no plugin providing the type, the definition says so.
    let d = parse_with(TEXT, Plugins::Any).unwrap();
    let p = validate(&d, &Capabilities::now());
    assert!(
        p.iter()
            .any(|p| p.message.contains("no extension provides the step type")),
        "{p:?}"
    );
}

#[test]
fn the_palette_lists_the_plugin_with_a_form_made_from_its_schema() {
    let all = palette(&caps());
    let built_in = palette(&Capabilities::now());
    assert_eq!(all.len(), built_in.len() + 1);
    let e = all.last().unwrap();
    assert_eq!(e["type"], "migration-check");
    assert_eq!(e["categories"], json!(["testing", "review"]));
    assert_eq!(
        e["fields"],
        json!({"directory": "x"}),
        "a new step is valid at once"
    );
    let form = &e["schema"];
    assert_eq!(form["properties"]["directory"]["title"], "Directory");
    assert_eq!(
        form["properties"]["category"]["enum"],
        json!(["testing", "review"])
    );
    assert_eq!(form["required"], json!(["directory"]));
    assert_eq!(form["x-order"][0], "category");
    assert_eq!(form["x-order"][1], "directory");
    let again = plugin_schema(&plugin(), &mason_definition::Definition::empty());
    assert_eq!(&again, form);
}
