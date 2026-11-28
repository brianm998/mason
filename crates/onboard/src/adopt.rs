// SPDX-License-Identifier: GPL-3.0-or-later

//! `mason adopt`: register a project that already has a `.mason/` folder, as it is.
//! It writes the project overlay and nothing else: `.mason/` is never written, so adopting
//! cannot overwrite a pipeline someone built. A repository with no `.mason/` is sent to
//! `mason init`, which makes one.

use crate::questions::*;
use crate::{overlay_path, Validator};
use mason_landing::git;
use std::path::{Path, PathBuf};

/// What `adopt` is told (the CLI's `--id`, `--base`, `--test`, `--agent-bin`).
#[derive(Clone, Debug, Default)]
pub struct Args {
    pub id: Option<String>,
    pub base: Option<String>,
    pub test: Option<String>,
    pub agent_bin: Option<String>,
    /// `[placement] mode` to write (only when nodes are enabled).
    pub placement: Option<String>,
}

impl Args {
    pub fn from_flags(f: &Flags) -> Args {
        Args {
            id: f.id.clone(),
            base: f.base.clone(),
            test: f.test.clone(),
            agent_bin: f.agent_bin.clone(),
            placement: f
                .machine
                .nodes_enabled
                .then(|| f.placement.clone())
                .flatten(),
        }
    }
}

/// A refusal: lines for stderr first (each already prefixed), then the message.
#[derive(Clone, Debug)]
pub struct AdoptError {
    pub lines: Vec<String>,
    pub message: String,
}

fn err(message: impl Into<String>) -> AdoptError {
    AdoptError {
        lines: vec![],
        message: message.into(),
    }
}

/// A successful adopt.
#[derive(Clone, Debug)]
pub struct Adopted {
    pub id: String,
    pub base: String,
    pub landing: Landing,
    pub overlay: PathBuf,
    /// The test step names no suites and no test command was given.
    pub test_is_true: bool,
}

struct Prepared {
    id: String,
    base: String,
    landing: Landing,
    overlay_text: String,
    test_is_true: bool,
}

fn prepare(d: &Detected, args: &Args, config_dir: Option<&Path>) -> Result<Prepared, AdoptError> {
    let id = args.id.clone().unwrap_or_else(|| d.id.clone());
    if !valid_project_id(&id) {
        return Err(err(format!(
            "project id {id:?} must be lowercase letters, digits, - and _; pass --id"
        )));
    }
    let base = args.base.clone().unwrap_or_else(|| d.branch.clone());
    let mason_dir = d.root.join(".mason");
    let on_head = git::ok(&d.root, &["cat-file", "-e", "HEAD:.mason/mason.toml"]);
    if !mason_dir.join("mason.toml").is_file() && !on_head {
        return Err(err("there is no .mason/mason.toml here, so there is nothing to adopt. `mason init` writes one"));
    }
    if let Some(config_dir) = config_dir {
        let overlay = overlay_path(config_dir, &id);
        if overlay.exists() {
            return Err(err(format!("{} already exists: project {id:?} is already registered. Nothing was changed (pass --id for another name)", overlay.display())));
        }
    }
    // Read what is there, so a broken pipeline is said now, not at the first task.
    let text = std::fs::read_to_string(mason_dir.join("mason.toml")).or_else(|_| {
        git::run(&d.root, &["show", "HEAD:.mason/mason.toml"]).map_err(|e| e.to_string())
    });
    let lines = |ps: &[String]| -> Vec<String> {
        ps.iter()
            .map(|p| format!("error  .mason/mason.toml: {p}"))
            .collect()
    };
    match text.as_deref().map(mason_definition::parse) {
        Ok(Ok(def)) => {
            let problems =
                mason_definition::validate(&def, &mason_definition::Capabilities::everything());
            if !problems.is_empty() {
                let ps: Vec<String> = problems.iter().map(|p| p.to_string()).collect();
                return Err(AdoptError {
                    lines: lines(&ps),
                    message: "that .mason/mason.toml does not parse and validate; fix it (`mason definition check`), then adopt".into(),
                });
            }
        }
        Ok(Err(ps)) => {
            let ps: Vec<String> = ps.iter().map(|p| p.to_string()).collect();
            return Err(AdoptError {
                lines: lines(&ps),
                message: "that .mason/mason.toml does not parse; fix it (`mason definition check`), then adopt".into(),
            });
        }
        Err(e) => return Err(err(format!("cannot read .mason/mason.toml: {e}"))),
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
        return Err(err(format!("its landing mode is `{}`, which needs a GitHub remote, and this repository has none; nothing was written", landing.mode())));
    }
    let a = Answers {
        id: id.clone(),
        tracker: Tracker::Local,
        agent: Agent::Claude,
        agent_bin: args.agent_bin.clone(),
        sessions: 1,
        workflow: Workflow::SoloFast,
        accept_suites: false,
        base: base.clone(),
        landing,
        alerts: None,
        ci_workflow: false,
        land: Land::No,
        prove: false,
        token: Some("env:GITHUB_TOKEN".into()),
        placement: args.placement.clone(),
        test: args.test.clone(),
        notes: vec![],
        warnings: vec![],
    };
    let overlay_text = render_overlay(&a, d, args.test.as_deref().unwrap_or("true"));
    Ok(Prepared {
        id,
        base,
        landing,
        overlay_text,
        test_is_true: !has_suites && args.test.is_none(),
    })
}

/// The refusals of [`adopt`], without writing anything.
pub fn check(d: &Detected, args: &Args) -> Result<(), AdoptError> {
    prepare(d, args, None).map(|_| ())
}

/// Write the overlay for the repository's own `.mason/`, which is not touched.
pub fn adopt(
    d: &Detected,
    args: &Args,
    config_dir: &Path,
    validate: Validator,
) -> Result<Adopted, AdoptError> {
    let p = prepare(d, args, Some(config_dir))?;
    validate(&p.id, &p.overlay_text).map_err(err)?;
    let overlay = overlay_path(config_dir, &p.id);
    std::fs::create_dir_all(overlay.parent().unwrap_or(config_dir))
        .and_then(|()| std::fs::write(&overlay, &p.overlay_text))
        .map_err(|e| err(format!("{}: {e}", overlay.display())))?;
    Ok(Adopted {
        id: p.id,
        base: p.base,
        landing: p.landing,
        overlay,
        test_is_true: p.test_is_true,
    })
}
