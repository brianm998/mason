// SPDX-License-Identifier: GPL-3.0-or-later

//! `mason adopt`: register a project that already has a `.mason/` folder, as it is.
//! It writes the project overlay and nothing else: `.mason/` is never written, so adopting
//! cannot overwrite a pipeline someone built. A repository with no `.mason/` is sent to
//! `mason init`, which makes one.

use crate::init::{self, detect, questions::*};
use mason_landing::git;
use std::path::Path;

pub fn run(mut args: Vec<String>, config_dir: &Path) -> i32 {
    let take = |name: &str, args: &mut Vec<String>| -> Result<Option<String>, String> {
        match args.iter().position(|a| a == name) {
            None => Ok(None),
            Some(i) => {
                args.remove(i);
                if i < args.len() {
                    Ok(Some(args.remove(i)))
                } else {
                    Err(format!("{name} needs a value"))
                }
            }
        }
    };
    let (id, base, test, bin) = match (
        take("--id", &mut args),
        take("--base", &mut args),
        take("--test", &mut args),
        take("--agent-bin", &mut args),
    ) {
        (Ok(a), Ok(b), Ok(c), Ok(d)) => (a, b, c, d),
        (Err(e), ..) | (_, Err(e), ..) | (_, _, Err(e), _) | (.., Err(e)) => return fail(&e),
    };
    if let Some(extra) = args.first() {
        return fail(&format!("unexpected argument {extra:?}"));
    }
    let d = match detect::detect(&std::env::current_dir().unwrap_or_default()) {
        Ok(d) => d,
        Err(e) => return fail(&e),
    };
    let id = id.unwrap_or_else(|| d.id.clone());
    if !valid_project_id(&id) {
        return fail(&format!(
            "project id {id:?} must be lowercase letters, digits, - and _; pass --id"
        ));
    }
    let base = base.unwrap_or_else(|| d.branch.clone());
    let mason_dir = d.root.join(".mason");
    let on_head = git::ok(&d.root, &["cat-file", "-e", "HEAD:.mason/mason.toml"]);
    if !mason_dir.join("mason.toml").is_file() && !on_head {
        return fail("there is no .mason/mason.toml here, so there is nothing to adopt. `mason init` writes one");
    }
    let overlay = init::overlay_path(config_dir, &id);
    if overlay.exists() {
        return fail(&format!("{} already exists: project {id:?} is already registered. Nothing was changed (pass --id for another name)", overlay.display()));
    }
    // Read what is there, so a broken pipeline is said now, not at the first task.
    let text = std::fs::read_to_string(mason_dir.join("mason.toml")).or_else(|_| {
        git::run(&d.root, &["show", "HEAD:.mason/mason.toml"]).map_err(|e| e.to_string())
    });
    match text.as_deref().map(mason_definition::parse) {
        Ok(Ok(def)) => {
            let problems =
                mason_definition::validate(&def, &mason_definition::Capabilities::everything());
            if !problems.is_empty() {
                problems
                    .iter()
                    .for_each(|p| eprintln!("error  .mason/mason.toml: {p}"));
                return fail("that .mason/mason.toml does not parse and validate; fix it (`mason definition check`), then adopt");
            }
        }
        Ok(Err(ps)) => {
            ps.iter()
                .for_each(|p| eprintln!("error  .mason/mason.toml: {p}"));
            return fail("that .mason/mason.toml does not parse; fix it (`mason definition check`), then adopt");
        }
        Err(e) => return fail(&format!("cannot read .mason/mason.toml: {e}")),
    }
    let has_suites = text.as_deref().is_ok_and(|t| t.contains("suites ="));
    let landing = std::fs::read_to_string(mason_dir.join("landing.toml"))
        .ok()
        .and_then(|t| mason_landing::config::parse(&t).ok())
        .map_or(Landing::Direct, |c| match c.mode.name() {
            "direct-ci" | "local-only-ci" => Landing::DirectCi,
            "pr-each" | "host-queue" => Landing::Pr,
            "pr-batch" => Landing::Batched,
            _ => Landing::Direct,
        });
    if landing.needs_host() && d.github().is_none() {
        return fail(&format!("its landing mode is `{}`, which needs a GitHub remote, and this repository has none; nothing was written", landing.mode()));
    }
    let a = Answers {
        id: id.clone(),
        tracker: Tracker::Local,
        agent: Agent::Claude,
        agent_bin: bin,
        sessions: 1,
        workflow: Workflow::SoloFast,
        accept_suites: false,
        base: base.clone(),
        landing,
        alerts: None,
        ci_workflow: false,
        prove: false,
        notes: vec![],
    };
    let overlay_text = render_overlay(&a, &d, test.as_deref().unwrap_or("true"));
    if let Err(e) = mason_daemon::config::parse_project(&id, &overlay_text) {
        return fail(&e);
    }
    if let Err(e) = std::fs::create_dir_all(overlay.parent().unwrap_or(config_dir))
        .and_then(|()| std::fs::write(&overlay, overlay_text))
    {
        return fail(&format!("{}: {e}", overlay.display()));
    }
    println!(
        "Adopted {} as project {id:?} ({}).",
        d.root.display(),
        overlay.display()
    );
    println!(
        "  .mason/ was not touched. Base branch {base}, landing `{}`.",
        landing.mode()
    );
    if !has_suites && test.is_none() {
        println!("  warning: its test step names no suites and no --test was given, so the test command is `true`. Edit `test` in the overlay.");
    }
    println!("  Restart masond so it reads the new project, then `mason doctor`.");
    0
}

fn fail(msg: &str) -> i32 {
    eprintln!("error: {msg}");
    1
}
