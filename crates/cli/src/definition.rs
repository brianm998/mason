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

/// `git ls-files` in `root`; `None` when it is not a git checkout.
fn tracked_files(root: &Path) -> Option<Vec<String>> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files"])
        .output()
        .ok()?;
    out.status.success().then(|| {
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(String::from)
            .collect()
    })
}

/// Lines naming the tracked files no `[impact]` glob matches, by top-level directory.
/// `.mason/mason.toml` stays unmapped on purpose (a definition change tests everything), and
/// so do the three files a remote run's snapshot commit adds to the tree it sends
/// (`.mason/run.json`, `changed-files.txt`, `base-sha`; written by `daemon/src/remote.rs`).
fn unmapped_report(impact: &mason_testing::model::ImpactMap, files: &[String]) -> Vec<String> {
    if impact.entries.is_empty() {
        return vec![];
    }
    impact
        .unmapped_by_dir(files)
        .into_iter()
        .map(|(dir, fs)| {
            let fs: Vec<String> = fs
                .into_iter()
                .filter(|f| {
                    ![
                        ".mason/mason.toml",
                        ".mason/run.json",
                        ".mason/changed-files.txt",
                        ".mason/base-sha",
                    ]
                    .contains(&f.as_str())
                })
                .collect();
            (dir, fs)
        })
        .filter(|(_, fs)| !fs.is_empty())
        .map(|(dir, fs)| {
            format!(
                "unmapped  {dir}: {} file(s) run every suite whole: {}",
                fs.len(),
                fs.join(", ")
            )
        })
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
    // Tracked files at the checkout no impact glob names: each one runs every suite whole.
    let root = path
        .parent()
        .and_then(Path::parent)
        .unwrap_or(Path::new("."));
    if let Some(files) = tracked_files(root) {
        for line in unmapped_report(&def.testing.impact, &files) {
            println!("{line}");
        }
    }
    for w in validate::warnings(&def, &Capabilities::now().with_plugins(plugins.to_vec())) {
        println!("warning  {w}");
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

/// `narrow = true` in the `.mason/landing.toml` beside `path` needs a full-test schedule with a
/// trigger (s804). `full_test` says whether the project has one; `None` when no `--project` was
/// given, which can only be said in a note. 1 for a refused or unreadable file.
pub fn check_narrow(path: &Path, full_test: Option<bool>) -> i32 {
    let landing = path.parent().unwrap_or(Path::new(".")).join("landing.toml");
    let Ok(text) = std::fs::read_to_string(&landing) else {
        return 0;
    };
    let cfg = match mason_landing::config::parse(&text) {
        Ok(c) => c,
        Err(ps) => {
            ps.iter()
                .for_each(|p| println!("error  {}: {p}", landing.display()));
            return 1;
        }
    };
    if !cfg.narrows() {
        return 0;
    }
    match full_test {
        None => {
            println!(
                "note  {}: the batch tier narrows; pass --project P to check that P has the full-test schedule it needs",
                landing.display()
            );
            0
        }
        Some(has) => {
            let problems = mason_landing::config::narrow_problems(&cfg, has);
            problems.iter().for_each(|p| println!("error  {p}"));
            i32::from(!problems.is_empty())
        }
    }
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
    fn real_definition() -> mason_definition::Definition {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.mason/mason.toml");
        parse(&std::fs::read_to_string(p).unwrap()).unwrap()
    }

    #[test]
    fn the_paths_that_ran_everything_are_now_mapped() {
        let def = real_definition();
        let all: Vec<String> = [
            "cargo-workspace",
            "swift-clients",
            "compose-clients",
            "exit-scripts",
        ]
        .map(String::from)
        .to_vec();
        for f in [
            ".github/workflows/release.yml",
            "scripts/package/deb.sh",
            "scripts/bundle-monitor.sh",
            "scripts/tests/pick.sh",
            "scripts/install.sh",
            ".mason/smoke-tests.txt",
            ".mason/landing.toml",
            "packaging/homebrew/mason.rb",
            ".gitignore",
            "clients/mascot.toml",
            "scripts/rustdoc-bootstrap.sh",
            "scripts/run-exit-scripts.sh",
            ".cargo/config.toml",
            "scripts/spike/sccache-workspace.sh",
            "scripts/test-shard.py",
        ] {
            let sel = def.testing.impact.select(&[f.to_string()], &all);
            assert!(
                matches!(sel, mason_testing::model::Selection::Some(_)),
                "{f}"
            );
        }
        let sel = def
            .testing
            .impact
            .select(&[".mason/mason.toml".to_string()], &all);
        assert_eq!(sel, mason_testing::model::Selection::All);
    }

    #[test]
    fn no_tracked_file_but_the_definition_is_unmapped() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let Some(files) = tracked_files(&root) else {
            return;
        };
        // A remote runner's checkout carries `.mason/run.json`, which MASON writes into the
        // snapshot it pushes; it is never a file of the repository.
        let files: Vec<String> = files
            .into_iter()
            .filter(|f| f != ".mason/run.json")
            .collect();
        let r = unmapped_report(&real_definition().testing.impact, &files);
        assert!(r.is_empty(), "{r:?}");
    }

    #[test]
    fn unmapped_files_are_listed_by_top_level_directory() {
        let impact = mason_testing::model::ImpactMap {
            entries: vec![("src/**".into(), vec!["unit".into()])],
            overrides: vec![],
        };
        let files: Vec<String> = [
            "src/a.rs",
            "tools/x.sh",
            "tools/y.sh",
            "LICENSE",
            ".mason/mason.toml",
        ]
        .map(String::from)
        .to_vec();
        let r = unmapped_report(&impact, &files);
        assert_eq!(r.len(), 2, "{r:?}");
        assert!(r
            .iter()
            .any(|l| l.contains("tools: 2") && l.contains("tools/x.sh")));
        assert!(r
            .iter()
            .any(|l| l.contains(".: 1") && l.contains("LICENSE")));
        assert!(!r.iter().any(|l| l.contains("src/a.rs")));
    }
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
