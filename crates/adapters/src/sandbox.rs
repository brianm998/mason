// SPDX-License-Identifier: GPL-3.0-or-later

//! Optional OS sandboxing for an agent CLI (Part 6): macOS `sandbox-exec` and
//! Linux `bubblewrap`. Both let the agent read the machine as before but write
//! only inside its worktree and MASON's state directory (and scratch space), so a
//! runaway agent cannot change the owner's other files. The network stays on:
//! the agent has to reach its provider. Off by default; turned on per project.
//!
//! The wrappers are pure: they turn a command into the command that runs it
//! under the sandbox. Whether the binary exists is the caller's question
//! ([`available`]), and a missing binary is an error, never a silent fallback to
//! running unsandboxed.

use crate::claude::AgentCommand;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SandboxKind {
    #[default]
    None,
    SandboxExec,
    Bubblewrap,
}

impl SandboxKind {
    pub fn parse(s: &str) -> Result<SandboxKind, String> {
        match s {
            "none" => Ok(SandboxKind::None),
            "sandbox-exec" => Ok(SandboxKind::SandboxExec),
            "bubblewrap" => Ok(SandboxKind::Bubblewrap),
            o => Err(format!(
                "`sandbox` is `none`, `sandbox-exec` or `bubblewrap`, not {o:?}"
            )),
        }
    }

    fn binary(self) -> Option<&'static str> {
        match self {
            SandboxKind::None => None,
            SandboxKind::SandboxExec => Some("sandbox-exec"),
            SandboxKind::Bubblewrap => Some("bwrap"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SandboxPolicy {
    pub kind: SandboxKind,
    pub worktree: PathBuf,
    pub state_dir: PathBuf,
    /// More places the CLI must write (its own session and config directory in
    /// the home directory). Only paths that exist are bound.
    pub extra: Vec<PathBuf>,
}

/// Is the sandbox's binary on `PATH`?
pub fn available(kind: SandboxKind) -> bool {
    let Some(b) = kind.binary() else { return true };
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(b).is_file()))
}

fn canon(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

impl SandboxPolicy {
    /// Paths are made canonical where they exist: a profile matches real paths
    /// (on macOS `/tmp` is `/private/tmp`).
    pub fn new(kind: SandboxKind, worktree: &Path, state_dir: &Path) -> Self {
        SandboxPolicy {
            kind,
            worktree: canon(worktree),
            state_dir: canon(state_dir),
            extra: vec![],
        }
    }
}

fn sbpl(p: &Path) -> Result<String, String> {
    let s = p.to_str().ok_or("a sandbox path is not UTF-8")?;
    if s.chars().any(char::is_control) {
        return Err(format!("a sandbox path has a control character: {s:?}"));
    }
    Ok(format!(
        "(subpath \"{}\")",
        s.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}

/// The `sandbox-exec` profile: everything allowed except writing, and writing
/// allowed only under the worktree, the state directory and scratch space.
pub fn sandbox_exec_profile(p: &SandboxPolicy) -> Result<String, String> {
    let mut s =
        String::from("(version 1)\n(allow default)\n(deny file-write*)\n(allow file-write*\n");
    for path in [&p.worktree, &p.state_dir].into_iter().chain(&p.extra) {
        s.push_str(&format!("  {}\n", sbpl(path)?));
    }
    for scratch in ["/private/tmp", "/private/var/folders", "/dev"] {
        s.push_str(&format!("  {}\n", sbpl(Path::new(scratch))?));
    }
    s.push_str(")\n");
    Ok(s)
}

/// The `bwrap` arguments that precede the program: the machine read-only, the
/// worktree and state directory writable, a private `/tmp`, `/proc` and `/dev`,
/// the network shared, and the sandbox dying with its parent.
pub fn bwrap_args(p: &SandboxPolicy) -> Vec<String> {
    let wt = p.worktree.display().to_string();
    let st = p.state_dir.display().to_string();
    let mut a: Vec<String> = [
        "--ro-bind",
        "/",
        "/",
        "--dev",
        "/dev",
        "--proc",
        "/proc",
        "--tmpfs",
        "/tmp",
    ]
    .map(String::from)
    .to_vec();
    let extra: Vec<String> = p
        .extra
        .iter()
        .filter(|e| e.exists())
        .map(|e| e.display().to_string())
        .collect();
    for d in [&wt, &st].into_iter().chain(&extra) {
        a.extend(["--bind".into(), d.clone(), d.clone()]);
    }
    a.extend(["--unshare-pid".into(), "--die-with-parent".into()]);
    a.extend(["--chdir".into(), wt]);
    a.push("--".into());
    a
}

/// `cmd` run under the sandbox. `SandboxKind::None` returns it unchanged.
pub fn wrap(cmd: AgentCommand, p: &SandboxPolicy) -> Result<AgentCommand, String> {
    let program = cmd.program.display().to_string();
    let (wrapper, mut args) = match p.kind {
        SandboxKind::None => return Ok(cmd),
        SandboxKind::SandboxExec => (
            "sandbox-exec",
            vec!["-p".to_string(), sandbox_exec_profile(p)?],
        ),
        SandboxKind::Bubblewrap => ("bwrap", bwrap_args(p)),
    };
    args.push(program);
    args.extend(cmd.args.iter().cloned());
    Ok(AgentCommand {
        program: PathBuf::from(wrapper),
        args,
        ..cmd
    })
}

/// [`wrap`] when a policy is set, refusing (not skipping) when its binary is missing.
pub fn apply(
    cmd: AgentCommand,
    policy: Option<&SandboxPolicy>,
) -> Result<AgentCommand, mason_adapter_api::AdapterError> {
    let Some(p) = policy else { return Ok(cmd) };
    if !available(p.kind) {
        return Err(mason_adapter_api::AdapterError::Other(format!(
            "the sandbox {:?} is configured but its binary is not installed",
            p.kind
        )));
    }
    wrap(cmd, p).map_err(mason_adapter_api::AdapterError::Other)
}
