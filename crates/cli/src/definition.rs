// SPDX-License-Identifier: GPL-3.0-or-later

//! `mason definition check` and `dry-run`: look at a pipeline definition
//! without applying it, and without a daemon.

use mason_definition::dryrun::{default_scenarios, dry_run, parse_scenarios, End, Trace};
use mason_definition::graph::Outcome;
use mason_definition::{parse, parse_with, validate, Capabilities, Plugins};
use mason_plugins::PluginType;
use std::path::Path;

fn read(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))
}

fn describe(t: &Trace) -> String {
    let path: Vec<String> = t
        .steps
        .iter()
        .map(|s| {
            if s.outcome == Outcome::Green {
                s.step.clone()
            } else {
                format!("{} ({})", s.step, s.outcome.name())
            }
        })
        .collect();
    let end = match &t.end {
        End::Landed => "landed".to_string(),
        End::Failed(why) => format!("failed: {why}"),
    };
    format!("{}  =>  {end}", path.join(" > "))
}

/// One line for each agent step whose prompt file is not in `dir`.
fn missing_prompts(def: &mason_definition::Definition, dir: &Path) -> Vec<String> {
    def.steps
        .iter()
        .filter_map(|s| s.prompt.as_ref().map(|f| (&s.name, f)))
        .filter(|(_, f)| !dir.join(f).is_file())
        .map(|(n, f)| format!("step.{n}.prompt: no file .mason/prompts/{f}"))
        .collect()
}

/// Validate a definition file: the language, then what this version can run,
/// then a dry run of the default scenarios. Returns the exit status.
///
/// `plugins` are the step types the project's approved extensions provide (`--project`);
/// without it only the built-in step types are known.
pub fn check(path: &Path, plugins: &[PluginType]) -> i32 {
    let text = match read(path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };
    let names: Vec<String> = plugins.iter().map(|p| p.name.clone()).collect();
    let def = match parse_with(&text, Plugins::Named(&names)) {
        Ok(d) => d,
        Err(problems) => {
            problems.iter().for_each(|p| println!("error  {p}"));
            return 1;
        }
    };
    let language = validate(
        &def,
        &Capabilities::everything().with_plugins(plugins.to_vec()),
    );
    language.iter().for_each(|p| println!("error  {p}"));
    let runnable = validate(&def, &Capabilities::now().with_plugins(plugins.to_vec()));
    for p in runnable.iter().filter(|p| !language.contains(p)) {
        println!("cannot run yet  {p}");
    }
    // A working-tree proxy: the daemon reads the file from the base commit.
    let dir = path.parent().unwrap_or(Path::new(".")).join("prompts");
    let missing = missing_prompts(&def, &dir);
    missing.iter().for_each(|m| println!("error  {m}"));
    if !language.is_empty() || !runnable.is_empty() || !missing.is_empty() {
        return 1;
    }
    println!(
        "ok  {} steps, definition {}",
        def.steps.len(),
        &def.hash[..12]
    );
    let mut status = 0;
    for sc in default_scenarios(&def) {
        match dry_run(&def, &sc) {
            Ok(t) => println!("    {:<45} {}", sc.name, describe(&t)),
            Err(e) => {
                println!("error  dry run `{}`: {e}", sc.name);
                status = 1;
            }
        }
    }
    status
}

/// Walk scripted tasks through a definition. With `scenarios`, those from the
/// file; otherwise the default ones.
pub fn dry(path: &Path, scenarios: Option<&Path>) -> i32 {
    let go = || -> Result<(), String> {
        let def = parse(&read(path)?).map_err(|e| {
            e.iter()
                .map(|p| p.to_string())
                .collect::<Vec<_>>()
                .join("\n")
        })?;
        let problems = validate(&def, &Capabilities::now());
        if !problems.is_empty() {
            return Err(problems
                .iter()
                .map(|p| p.to_string())
                .collect::<Vec<_>>()
                .join("\n"));
        }
        let list = match scenarios {
            Some(p) => parse_scenarios(&read(p)?)?,
            None => default_scenarios(&def),
        };
        for sc in list {
            let t = dry_run(&def, &sc)?;
            println!(
                "{}\n    started by {}: {}",
                sc.name,
                t.route.name(),
                describe(&t)
            );
        }
        Ok(())
    };
    match go() {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn a_missing_prompt_file_is_reported() {
        let text = "schema = 1\nstart = \"s\"\n[step.s]\ntype = \"agent\"\ncategory = \"exploring\"\nprompt = \"sec.md\"\nnext = \"l\"\n[step.l]\ntype = \"landing\"\ncategory = \"landing\"\n";
        let def = parse(text).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let m = missing_prompts(&def, dir.path());
        assert_eq!(m, vec!["step.s.prompt: no file .mason/prompts/sec.md"]);
        std::fs::write(dir.path().join("sec.md"), "x").unwrap();
        assert!(missing_prompts(&def, dir.path()).is_empty());
    }

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
            categories: vec!["testing".into()],
            config_schema: json!({"type": "object"}),
            about: String::new(),
        }
    }

    #[test]
    fn a_plugin_step_type_checks_offline_only_when_the_plugin_is_known() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("mason.toml");
        std::fs::write(&file, TEXT).unwrap();
        assert_eq!(check(&file, &[plugin()]), 0);
        // Without the project's plugins, or with one that is not this step's, it is refused.
        assert_eq!(check(&file, &[]), 1);
        let mut other = plugin();
        other.name = "something-else".into();
        assert_eq!(check(&file, &[other]), 1);
    }
}
