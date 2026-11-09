// SPDX-License-Identifier: GPL-3.0-or-later

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::proc::{self, ProcError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitError {
    pub args: Vec<String>,
    pub code: Option<i32>,
    pub stderr: String,
    /// The deadline passed and the process group was killed: not a git failure.
    pub timed_out: bool,
}

impl std::fmt::Display for GitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.timed_out {
            return write!(
                f,
                "git {} timed out: {}",
                self.args.join(" "),
                self.stderr.trim()
            );
        }
        write!(
            f,
            "git {} failed ({:?}): {}",
            self.args.join(" "),
            self.code,
            self.stderr.trim()
        )
    }
}

impl std::error::Error for GitError {}

/// How long an ordinary git call may take. Generous: the slowest healthy calls
/// (a fetch of a large repository) take well under a minute.
pub const DEFAULT_DEADLINE: Duration = Duration::from_secs(300);
/// A clone moves the whole history.
pub const CLONE_DEADLINE: Duration = Duration::from_secs(1800);

static DEADLINE_MS: AtomicU64 = AtomicU64::new(0);

/// Change the default deadline (tests shorten it; `MASON_GIT_TIMEOUT_SECS` does
/// the same for a whole process). Zero restores the built-in default.
pub fn set_default_deadline(d: Duration) {
    DEADLINE_MS.store(d.as_millis() as u64, Ordering::SeqCst);
}

pub fn default_deadline() -> Duration {
    let ms = DEADLINE_MS.load(Ordering::SeqCst);
    if ms > 0 {
        return Duration::from_millis(ms);
    }
    std::env::var("MASON_GIT_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|s| *s > 0.0)
        .map_or(DEFAULT_DEADLINE, Duration::from_secs_f64)
}

/// The git subcommand in `args`, skipping global options such as `-c k=v`.
fn subcommand<'a>(args: &[&'a str]) -> Option<&'a str> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if *a == "-c" || *a == "-C" {
            it.next();
        } else if !a.starts_with('-') {
            return Some(a);
        }
    }
    None
}

fn is_network(sub: Option<&str>) -> bool {
    matches!(sub, Some("fetch" | "push" | "clone" | "pull" | "ls-remote"))
}

static FETCH_LOCKS: std::sync::Mutex<
    Option<std::collections::HashMap<PathBuf, std::sync::Arc<std::sync::Mutex<()>>>>,
> = std::sync::Mutex::new(None);

/// Two fetches in one repository collide on its refs, wherever they run from (a
/// worktree shares the repository's refs): one at a time per repository. This lock
/// is not the daemon's repository lock, so a slow fetch holds up other fetches and
/// nothing else.
fn fetch_lock(cwd: &Path) -> std::sync::Arc<std::sync::Mutex<()>> {
    let common = proc::output(
        Command::new("git")
            .args(["rev-parse", "--git-common-dir"])
            .current_dir(cwd),
        default_deadline(),
    )
    .ok()
    .filter(|o| o.status.success())
    .map(|o| cwd.join(String::from_utf8_lossy(&o.stdout).trim()));
    let key = common.unwrap_or_else(|| cwd.to_path_buf());
    let key = std::fs::canonicalize(&key).unwrap_or(key);
    FETCH_LOCKS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(Default::default)
        .entry(key)
        .or_default()
        .clone()
}

/// Run `git` in `cwd` under the default deadline (longer for a clone).
/// Never prompts; commits and merges carry a fixed identity.
pub fn run(cwd: &Path, args: &[&str]) -> Result<String, GitError> {
    let d = if subcommand(args) == Some("clone") {
        default_deadline().max(CLONE_DEADLINE)
    } else {
        default_deadline()
    };
    run_for(cwd, args, d)
}

/// Like [`run`], with a deadline of the caller's choosing.
pub fn run_for(cwd: &Path, args: &[&str], limit: Duration) -> Result<String, GitError> {
    let err = |code, stderr: String, timed_out| GitError {
        args: args.iter().map(|s| s.to_string()).collect(),
        code,
        stderr,
        timed_out,
    };
    let lock = (subcommand(args) == Some("fetch")).then(|| fetch_lock(cwd));
    let _held = lock
        .as_ref()
        .map(|l| l.lock().unwrap_or_else(|e| e.into_inner()));
    let mut cmd = Command::new("git");
    cmd.args(["-c", "user.name=MASON", "-c", "user.email=mason@localhost"])
        .args(["-c", "commit.gpgsign=false", "-c", "core.editor=true"]);
    if is_network(subcommand(args)) {
        // A connection that stalls or crawls must end, and ssh must never ask.
        cmd.args(["-c", "http.lowSpeedLimit=1", "-c", "http.lowSpeedTime=60"]);
        if std::env::var_os("GIT_SSH_COMMAND").is_none() && std::env::var_os("GIT_SSH").is_none() {
            cmd.env(
                "GIT_SSH_COMMAND",
                "ssh -o BatchMode=yes -o ConnectTimeout=30 -o ServerAliveInterval=15 -o ServerAliveCountMax=4",
            );
        }
    }
    cmd.args(args)
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_EDITOR", "true");
    let out = match proc::output(&mut cmd, limit) {
        Ok(o) => o,
        Err(ProcError::Spawn(e)) => return Err(err(None, e.to_string(), false)),
        Err(ProcError::TimedOut { stderr, .. }) => {
            return Err(err(
                None,
                format!(
                    "no answer within {}s; the process was killed. {}",
                    limit.as_secs_f64().round(),
                    String::from_utf8_lossy(&stderr)
                ),
                true,
            ))
        }
    };
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(err(
            out.status.code(),
            // Some git versions print the reason for a refused push on stdout.
            format!(
                "{}{}",
                String::from_utf8_lossy(&out.stderr),
                String::from_utf8_lossy(&out.stdout)
            ),
            false,
        ))
    }
}

pub fn ok(cwd: &Path, args: &[&str]) -> bool {
    run(cwd, args).is_ok()
}

pub fn rev_parse(cwd: &Path, rev: &str) -> Result<String, GitError> {
    run(
        cwd,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{rev}^{{commit}}"),
        ],
    )
}

pub fn is_ancestor(cwd: &Path, ancestor: &str, of: &str) -> bool {
    ok(cwd, &["merge-base", "--is-ancestor", ancestor, of])
}

pub fn branch_exists(cwd: &Path, branch: &str) -> bool {
    ok(
        cwd,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )
}

pub fn commits_ahead(cwd: &Path, base: &str, head: &str) -> Result<u32, GitError> {
    Ok(
        run(cwd, &["rev-list", "--count", &format!("{base}..{head}")])?
            .parse()
            .unwrap_or(0),
    )
}

/// Every worktree path of the repository at `cwd`, with its checked-out branch
/// (if any).
pub fn worktrees(cwd: &Path) -> Result<Vec<(PathBuf, Option<String>)>, GitError> {
    let out = run(cwd, &["worktree", "list", "--porcelain"])?;
    let mut v = vec![];
    let (mut path, mut branch): (Option<PathBuf>, Option<String>) = (None, None);
    for l in out.lines().chain(std::iter::once("")) {
        if let Some(p) = l.strip_prefix("worktree ") {
            path = Some(PathBuf::from(p));
        } else if let Some(b) = l.strip_prefix("branch refs/heads/") {
            branch = Some(b.to_string());
        } else if l.is_empty() {
            if let Some(p) = path.take() {
                v.push((p, branch.take()));
            }
            branch = None;
        }
    }
    Ok(v)
}

/// Create (or find again, after a restart) a worktree on `branch`, starting
/// from `start` when the branch doesn't exist yet. Idempotent.
pub fn ensure_worktree(repo: &Path, wt: &Path, branch: &str, start: &str) -> Result<(), GitError> {
    if wt.join(".git").exists() {
        return Ok(());
    }
    let _ = run(repo, &["worktree", "prune"]);
    if let Some(parent) = wt.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let w = wt.display().to_string();
    if branch_exists(repo, branch) {
        run(repo, &["worktree", "add", &w, branch])?;
    } else {
        run(repo, &["worktree", "add", "-b", branch, &w, start])?;
    }
    Ok(())
}

pub fn remove_worktree(repo: &Path, wt: &Path) {
    let w = wt.display().to_string();
    let _ = run(repo, &["worktree", "remove", "--force", &w]);
    let _ = std::fs::remove_dir_all(wt);
    let _ = run(repo, &["worktree", "prune"]);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatchUp {
    /// The branch already contained the base.
    UpToDate,
    /// The base was folded in; the branch changed.
    Merged,
    /// Folding in the base conflicts. The merge was aborted; the tree is clean.
    Conflict(String),
}

/// Bring the branch checked out in `wt` up to date with `base_ref`.
pub fn catch_up(wt: &Path, base_ref: &str) -> Result<CatchUp, GitError> {
    if is_ancestor(wt, base_ref, "HEAD") {
        return Ok(CatchUp::UpToDate);
    }
    match run(wt, &["merge", "--no-edit", base_ref]) {
        Ok(_) => Ok(CatchUp::Merged),
        Err(e) => {
            let conflicts =
                run(wt, &["diff", "--name-only", "--diff-filter=U"]).unwrap_or_default();
            let _ = run(wt, &["merge", "--abort"]);
            if conflicts.trim().is_empty() {
                Err(e)
            } else {
                Ok(CatchUp::Conflict(conflicts))
            }
        }
    }
}

/// Refuse anything but plain, safe branch names.
pub fn valid_branch(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with(['-', '/', '.'])
        && !name.ends_with(['/', '.'])
        && !name.contains("..")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/-_.".contains(c))
}

#[cfg(test)]
mod deadline_tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn a_hung_git_is_killed_at_its_deadline() {
        let dir = tempfile::tempdir().unwrap();
        let t = Instant::now();
        let e = run_for(
            dir.path(),
            &["-c", "alias.hang=!sleep 30 & echo $! > pid; wait", "hang"],
            Duration::from_millis(500),
        )
        .unwrap_err();
        assert!(e.timed_out, "{e}");
        assert!(e.to_string().contains("timed out"));
        assert!(t.elapsed() < Duration::from_secs(3));
        // The sleep (a grandchild) died with the group.
        let pid = std::fs::read_to_string(dir.path().join("pid")).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        let alive = Command::new("kill")
            .args(["-0", pid.trim()])
            .status()
            .unwrap()
            .success();
        assert!(!alive, "the helper outlived the deadline");
    }

    #[test]
    fn a_git_failure_is_not_a_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let e = run(dir.path(), &["rev-parse", "HEAD"]).unwrap_err();
        assert!(!e.timed_out);
    }

    #[test]
    fn subcommand_skips_global_options() {
        assert_eq!(subcommand(&["-c", "a=b", "fetch", "-q"]), Some("fetch"));
        assert_eq!(
            subcommand(&["--no-optional-locks", "status"]),
            Some("status")
        );
    }
}
