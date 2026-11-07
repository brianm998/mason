// SPDX-License-Identifier: GPL-3.0-or-later

//! What a new build proves before it takes over (`masond selftest --db COPY`):
//! its migrations run on a copy of the real database and the copy is
//! consistent; the golden event logs recorded from earlier builds still replay
//! to the same states; and, in a sandbox of its own (a scratch repository, the
//! scripted stand-in agent, a throwaway data directory), a scripted task goes
//! from filed to landed. Nothing here touches the real database or any project.

use crate::config::DaemonConfig;
use crate::Daemon;
use mason_core::*;
use mason_engine::{reduce, World};
use mason_landing::git;
use mason_store::{decode_event, Store};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

const GOLDEN: [(&str, &str); 5] = [
    (
        "landed_task",
        include_str!("../tests/fixtures/landed_task.events"),
    ),
    (
        "red_then_green",
        include_str!("../tests/fixtures/red_then_green.events"),
    ),
    (
        "skipped_exploring",
        include_str!("../tests/fixtures/skipped_exploring.events"),
    ),
    (
        "stage6_landed_task",
        include_str!("../tests/fixtures/stage6_landed_task.events"),
    ),
    (
        "stage6_red_then_green",
        include_str!("../tests/fixtures/stage6_red_then_green.events"),
    ),
];

pub fn run(db_copy: &Path) -> Result<String, String> {
    let mut said = vec![];
    said.push(migrations(db_copy)?);
    said.push(golden()?);
    said.push(scripted_task()?);
    Ok(said.join("\n"))
}

/// Open the copy (which migrates it to this build's schema) and check it.
fn migrations(db: &Path) -> Result<String, String> {
    let store =
        Store::open(db).map_err(|e| format!("migrations on the copy of the database: {e}"))?;
    store
        .verify()
        .map_err(|e| format!("the migrated copy is inconsistent: {e}"))?;
    let n = store.last_seq().map_err(|e| e.to_string())?;
    Ok(format!(
        "migrations: the copy ({n} events) opens, migrates and verifies"
    ))
}

fn golden() -> Result<String, String> {
    let mut total = 0;
    for (name, text) in GOLDEN {
        let mut w = World::new();
        for line in text.lines() {
            let mut p = line.splitn(3, '\t');
            let at: u64 = p
                .next()
                .and_then(|x| x.parse().ok())
                .ok_or_else(|| format!("{name}: bad line"))?;
            let (kind, payload) = (p.next().unwrap_or(""), p.next().unwrap_or(""));
            let kind = decode_event(kind, payload).map_err(|e| format!("{name}: {line}: {e:?}"))?;
            w = reduce(
                &w,
                &Event {
                    at: Timestamp(at),
                    kind,
                },
            )
            .map_err(|e| format!("{name}: an event was refused on replay: {e:?}"))?
            .0;
            w.check_invariants().map_err(|e| format!("{name}: {e:?}"))?;
            total += 1;
        }
        let t = w
            .tasks
            .get(&TaskId(0))
            .ok_or_else(|| format!("{name}: no task"))?;
        if t.state != State::Landed {
            return Err(format!("{name}: replays to {:?}, not landed", t.state));
        }
    }
    Ok(format!(
        "golden replay: {} recorded runs, {total} events, all land",
        GOLDEN.len()
    ))
}

fn sh(cwd: &Path, args: &[&str]) -> Result<String, String> {
    git::run(cwd, args).map_err(|e| e.to_string())
}

fn scripted_task() -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let sib = |n: &str| exe.with_file_name(n);
    for need in ["step-host", "mason-mcp", "fake-claude"] {
        if !sib(need).is_file() {
            return Err(format!(
                "the build has no {need} next to masond, so the scripted task cannot run"
            ));
        }
    }
    let root: PathBuf = std::env::temp_dir().join(format!("mason-selftest-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let r = (|| -> Result<String, String> {
        let origin = root.join("o.git");
        let repo = root.join("repo");
        std::fs::create_dir_all(&origin).map_err(|e| e.to_string())?;
        sh(&origin, &["init", "-q", "--bare", "-b", "main"])?;
        sh(
            &root,
            &["clone", "-q", &origin.display().to_string(), "repo"],
        )?;
        sh(&repo, &["checkout", "-q", "-b", "main"])?;
        std::fs::write(repo.join("README"), "selftest\n").map_err(|e| e.to_string())?;
        sh(&repo, &["add", "-A"])?;
        sh(
            &repo,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-qm",
                "init",
            ],
        )?;
        sh(&repo, &["push", "-q", "origin", "main"])?;
        sh(&repo, &["checkout", "-q", "-b", "owner-work"])?;
        let script = root.join("script.json");
        std::fs::write(
            &script,
            serde_json::json!({"steps": {
                "explore": [[{"call": {"tool": "submit_plan", "args": {"summary": "add hello.txt", "files": ["hello.txt"], "approach": "write the file", "acceptance_criteria": ["hello.txt exists"]}}}]],
                "develop": [[
                    {"sh": "echo hello > hello.txt && git add -A && git -c user.name=t -c user.email=t@t commit -qm 'add hello'"},
                    {"call": {"tool": "submit_change", "args": {"summary": "added hello.txt", "head": "$HEAD"}}}
                ]]
            }})
            .to_string(),
        )
        .map_err(|e| e.to_string())?;
        let config = root.join("c");
        std::fs::create_dir_all(config.join("projects")).map_err(|e| e.to_string())?;
        std::fs::write(
            config.join("projects/selftest.toml"),
            format!(
                "path = \"{}\"\nbase = \"main\"\nremote = \"origin\"\ntest = \"test -f hello.txt\"\nclaude_bin = \"{}\"\nstep_timeout_s = 120\ntest_timeout_s = 60\n[agent_env]\nFAKE_CLAUDE_SCRIPT = \"{}\"\n",
                repo.display(),
                sib("fake-claude").display(),
                script.display()
            ),
        )
        .map_err(|e| e.to_string())?;
        let mut cfg = DaemonConfig::new(root.join("d"), config, sib("step-host"), sib("mason-mcp"));
        cfg.sweep_interval = Duration::from_secs(3600);
        cfg.heartbeat = Duration::from_millis(100);
        cfg.hold_retry = Duration::from_millis(300);
        let d: Arc<Daemon> =
            Daemon::start(cfg).map_err(|e| format!("the sandbox daemon did not start: {e}"))?;
        let pid = d
            .project_id_by_name("selftest")
            .ok_or("the sandbox project is missing")?;
        let appended = d
            .emit(EventKind::TaskFiled {
                project: pid,
                title: "self-test".into(),
                body: "Create hello.txt.".into(),
                priority: Priority::Normal,
                trust: TrustSource::Owner,
                explored: false,
                proposed: false,
                blockers: vec![],
                not_before: None,
            })
            .map_err(|e| e.to_string())?;
        let task = appended
            .effects
            .iter()
            .find_map(|e| match e {
                Effect::TaskCreated { task } => Some(*task),
                _ => None,
            })
            .ok_or("the task was not created")?;
        let end = Instant::now() + Duration::from_secs(150);
        loop {
            let w = d.world().map_err(|e| e.to_string())?;
            match w.tasks.get(&task).map(|t| t.state) {
                Some(State::Landed) => break,
                Some(s @ (State::Failed | State::Canceled)) => {
                    return Err(format!("the scripted task ended {s:?}"))
                }
                _ => {}
            }
            if Instant::now() > end {
                return Err("the scripted task did not land in 150 s".into());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let landed = sh(&origin, &["show", "main:hello.txt"])?;
        if landed.trim() != "hello" {
            return Err("the scripted task landed but its file is not on the remote".into());
        }
        d.store
            .verify()
            .map_err(|e| format!("the sandbox store is inconsistent: {e}"))?;
        // Hosts of the sandbox end with their steps; make sure none is left.
        for h in mason_step_host::registry::Registry::new(root.join("d/h"))
            .scan()
            .unwrap_or_default()
        {
            let _ = nix::sys::signal::killpg(
                nix::unistd::Pid::from_raw(h.info.child_pid as i32),
                nix::sys::signal::Signal::SIGKILL,
            );
            let _ = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(h.info.host_pid as i32),
                nix::sys::signal::Signal::SIGKILL,
            );
        }
        Ok("sandbox: a scripted task went from filed to landed".to_string())
    })();
    let _ = std::fs::remove_dir_all(&root);
    r
}
