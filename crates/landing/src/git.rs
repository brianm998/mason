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

/// What MASON appends to every ssh command: never ask, never hang. ssh takes the first value of
/// an option, so a key or option already in the base command wins.
pub const SSH_OPTIONS: &str =
    "-o BatchMode=yes -o ConnectTimeout=30 -o ServerAliveInterval=15 -o ServerAliveCountMax=4";

/// The repository's `core.sshCommand`, if it has one.
pub fn configured_ssh(cwd: &Path) -> Option<String> {
    let o = proc::output(
        command()
            .args(["config", "--get", "core.sshCommand"])
            .current_dir(cwd),
        default_deadline(),
    )
    .ok()
    .filter(|o| o.status.success())?;
    let v = String::from_utf8_lossy(&o.stdout).trim().to_string();
    (!v.is_empty()).then_some(v)
}

/// Why GitHub (or the host) refused to let this client in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthFailure {
    /// The account the host saw, when it said so ("denied to brian-preffect").
    pub seen_as: Option<String>,
}

/// Is `stderr` a refusal of credentials, as opposed to a network failure or a rejected push?
pub fn auth_failure(stderr: &str) -> Option<AuthFailure> {
    if let Some(i) = stderr.find(" denied to ") {
        let rest = &stderr[i + " denied to ".len()..];
        let user: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
            .collect();
        return Some(AuthFailure {
            seen_as: (!user.is_empty()).then_some(user),
        });
    }
    let l = stderr.to_ascii_lowercase();
    if l.contains("permission denied (publickey")
        || (l.contains("the requested url returned error: 403"))
        || l.contains("authentication failed")
    {
        return Some(AuthFailure { seen_as: None });
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
    // The repository a directory belongs to does not change: ask git once.
    type Keys = std::collections::HashMap<PathBuf, PathBuf>;
    static KEYS: std::sync::Mutex<Option<Keys>> = std::sync::Mutex::new(None);
    let known = KEYS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(Default::default)
        .get(cwd)
        .cloned();
    let key = known.unwrap_or_else(|| {
        let k = fetch_key(cwd);
        KEYS.lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_or_insert_with(Default::default)
            .insert(cwd.to_path_buf(), k.clone());
        k
    });
    FETCH_LOCKS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(Default::default)
        .entry(key)
        .or_default()
        .clone()
}

fn fetch_key(cwd: &Path) -> PathBuf {
    let common = proc::output(
        command()
            .args(["rev-parse", "--git-common-dir"])
            .current_dir(cwd),
        default_deadline(),
    )
    .ok()
    .filter(|o| o.status.success())
    .map(|o| cwd.join(String::from_utf8_lossy(&o.stdout).trim()));
    let key = common.unwrap_or_else(|| cwd.to_path_buf());
    std::fs::canonicalize(&key).unwrap_or(key)
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

/// The git to run, resolved once per process. On macOS `git` on PATH is usually
/// `/usr/bin/git`, the xcrun shim: every call costs two execs and a directory-services
/// lookup, which a test suite starting thousands of them a minute turns into a hang.
/// `MASON_GIT` overrides; otherwise macOS asks `xcrun -f git` once; otherwise `git`.
pub fn program() -> &'static Path {
    static GIT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    GIT.get_or_init(|| {
        let xcrun = if cfg!(target_os = "macos") {
            Command::new("/usr/bin/xcrun")
                .args(["-f", "git"])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        } else {
            None
        };
        resolve_program(std::env::var_os("MASON_GIT"), xcrun)
    })
}

/// The choice behind [`program`], from its inputs.
pub fn resolve_program(over: Option<std::ffi::OsString>, xcrun: Option<String>) -> PathBuf {
    if let Some(o) = over.filter(|o| !o.is_empty()) {
        return PathBuf::from(o);
    }
    if let Some(x) = xcrun {
        let p = PathBuf::from(&x);
        if p.is_absolute() && p != Path::new("/usr/bin/git") && is_executable(&p) {
            return p;
        }
    }
    PathBuf::from("git")
}

fn is_executable(p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        p.is_file()
    }
}

/// A `git` command using the resolved [`program`].
pub fn command() -> Command {
    Command::new(program())
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
    let mut cmd = command();
    cmd.args(["-c", "user.name=MASON", "-c", "user.email=mason@localhost"])
        .args(["-c", "commit.gpgsign=false", "-c", "core.editor=true"]);
    if is_network(subcommand(args)) {
        // A connection that stalls or crawls must end, and ssh must never ask.
        cmd.args(["-c", "http.lowSpeedLimit=1", "-c", "http.lowSpeedTime=60"]);
        if std::env::var_os("GIT_SSH_COMMAND").is_none() && std::env::var_os("GIT_SSH").is_none() {
            // The repository's own `core.sshCommand` (a key the owner chose) is the base:
            // GIT_SSH_COMMAND would otherwise take precedence over it and hide it.
            let base = configured_ssh(cwd).unwrap_or_else(|| "ssh".to_string());
            cmd.env("GIT_SSH_COMMAND", format!("{base} {SSH_OPTIONS}"));
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

    /// True once the pid file holds a parsable non-zero pid (it may exist but still be empty).
    fn pid_recorded(dir: &std::path::Path) -> bool {
        std::fs::read_to_string(dir.join("pid"))
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
            .is_some_and(|p| p != 0)
    }

    #[test]
    fn a_hung_git_is_killed_at_its_deadline() {
        let dir = tempfile::tempdir().unwrap();
        // The deadline has to outlast git's own start-up, or git is killed before its alias has
        // written the pid and there is no grandchild to look for (a busy macOS runner took over
        // half a second: ENOENT on the pid file). So a deadline that fell before the pid was
        // written is tried again with a longer one; every attempt must still time out.
        let mut deadline = Duration::from_millis(500);
        loop {
            let e = run_for(
                dir.path(),
                &["-c", "alias.hang=!sleep 30 & echo $! > pid; wait", "hang"],
                deadline,
            )
            .unwrap_err();
            assert!(e.timed_out, "{e}");
            assert!(e.to_string().contains("timed out"));
            if pid_recorded(dir.path()) || deadline >= Duration::from_secs(20) {
                break;
            }
            deadline *= 4;
        }
        // The sleep (a grandchild) died with the group.
        assert!(
            pid_recorded(dir.path()),
            "the helper never recorded its pid after a 20 s deadline"
        );
        let pid = std::fs::read_to_string(dir.path().join("pid")).unwrap();
        // Bounded poll: a late reap is not a failure, only a helper still alive after 5 s is.
        let mut alive = true;
        for _ in 0..100 {
            alive = Command::new("kill")
                .args(["-0", pid.trim()])
                .status()
                .unwrap()
                .success();
            if !alive {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
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

#[cfg(test)]
mod auth_tests {
    use super::*;

    const SEEN: &str = "failed (Some(128)): ERROR: Permission to brianm998/starry_timelapse_airplane_remover.git denied to brian-preffect.\nfatal: Could not read from remote repository.";

    #[test]
    fn recognises_refusals() {
        assert_eq!(
            auth_failure(SEEN),
            Some(AuthFailure {
                seen_as: Some("brian-preffect".into())
            })
        );
        assert!(auth_failure("git@github.com: Permission denied (publickey).").is_some());
        assert!(auth_failure(
            "fatal: unable to access 'https://x/y.git/': The requested URL returned error: 403"
        )
        .is_some());
    }

    #[test]
    fn ignores_other_failures() {
        assert!(
            auth_failure("ssh: connect to host github.com port 22: Operation timed out").is_none()
        );
        assert!(auth_failure(
            " ! [rejected] a -> a (non-fast-forward)\nerror: failed to push some refs"
        )
        .is_none());
    }
}

#[cfg(test)]
mod program_tests {
    use super::*;
    use std::ffi::OsString;

    #[test]
    fn override_wins() {
        let p = resolve_program(Some(OsString::from("/x/git")), Some("/bin/sh".into()));
        assert_eq!(p, PathBuf::from("/x/git"));
    }

    #[test]
    fn falls_back_to_path_without_xcrun() {
        assert_eq!(resolve_program(None, None), PathBuf::from("git"));
        assert_eq!(
            resolve_program(None, Some("relative/git".into())),
            PathBuf::from("git")
        );
        assert_eq!(
            resolve_program(None, Some("/nonexistent/git".into())),
            PathBuf::from("git")
        );
    }

    #[test]
    fn the_shim_is_rejected() {
        assert_eq!(
            resolve_program(None, Some("/usr/bin/git".into())),
            PathBuf::from("git")
        );
    }

    #[test]
    fn an_executable_xcrun_answer_is_used() {
        let p = resolve_program(None, Some("/bin/sh".into()));
        assert_eq!(p, PathBuf::from("/bin/sh"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn on_macos_the_resolved_git_is_not_the_shim() {
        let p = program();
        if p != Path::new("git") && std::env::var_os("MASON_GIT").is_none() {
            assert!(p.is_absolute());
            assert_ne!(p, Path::new("/usr/bin/git"));
        }
    }
}
