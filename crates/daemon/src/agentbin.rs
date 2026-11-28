// SPDX-License-Identifier: GPL-3.0-or-later

//! An agent CLI that cannot be started is a fault in the setup, not in any task (s1287,
//! decision 1277).
//!
//! masond resolves each project's agent binary, in the environment its agents get (the daemon's
//! environment under the project's `agent_env`), at start, at register and reload, before a
//! launch and on every pass of the definition watcher. While it cannot be executed the project
//! is **held**: its queued tasks are not claimed, a step already claimed waits before its
//! launch, no diagnosis session starts, and one needs-you notice names the registration file
//! and the key to set.
//!
//! A launch that ends in the step host's `failed to start` puts the project on the same hold.
//! A file that looks executable can still fail that way, so that hold is not lifted by looking
//! again: it takes a reload, or a binary that has changed since the failure.

use crate::config::{ProjectCfg, Provider};
use crate::daemon::Daemon;
use mason_core::{Activity, ProjectId, State, Task, WaitReason};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

/// The binary a project's agent steps run: which key names it, and what it is run as.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wanted {
    /// `claude` or `codex`: the name looked up on `PATH` when the key is not set.
    pub cli: &'static str,
    /// `claude_bin` or `codex_bin`.
    pub key: &'static str,
    pub program: PathBuf,
}

pub fn wanted(p: &ProjectCfg) -> Wanted {
    let agent = p.agent();
    let (cli, key, set) = match agent.provider {
        Provider::Claude => ("claude", "claude_bin", p.claude_bin.clone()),
        Provider::Codex => ("codex", "codex_bin", agent.codex_bin),
    };
    Wanted {
        cli,
        key,
        program: set.unwrap_or_else(|| PathBuf::from(cli)),
    }
}

/// The `PATH` the project's agents get: its `agent_env`'s, else this process's.
fn agent_path(p: &ProjectCfg) -> Option<OsString> {
    p.agent_env
        .iter()
        .rev()
        .find(|(k, _)| k == "PATH")
        .map(|(_, v)| OsString::from(v))
        .or_else(|| std::env::var_os("PATH"))
}

/// Why `file` cannot be executed, if it cannot: it is missing, is not an executable file, or
/// is a script whose interpreter is neither.
fn not_runnable(file: &Path) -> Option<String> {
    let Ok(meta) = std::fs::metadata(file) else {
        return Some(format!("{} does not exist", file.display()));
    };
    if !meta.is_file() || !mason_platform::fs::is_executable(&meta, file) {
        return Some(format!("{} is not an executable file", file.display()));
    }
    if cfg!(unix) {
        let mut head = [0u8; 256];
        let n = std::fs::File::open(file)
            .and_then(|mut f| f.read(&mut head))
            .unwrap_or(0);
        if let Some(rest) = head[..n].strip_prefix(b"#!") {
            let line = rest.split(|b| *b == b'\n').next().unwrap_or_default();
            let line = String::from_utf8_lossy(line);
            if let Some(interp) = line.split_whitespace().next() {
                let ok = std::fs::metadata(interp)
                    .is_ok_and(|m| mason_platform::fs::is_executable(&m, Path::new(interp)));
                if interp.starts_with('/') && !ok {
                    return Some(format!(
                        "{} is a script whose interpreter {interp} cannot be executed",
                        file.display()
                    ));
                }
            }
        }
    }
    None
}

/// The file `program` starts, looked for as the launch would: a name with no directory on
/// `path`, any other path as it is (a relative one from `base`).
pub fn resolve(program: &Path, path: Option<&OsStr>, base: &Path) -> Result<PathBuf, String> {
    if program.components().count() > 1 || program.is_absolute() {
        let file = base.join(program);
        return match not_runnable(&file) {
            None => Ok(file),
            Some(why) => Err(why),
        };
    }
    let mut first_bad = None;
    for dir in path.map(std::env::split_paths).into_iter().flatten() {
        let mut file = dir.join(program);
        if cfg!(windows) && !file.is_file() {
            file.set_extension("exe");
        }
        if !file.is_file() {
            continue;
        }
        match not_runnable(&file) {
            None => return Ok(file),
            Some(why) => first_bad = first_bad.or(Some(why)),
        }
    }
    Err(first_bad
        .unwrap_or_else(|| format!("`{}` is not on the PATH the agents get", program.display())))
}

/// What the project's agent resolves to in this process's environment.
pub fn resolve_for(p: &ProjectCfg) -> Result<PathBuf, String> {
    resolve(&wanted(p).program, agent_path(p).as_deref(), &p.path)
}

/// The file a launch failed on, as it was then.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Seen {
    file: PathBuf,
    len: u64,
    modified: Option<SystemTime>,
}

fn seen(file: &Path) -> Option<Seen> {
    let m = std::fs::metadata(file).ok()?;
    Some(Seen {
        file: file.to_path_buf(),
        len: m.len(),
        modified: m.modified().ok(),
    })
}

#[derive(Clone, Debug)]
struct Hold {
    why: String,
    /// Set when a launch failed: only a change to this file, or a reload, lifts the hold.
    failed_on: Option<Option<Seen>>,
}

/// The held projects of one daemon.
#[derive(Default)]
pub struct Holds(Mutex<HashMap<ProjectId, Hold>>);

impl Holds {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<ProjectId, Hold>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Look at `p`'s agent binary now and update the hold. `forget` (a reload, a register)
    /// drops what an earlier launch said. `true` when the project is held afterwards.
    pub(crate) fn look(&self, pid: ProjectId, p: &ProjectCfg, forget: bool) -> bool {
        let found = resolve_for(p);
        let mut held = self.lock();
        if forget {
            held.remove(&pid);
        }
        match found {
            Err(why) => {
                if held.get(&pid).is_none_or(|h| h.failed_on.is_none()) {
                    held.insert(
                        pid,
                        Hold {
                            why,
                            failed_on: None,
                        },
                    );
                }
                true
            }
            Ok(file) => {
                let stays = held
                    .get(&pid)
                    .and_then(|h| h.failed_on.as_ref())
                    .is_some_and(|then| *then == seen(&file));
                if !stays {
                    held.remove(&pid);
                }
                stays
            }
        }
    }

    fn why(&self, pid: ProjectId) -> Option<String> {
        self.lock().get(&pid).map(|h| h.why.clone())
    }
}

/// Look at one project's agent binary now. `true` when it is held.
pub fn check(d: &Daemon, pid: ProjectId) -> bool {
    d.project(pid)
        .is_some_and(|p| d.agent_holds.look(pid, p, false))
}

/// Look at every project's agent binary, and let claims go where a hold has lifted.
pub fn check_all(d: &Daemon) {
    for (pid, p) in d.projects() {
        d.agent_holds.look(pid, p, false);
    }
}

/// Why the project is held, as the last look found it. Does not touch the disk.
pub fn why(d: &Daemon, pid: ProjectId) -> Option<String> {
    d.agent_holds.why(pid)
}

/// Does the project's current definition have an agent step? A project of commands only
/// needs no agent, so nothing of it is held.
fn uses_agent(d: &Daemon, pid: ProjectId) -> bool {
    d.current_def(pid)
        .steps
        .iter()
        .any(|s| s.kind == mason_definition::StepType::Agent)
}

/// Is the project's queue held back for its agent binary?
pub fn holds_claims(d: &Daemon, pid: ProjectId) -> bool {
    why(d, pid).is_some() && uses_agent(d, pid)
}

/// The start of the step host's message when the CLI could not be started, as a crash's
/// detail carries it (exit status 127, no result).
const FAILED_TO_START: &str =
    "the CLI exited with status 127 and no result; stderr: step-host: failed to start ";

/// What the step host said, when a session's crash says its CLI could not be started.
pub fn failed_to_start(detail: &str) -> Option<&str> {
    let rest = detail.strip_prefix(FAILED_TO_START)?;
    Some(rest.lines().next().unwrap_or(rest).trim())
}

/// [`failed_to_start`] of the way a step ended.
pub fn could_not_start(end: &mason_adapters::runner::StepEnd) -> Option<&str> {
    use mason_adapters::runner::{FailReason, StepEnd};
    match end {
        StepEnd::Failed(FailReason::Crash { detail }) => failed_to_start(detail),
        _ => None,
    }
}

/// A launch for `pid` ended in `failed to start`: hold the project.
pub fn launch_failed(d: &Daemon, pid: ProjectId, said: &str) {
    let Some(p) = d.project(pid) else { return };
    let on = resolve_for(p).ok().and_then(|f| seen(&f));
    tracing::warn!(project = %p.id, "the agent CLI failed to start, the project is held: {said}");
    d.agent_holds.lock().insert(
        pid,
        Hold {
            why: format!("a launch failed to start {said}"),
            failed_on: Some(on),
        },
    );
    d.poke();
}

const WAIT: &str = "Held: the agent CLI cannot be started";

/// What a held project's tasks say they wait for.
pub fn wait_sentence(why: &str) -> String {
    format!("{WAIT} ({why})")
}

/// Is `t` going nowhere only because of its project's agent CLI: queued in a held project, or
/// a step waiting to launch?
pub fn is_waiting(d: &Daemon, t: &Task) -> bool {
    match &t.activity {
        Activity::Waiting(WaitReason::Slot) => {
            t.state == State::Queued && holds_claims(d, t.project)
        }
        Activity::Waiting(WaitReason::On(on)) => {
            on.starts_with(WAIT) && why(d, t.project).is_some()
        }
        _ => false,
    }
}

/// What this daemon's environment resolves every project's agent to, as `mason doctor` asks.
pub fn report(d: &Daemon) -> Value {
    let list: Vec<Value> = d
        .projects()
        .into_iter()
        .map(|(pid, p)| {
            let w = wanted(p);
            let found = resolve_for(p);
            json!({
                "project": p.id,
                "cli": w.cli,
                "key": w.key,
                "binary": found.as_ref().ok().map(|f| f.display().to_string()),
                "error": found.err(),
                "held": why(d, pid),
                "needs_agent": uses_agent(d, pid),
            })
        })
        .collect();
    json!({"projects": list})
}

/// `agent-bin:<project>`: one notice for each held project.
pub fn alerts(d: &Daemon) -> Vec<(String, String, String)> {
    d.projects()
        .into_iter()
        .filter_map(|(pid, p)| {
            let why = why(d, pid).filter(|_| uses_agent(d, pid))?;
            let w = wanted(p);
            let file = d
                .cfg
                .config_dir
                .join("projects")
                .join(format!("{}.toml", p.id));
            Some((
                format!("agent-bin:{}", p.id),
                "needs_you".to_string(),
                format!(
                    "{id}: the agent CLI cannot be started: {why}. Its tasks are held. Set `{key}` \
                     in {file} to a `{cli}` that masond can run, then \
                     `mason project reload --project {id}`",
                    id = p.id,
                    key = w.key,
                    cli = w.cli,
                    file = file.display(),
                ),
            ))
        })
        .collect()
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn file(dir: &Path, name: &str, text: &str, mode: u32) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, text).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode)).unwrap();
        p
    }

    #[test]
    fn a_name_is_looked_for_on_the_path_it_is_given_and_a_path_as_it_is() {
        let t = tempfile::tempdir().unwrap();
        let bin = file(t.path(), "claude", "#!/bin/sh\n", 0o755);
        let path = t.path().as_os_str();
        let at = |program: &str, path: Option<&OsStr>| resolve(Path::new(program), path, t.path());
        assert_eq!(at("claude", Some(path)), Ok(bin.clone()));
        assert_eq!(at(bin.to_str().unwrap(), None), Ok(bin.clone()));
        assert_eq!(at("./claude", None), Ok(t.path().join("./claude")));
        let e = at("claude", Some(OsStr::new("/nonexistent"))).unwrap_err();
        assert!(e.contains("not on the PATH"), "{e}");
        assert!(at("/nonexistent/claude", Some(path))
            .unwrap_err()
            .contains("does not exist"));
    }

    #[test]
    fn a_file_without_the_bit_or_without_its_interpreter_cannot_be_run() {
        let t = tempfile::tempdir().unwrap();
        let plain = file(t.path(), "plain", "x", 0o644);
        let broken = file(t.path(), "broken", "#!/nonexistent/node\n", 0o755);
        let e = resolve(&plain, None, t.path()).unwrap_err();
        assert!(e.contains("not an executable file"), "{e}");
        let e = resolve(&broken, None, t.path()).unwrap_err();
        assert!(e.contains("/nonexistent/node"), "{e}");
        assert!(resolve(t.path(), None, t.path()).is_err(), "a directory");
    }

    #[test]
    fn only_the_step_hosts_failure_to_start_counts_as_one() {
        let d = format!("{FAILED_TO_START}\"/x/claude\": No such file or directory (os error 2)");
        assert!(failed_to_start(&d).unwrap().starts_with("\"/x/claude\""));
        for other in [
            "the CLI exited with status 127 and no result; stderr: sh: claude: not found",
            "the CLI exited with status 1 and no result; stderr: step-host: failed to start x",
            "step-host: failed to start",
        ] {
            assert_eq!(failed_to_start(other), None, "{other}");
        }
    }
}
