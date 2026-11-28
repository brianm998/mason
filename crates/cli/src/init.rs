// SPDX-License-Identifier: GPL-3.0-or-later

//! `mason init`: the guided setup of Part 21. It detects what it can, asks the eight
//! questions in order (each also a flag), writes `.mason/` and the "Filing work" notes on a
//! new branch, writes the project overlay, and offers to prove the setup with one harmless
//! task. It writes the branch in a temporary worktree and never touches the working tree. It
//! lands the branch (merge, push, pull request) only when the owner answers `--land`; `--yes`
//! never does. Then it registers the project with a running masond, files the proof task and
//! points at the monitor.

pub use mason_onboard::{detect, land, overlay_path, questions, summary, FILING_FILES};

use crate::client;
use mason_landing::git;
use mason_onboard::questions::{Detected, Mode, Question, SETUP_BRANCH};
use serde_json::json;
use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};

fn ask_on_terminal(q: &Question) -> Option<String> {
    let mut out = std::io::stdout();
    println!("\n{}", q.text);
    for (i, o) in q.options.iter().enumerate() {
        println!(
            "  {}. {o}{}",
            i + 1,
            if *o == q.default {
                "  (recommended)"
            } else {
                ""
            }
        );
    }
    print!("[{}] > ", q.default);
    let _ = out.flush();
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line).ok()?;
    let line = line.trim().to_string();
    match line.parse::<usize>() {
        Ok(n) if n >= 1 && n <= q.options.len() => Some(q.options[n - 1].clone()),
        _ => Some(line),
    }
}

pub fn run(mut args: Vec<String>, data_dir: &Path, config_dir: &Path) -> i32 {
    let flags = match questions::parse_flags(&mut args) {
        Ok(f) => f,
        Err(e) => return fail(&e),
    };
    if let Some(extra) = args.first() {
        return fail(&format!(
            "unexpected argument {extra:?}; see `mason help init`"
        ));
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let d = match detect::detect(&cwd) {
        Ok(d) => d,
        Err(e) => return fail(&e),
    };
    if flags.prove_only {
        let id = flags.id.clone().unwrap_or_else(|| d.id.clone());
        let base = flags.base.clone().unwrap_or_else(|| d.branch.clone());
        return match prove(&d, &id, &base, data_dir) {
            Ok(()) => 0,
            Err(e) => fail(&e),
        };
    }
    print!("{}", detect::describe(&d));
    let interactive = std::io::stdin().is_terminal() && !flags.yes;
    if interactive {
        print!("\nIs that right? [Y/n] > ");
        let _ = std::io::stdout().flush();
        let mut l = String::new();
        let _ = std::io::stdin().lock().read_line(&mut l);
        if l.trim().to_lowercase().starts_with('n') {
            return fail("stopped; nothing was written. Fix the repository (or pass the flags) and run again");
        }
    }
    let mut ask = ask_on_terminal;
    let a = match questions::resolve(
        &flags,
        &d,
        &mut Mode {
            interactive,
            ask: &mut ask,
        },
        &mut |t| println!("\n{t}"),
    ) {
        Ok(a) => a,
        Err(e) => return fail(&e),
    };
    match mason_onboard::setup(
        &flags,
        &d,
        &a,
        config_dir,
        &mut |t| println!("{t}"),
        &validate_overlay,
    ) {
        Ok(next) => {
            for n in &a.notes {
                println!("note: {n}");
            }
            for w in &a.warnings {
                println!("warning: {w}");
            }
            // The "Next:" paragraph is for an owner who is landing by hand.
            if a.land == questions::Land::No {
                println!("{next}");
            } else {
                println!("{}", next.split("\nNext:").next().unwrap_or(&next));
            }
        }
        Err(e) => return fail(&e),
    }
    let landed = land::apply(&d, &a, &mut |t| println!("{t}"));
    let mut code = i32::from(landed.refused);
    if a.land == questions::Land::No {
        println!("\nNothing was merged or pushed. Land it with the commands above, then `mason init --prove-only`.");
    }
    let daemon = client::find(data_dir).ok();
    match &daemon {
        Some(_) => match register(data_dir, &a.id) {
            Ok(()) => println!("\nRegistered {} with the running masond (no restart needed).", a.id),
            Err(e) => {
                println!("\n  {e}");
                code = 1;
            }
        },
        None => println!("\nmasond is not running. Start it (`masond`, or `mason service install`); it reads {} when it starts.", overlay_path(config_dir, &a.id).display()),
    }
    if a.prove {
        println!("\nProve it:");
        if d.remote_name.is_some() && landed.merged && !landed.published {
            println!("  {} is merged here but not pushed, and masond lands from the remote: push it, then run `mason init --prove-only`.", a.base);
        } else if let Err(e) = prove(&d, &a.id, &a.base, data_dir) {
            println!("  {e}");
        }
    }
    if daemon.is_some() {
        open_monitor(data_dir);
    }
    print!("{}", summary(&a, config_dir));
    code
}

/// Tell the running masond about a project whose file `init` just wrote.
pub fn register(data_dir: &Path, id: &str) -> Result<(), String> {
    let daemon = client::find(data_dir).map_err(|e| e.to_string())?;
    let r = daemon
        .call("POST", &format!("/projects/{id}/register"), None)
        .map_err(|e| e.to_string())?;
    match r["result"].as_str() {
        Some("registered") | Some("known") => Ok(()),
        _ => Err(format!(
            "masond refused project {id:?}: {}",
            r["errors"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        )),
    }
}

/// The monitor app on a Mac you are sitting at; otherwise the pairing code for one.
fn open_monitor(data_dir: &Path) {
    let app = Path::new("/Applications/Mason Monitor.app");
    let watching = std::io::stdin().is_terminal() && std::env::var_os("MASON_NO_OPEN").is_none();
    if cfg!(target_os = "macos") && watching && app.exists() {
        let opened = std::process::Command::new("/usr/bin/open")
            .args(["-a", "Mason Monitor"])
            .status()
            .is_ok_and(|s| s.success());
        if opened {
            println!("\nOpened the monitor.");
            return;
        }
    }
    println!("\nMonitor: open the Mason Monitor app (or any paired device) and pair with:");
    match client::find(data_dir)
        .map_err(|e| e.to_string())
        .and_then(|d| {
            d.call("POST", "/pair/codes", Some(&json!({"role": "owner"})))
                .map_err(|e| e.to_string())
        }) {
        Ok(r) => println!(
            "  pairing code (owner, one use, {} s): {}  {}",
            r["expires_in_s"],
            r["code"].as_str().unwrap_or("?"),
            r["url"].as_str().unwrap_or("")
        ),
        Err(_) => println!("  mason pair"),
    }
}

/// The daemon's own parser: the overlay is checked as masond will read it.
pub fn validate_overlay(id: &str, text: &str) -> Result<(), String> {
    mason_daemon::config::parse_project(id, text).map(|_| ())
}

fn fail(msg: &str) -> i32 {
    eprintln!("error: {msg}");
    1
}

pub use mason_onboard::{PROOF_BODY, PROOF_TITLE};

/// Question 8: file one small task, if a daemon that knows this project is running.
pub fn prove(d: &Detected, id: &str, base: &str, data_dir: &Path) -> Result<(), String> {
    let at = format!(
        "{}:.mason/mason.toml",
        d.remote_name
            .as_ref()
            .map_or(base.to_string(), |r| format!("{r}/{base}"))
    );
    // With a remote only its copy counts: tasks branch from it, not from the local base.
    let local = format!("{base}:.mason/mason.toml");
    let found = if d.remote_name.is_some() {
        git::ok(&d.root, &["cat-file", "-e", &at])
    } else {
        git::ok(&d.root, &["cat-file", "-e", &local])
    };
    if !found {
        return Err(format!("{base} has no .mason/ yet, so a task would run the built-in pipeline rather than your workflow. Merge {SETUP_BRANCH} into {base} (and push it, if there is a remote), then run `mason init --prove-only`"));
    }
    let daemon = client::find(data_dir).map_err(|_| "masond is not running. Start it (`masond`), then run `mason init --prove-only` to file the first task".to_string())?;
    register(data_dir, id)?;
    let r = daemon
        .call("POST", "/tasks", Some(&json!({"project": id, "title": PROOF_TITLE, "body": PROOF_BODY, "priority": "normal", "proposed": false, "explored": true, "after": []})))
        .map_err(|e| e.to_string())?;
    println!("  Filed task #{} in {id}: {PROOF_TITLE:?}.", r["task"]);
    println!("  Watch it with `mason task show {} --watch`, or in the monitor. When it lands, MASON-WORKS.md is on {base}.", r["task"]);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mason_onboard::questions::*;

    fn answers(landing: Landing, tracker: Tracker) -> Answers {
        Answers {
            id: "r".into(),
            tracker,
            agent: Agent::Claude,
            agent_bin: Some("/x/claude".into()),
            sessions: 1,
            workflow: Workflow::SoloFast,
            accept_suites: true,
            base: "main".into(),
            landing,
            alerts: None,
            ci_workflow: false,
            land: Land::No,
            prove: true,
            token: Some("env:GITHUB_TOKEN".into()),
            placement: None,
            test: None,
            notes: vec![],
            warnings: vec![],
        }
    }

    #[test]
    fn the_overlay_parses_as_a_project_and_a_token_is_only_a_reference() {
        let d = Detected {
            root: "/r".into(),
            id: "r".into(),
            remote_name: Some("origin".into()),
            remote: parse_remote("git@github.com:o/r.git"),
            branch: "main".into(),
            ..Default::default()
        };
        let pr = render_overlay(&answers(Landing::Pr, Tracker::Github), &d, "true");
        let p = mason_daemon::config::parse_project("r", &pr).unwrap();
        assert!(p.services.host.is_some() && p.services.tracker.is_some());
        assert!(validate_overlay("r", &pr).is_ok());
    }

    #[test]
    fn notify_routing_parses() {
        let t = render_notify(&[Alert::Sound, Alert::Email]);
        assert!(mason_daemon::notify::parse(&t).is_ok());
    }
}
