// SPDX-License-Identifier: GPL-3.0-or-later

//! `mason init`: the guided setup of Part 21. It detects what it can, asks the eight
//! questions in order (each also a flag), writes `.mason/` and the "Filing work" notes on a
//! new branch, writes the project overlay, and offers to prove the setup with one harmless
//! task. It never pushes, never merges, and never touches the working tree: the branch is
//! made in a temporary worktree and the owner merges it.

pub mod detect;
pub mod questions;
pub mod suites;

use crate::client;
use mason_landing::git;
use questions::{
    Agent, Alert, Answers, Detected, Flags, Landing, Mode, Question, SuiteSpec, SETUP_BRANCH,
};
use serde_json::json;
use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};

pub const FILING_FILES: [&str; 2] = ["AGENTS.md", "CLAUDE.md"];

/// Where the Filing section goes: AGENTS.md if there is one, else CLAUDE.md, else a new AGENTS.md.
pub fn filing_file(exists: impl Fn(&str) -> bool) -> &'static str {
    FILING_FILES
        .into_iter()
        .find(|f| exists(f))
        .unwrap_or(FILING_FILES[0])
}

pub fn overlay_path(config_dir: &Path, id: &str) -> PathBuf {
    config_dir.join("projects").join(format!("{id}.toml"))
}

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

fn git_cmd(dir: &Path, args: &[&str]) -> Result<String, String> {
    git::run(dir, args).map_err(|e| format!("git {}: {e}", args.join(" ")))
}

/// Commit identity: the user's own if they have one, else a stand-in, so a fresh machine can
/// still make the setup commit.
fn identity(dir: &Path) -> Vec<String> {
    if git::run(dir, &["config", "user.email"]).is_ok_and(|e| !e.trim().is_empty()) {
        vec![]
    } else {
        ["-c", "user.name=MASON", "-c", "user.email=mason@localhost"]
            .map(String::from)
            .to_vec()
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
    match setup(&flags, &d, &a, config_dir) {
        Ok(next) => {
            for n in &a.notes {
                println!("note: {n}");
            }
            println!("{next}");
        }
        Err(e) => return fail(&e),
    }
    if !a.prove {
        return 0;
    }
    println!("\nProve it:");
    match prove(&d, &a.id, &a.base, data_dir) {
        Ok(()) => 0,
        Err(e) => {
            println!("  {e}");
            0
        }
    }
}

fn fail(msg: &str) -> i32 {
    eprintln!("error: {msg}");
    1
}

/// Questions 5 and part of 8: everything that is written. Returns the closing instructions.
pub fn setup(
    flags: &Flags,
    d: &Detected,
    a: &Answers,
    config_dir: &Path,
) -> Result<String, String> {
    // Refusals first, before anything is written.
    if git::rev_parse(&d.root, "HEAD").is_err() {
        return Err(
            "the repository has no commits yet; make one first (the setup is a branch from it)"
                .into(),
        );
    }
    let on_head = git::run(&d.root, &["ls-tree", "--name-only", "HEAD", ".mason"])
        .is_ok_and(|s| !s.trim().is_empty());
    if (on_head || d.existing_mason) && !flags.force {
        return Err("this repository already has a .mason/ folder. `mason adopt` registers it as it is; `mason init --force` replaces it".into());
    }
    let overlay = overlay_path(config_dir, &a.id);
    if overlay.exists() && !flags.force {
        return Err(format!("{} already exists (project {:?} is registered); `mason init --force` replaces it, or pass --id for another name", overlay.display(), a.id));
    }
    if git::ok(
        &d.root,
        &[
            "rev-parse",
            "--verify",
            "-q",
            &format!("refs/heads/{SETUP_BRANCH}"),
        ],
    ) && !flags.force
    {
        return Err(format!("the branch {SETUP_BRANCH} already exists from an earlier init; merge or delete it, or use --force"));
    }
    if !git::ok(
        &d.root,
        &[
            "rev-parse",
            "--verify",
            "-q",
            &format!("refs/heads/{}", a.base),
        ],
    ) && d.remote_name.as_ref().is_none_or(|r| {
        !git::ok(
            &d.root,
            &[
                "rev-parse",
                "--verify",
                "-q",
                &format!("refs/remotes/{r}/{}", a.base),
            ],
        )
    }) {
        return Err(format!(
            "the base branch {:?} does not exist here; pass --base with an existing branch",
            a.base
        ));
    }

    // 5. Tests: propose, run each once, keep what parses.
    let mut kept: Vec<SuiteSpec> = vec![];
    let mut baselines = vec![];
    if a.accept_suites {
        let proposals = suites::propose(&d.root, &d.languages);
        if proposals.is_empty() {
            println!(
                "\nTests: no suite could be proposed for {}.",
                if d.languages.is_empty() {
                    "this repository (no build files recognised)".to_string()
                } else {
                    d.languages.join(", ")
                }
            );
        }
        for s in proposals {
            println!(
                "\nTests: running `{}` once to check it parses...",
                s.command
            );
            let b = suites::run_once(&s, &d.root);
            if b.executed > 0 {
                println!(
                    "  {}: {} test(s) found, {} passed, {} failed, {:.1}s{}",
                    s.name,
                    b.executed,
                    b.passed,
                    b.failed,
                    b.seconds,
                    if b.green { "" } else { " (NOT green)" }
                );
                kept.push(s);
            } else {
                println!("  {}: dropped.", s.name);
            }
            if let Some(p) = &b.problem {
                println!("  warning: {}: {p}", b.name);
            }
            baselines.push(b);
        }
    }
    let test_cmd = if kept.is_empty() {
        println!("\nwarning: no test suite is set up, so the test step runs `true` and proves nothing. Add one in .mason/mason.toml (see `mason test parse`) before trusting a landing.");
        "true".to_string()
    } else {
        kept.iter()
            .map(|s| s.command.as_str())
            .collect::<Vec<_>>()
            .join(" && ")
    };

    let overlay_text = questions::render_overlay(a, d, &test_cmd);
    mason_daemon::config::parse_project(&a.id, &overlay_text)
        .map_err(|e| format!("the overlay would not load (a bug in init): {e}"))?;

    // The branch, in a temporary worktree, so the working tree is never touched.
    let tmp = std::env::temp_dir().join(format!("mason-setup-{}-{}", std::process::id(), a.id));
    let _ = std::fs::remove_dir_all(&tmp);
    let tmp_s = tmp.display().to_string();
    git_cmd(&d.root, &["worktree", "prune"])?;
    let branch_flag = if flags.force { "-B" } else { "-b" };
    git_cmd(
        &d.root,
        &[
            "worktree",
            "add",
            "-q",
            branch_flag,
            SETUP_BRANCH,
            &tmp_s,
            "HEAD",
        ],
    )?;
    let built = (|| -> Result<Vec<String>, String> {
        let mut said = vec![];
        let write = |rel: &str, text: &str| -> Result<(), String> {
            let p = tmp.join(rel);
            std::fs::create_dir_all(p.parent().unwrap_or(&tmp))
                .map_err(|e| format!("{rel}: {e}"))?;
            std::fs::write(&p, text).map_err(|e| format!("{rel}: {e}"))
        };
        if flags.force {
            let _ = std::fs::remove_dir_all(tmp.join(".mason"));
        }
        let mason_toml = questions::render_mason_toml(a.workflow, &kept);
        if let Err(p) = mason_definition::parse(&mason_toml) {
            return Err(format!(
                "the {} template with these suites does not parse (a bug in init): {p:?}",
                a.workflow.name()
            ));
        }
        write(".mason/mason.toml", &mason_toml)?;
        write(
            ".mason/landing.toml",
            &questions::render_landing_toml(a.workflow, a.landing, &a.base),
        )?;
        if !baselines.is_empty() {
            write(
                ".mason/baselines.toml",
                &suites::render_baselines(&baselines),
            )?;
        }
        // The Filing section.
        let target = filing_file(|f| tmp.join(f).is_file());
        let existing = std::fs::read_to_string(tmp.join(target)).unwrap_or_default();
        match questions::with_filing(&existing) {
            Some(text) => {
                write(target, &text)?;
                said.push(format!("added the \"Filing work\" notes to {target} (they explain the `explored` flag)"));
            }
            None => said.push(format!(
                "{target} already has the \"Filing work\" notes; left alone"
            )),
        }
        // CI trigger check, for the one mode that needs it.
        if a.landing == Landing::DirectCi {
            let problems = mason_adapters::github::workflow_problems_at(&d.root, "HEAD");
            if problems.is_empty() {
                said.push("CI: a workflow already runs on `mason/ci/**` or by dispatch".into());
            } else if a.ci_workflow {
                write(
                    ".github/workflows/mason-ci.yml",
                    &questions::render_ci_workflow(&test_cmd),
                )?;
                said.push("CI: added .github/workflows/mason-ci.yml, which runs the tests on `mason/ci/**` (on the setup branch only; nothing is pushed)".into());
            } else {
                said.push(format!("CI: direct-ci will not work yet: {}. Re-run with --ci-workflow yes to add one, or add the trigger yourself", problems.iter().map(|p| p.0.clone()).collect::<Vec<_>>().join("; ")));
            }
        }
        git_cmd(&tmp, &["add", "-A"])?;
        let mut commit = identity(&tmp);
        commit.extend(
            [
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "Set up MASON (mason init)",
                "-m",
                "Written by `mason init`. Review it, merge it, and tasks will use this workflow.",
            ]
            .map(String::from),
        );
        let refs: Vec<&str> = commit.iter().map(String::as_str).collect();
        git_cmd(&tmp, &refs)?;
        Ok(said)
    })();
    let _ = git::run(&d.root, &["worktree", "remove", "--force", &tmp_s]);
    let _ = std::fs::remove_dir_all(&tmp);
    let said = match built {
        Ok(s) => s,
        Err(e) => {
            let _ = git::run(&d.root, &["branch", "-D", SETUP_BRANCH]);
            return Err(e);
        }
    };

    // The overlay and the machine-wide files.
    std::fs::create_dir_all(overlay.parent().unwrap_or(config_dir))
        .map_err(|e| format!("{}: {e}", config_dir.display()))?;
    std::fs::write(&overlay, &overlay_text).map_err(|e| format!("{}: {e}", overlay.display()))?;
    let mut said = said;
    if let Some(channels) = &a.alerts {
        let p = config_dir.join("notify.toml");
        if p.exists() {
            said.push(format!(
                "{} exists; alert routing left as it is",
                p.display()
            ));
        } else {
            std::fs::write(&p, questions::render_notify(channels))
                .map_err(|e| format!("{}: {e}", p.display()))?;
            said.push(format!(
                "wrote {} (alerts: monitor{})",
                p.display(),
                channels
                    .iter()
                    .filter(|c| matches!(c, Alert::Sound | Alert::Speech))
                    .map(|c| format!(", {}", c.name()))
                    .collect::<String>()
            ));
        }
        for c in channels
            .iter()
            .filter(|c| matches!(c, Alert::Email | Alert::Push))
        {
            said.push(format!("{} needs an account and secrets: add an [{}] section to notify.toml (`mason notify` shows what is set up)", c.name(), if *c == Alert::Email { "email" } else { "ntfy" }));
        }
    }
    let res = config_dir.join("resources.toml");
    if a.agent == Agent::Claude {
        if res.exists() {
            said.push(format!("{} exists; sessions left as it is", res.display()));
        } else {
            std::fs::write(&res, format!("[llm]\nclaude = {}\n", a.sessions))
                .map_err(|e| format!("{}: {e}", res.display()))?;
            said.push(format!(
                "wrote {} ({} session(s) at once)",
                res.display(),
                a.sessions
            ));
        }
    } else {
        said.push("sessions for codex are set per account in providers.toml".into());
    }
    if a.agent_bin.is_none() {
        said.push(format!(
            "{} is logged in? MASON cannot check; if not, run `{}` once and log in",
            a.agent.name(),
            a.agent.name()
        ));
    }

    let mut out = String::from("\nWritten:\n");
    out.push_str(&format!("  branch    {SETUP_BRANCH} (one commit: .mason/ and the notes; your working tree is untouched)\n  project   {} ({})\n", a.id, overlay.display()));
    for s in said {
        out.push_str(&format!("  {s}\n"));
    }
    let push = if d.remote_name.is_some() {
        format!(
            "\n  git push {} {}",
            d.remote_name.as_deref().unwrap_or("origin"),
            a.base
        )
    } else {
        String::new()
    };
    out.push_str(&format!("\nNext: review and merge the branch, so tasks use it:\n  git diff {}...{SETUP_BRANCH}\n  git checkout {} && git merge --ff-only {SETUP_BRANCH}{push}\nthen start `masond`, and run `mason init --prove-only` to land a harmless first task.", a.base, a.base));
    Ok(out)
}

/// The text of the harmless task.
pub const PROOF_TITLE: &str = "Add a file that proves MASON works";
pub const PROOF_BODY: &str = "Create a file named MASON-WORKS.md at the top of the repository containing one line, `MASON landed this task.`, and nothing else. Change no other file.\n\n## Acceptance criteria\n- MASON-WORKS.md exists with exactly that one line.\n\n## Approach\nCreate MASON-WORKS.md (new) with the line above and commit it. This task exists only to show the pipeline works end to end; delete the file afterwards if you like.\n";

/// Question 8: file one small task, if a daemon that knows this project is running.
pub fn prove(d: &Detected, id: &str, base: &str, data_dir: &Path) -> Result<(), String> {
    let at = format!(
        "{}:.mason/mason.toml",
        d.remote_name
            .as_ref()
            .map_or(base.to_string(), |r| format!("{r}/{base}"))
    );
    let local = format!("{base}:.mason/mason.toml");
    if !git::ok(&d.root, &["cat-file", "-e", &at]) && !git::ok(&d.root, &["cat-file", "-e", &local])
    {
        return Err(format!("{base} has no .mason/ yet, so a task would run the built-in pipeline rather than your workflow. Merge {SETUP_BRANCH} into {base} (and push it, if there is a remote), then run `mason init --prove-only`"));
    }
    let daemon = client::find(data_dir).map_err(|_| "masond is not running. Start it (`masond`), then run `mason init --prove-only` to file the first task".to_string())?;
    let status = daemon
        .call("GET", "/status", None)
        .map_err(|e| e.to_string())?;
    let known = status["projects"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|p| p.as_str() == Some(id));
    if !known {
        return Err(format!("the running masond does not know project {id:?} yet: it reads projects when it starts. Restart it, then run `mason init --prove-only`"));
    }
    let r = daemon
        .call("POST", "/tasks", Some(&json!({"project": id, "title": PROOF_TITLE, "body": PROOF_BODY, "priority": "normal", "proposed": false, "explored": true, "after": []})))
        .map_err(|e| e.to_string())?;
    println!("  Filed task #{} in {id}: {PROOF_TITLE:?}.", r["task"]);
    println!("  Watch it with `mason task show {} --watch`, or in the monitor. When it lands, MASON-WORKS.md is on {base}.", r["task"]);
    println!("  (Opening the monitor for you is not done: it is a separate app.)");
    Ok(())
}
