// SPDX-License-Identifier: GPL-3.0-or-later

//! Self-reload and updates (Part 15). New code runs from an immutable build in
//! `<data>/runtimes/<sha>/`, never from a checkout that can move. It proves
//! itself on a copy of the database before it takes over, takes over by `exec`
//! (step hosts are separate processes and keep running), and rolls back by
//! itself after three crashes in five minutes. The updater refuses to move to
//! an ancestor of the running version unless asked to roll back.

use crate::daemon::now_ms;
use crate::Daemon;
use mason_landing::git;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

// ---- channels and configuration -------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Channel {
    /// The newest `v*` tag.
    Stable,
    /// The tip of `main`.
    Main,
    /// The tip of a branch.
    Branch(String),
}

impl Channel {
    pub fn parse(s: &str) -> Result<Channel, String> {
        match s {
            "stable" => Ok(Channel::Stable),
            "main" => Ok(Channel::Main),
            _ => match s.strip_prefix("branch:") {
                Some(b) if git::valid_branch(b) => Ok(Channel::Branch(b.to_string())),
                Some(b) => Err(format!("{b:?} is not a plain branch name")),
                None => Err(format!(
                    "a channel is `stable`, `main` or `branch:<name>`, not {s:?}"
                )),
            },
        }
    }

    pub fn name(&self) -> String {
        match self {
            Channel::Stable => "stable".into(),
            Channel::Main => "main".into(),
            Channel::Branch(b) => format!("branch:{b}"),
        }
    }
}

/// Ten minutes: a fetch of a bare mirror is cheap, and landings are not minutes apart for long.
pub const DEFAULT_CHECK_INTERVAL_S: u64 = 600;

/// `<config dir>/update.toml`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateCfg {
    pub channel: Channel,
    /// Where the source is: a path or a URL `git` can clone.
    pub repo: String,
    /// Run in a clean copy of the source; produces the binaries.
    pub build: String,
    /// Where the build leaves them, relative to the source.
    pub out_dir: String,
    /// The files to keep; the first is the daemon.
    pub bins: Vec<String>,
    /// How often to look for a newer version, in seconds; 0 never looks.
    pub check_interval_s: u64,
    /// Run in a clean copy of the source to build the Swift monitor (replaces the default).
    pub monitor_swift_build: Option<String>,
    /// The same for the Compose monitor.
    pub monitor_compose_build: Option<String>,
    /// How long a takeover waits for running landing batches, in seconds; then it goes anyway.
    pub takeover_wait_s: u64,
    /// The code-signing identity installed binaries are signed with (macOS): a certificate's
    /// SHA-1 or its name, e.g. "Developer ID Application: Name (TEAMID)". None signs ad hoc.
    /// A real identity gives the binaries a stable designated requirement (team + identifier),
    /// so permissions granted to MASON survive self-updates.
    pub sign_identity: Option<String>,
}

/// An hour: longer than a batch's verify and CI usually take.
pub const DEFAULT_TAKEOVER_WAIT_S: u64 = 3600;

impl UpdateCfg {
    pub fn parse(text: &str) -> Result<UpdateCfg, String> {
        let doc: toml_edit::DocumentMut = text
            .parse()
            .map_err(|e| format!("update.toml is not valid TOML: {e}"))?;
        let s = |k: &str| doc.get(k).and_then(|v| v.as_str()).map(String::from);
        let need = |k: &str| {
            s(k).filter(|v| !v.trim().is_empty())
                .ok_or_else(|| format!("update.toml: `{k}` is required"))
        };
        let bins: Vec<String> = match doc.get("bins").and_then(|v| v.as_array()) {
            Some(a) => a
                .iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect(),
            None => ["masond", "step-host", "mason-mcp", "fake-claude", "mason"]
                .map(String::from)
                .to_vec(),
        };
        if bins.first().is_none_or(|b| b != "masond") {
            return Err("update.toml: `bins` must start with masond".into());
        }
        if bins.iter().any(|b| b.contains('/') || b.starts_with('.')) {
            return Err("update.toml: `bins` are plain file names".into());
        }
        Ok(UpdateCfg {
            channel: Channel::parse(&s("channel").unwrap_or_else(|| "stable".into()))?,
            repo: need("repo")?,
            build: s("build").unwrap_or_else(|| "cargo build --release --bins".into()),
            out_dir: s("out_dir").unwrap_or_else(|| "target/release".into()),
            bins,
            check_interval_s: doc
                .get("check_interval_s")
                .and_then(|v| v.as_integer())
                .map_or(DEFAULT_CHECK_INTERVAL_S, |n| n.max(0) as u64),
            monitor_swift_build: s("monitor_swift_build"),
            monitor_compose_build: s("monitor_compose_build"),
            takeover_wait_s: doc
                .get("takeover_wait_s")
                .and_then(|v| v.as_integer())
                .map_or(DEFAULT_TAKEOVER_WAIT_S, |n| n.max(0) as u64),
            sign_identity: s("sign_identity").filter(|v| !v.trim().is_empty()),
        })
    }

    pub fn load(config_dir: &Path) -> Result<UpdateCfg, String> {
        let p = config_dir.join("update.toml");
        let t = std::fs::read_to_string(&p).map_err(|_| {
            format!(
                "there is no {}; updates need a source (`repo = ...`)",
                p.display()
            )
        })?;
        UpdateCfg::parse(&t)
    }
}

// ---- the runtimes directory -----------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Runtimes {
    pub root: PathBuf,
}

fn atomic_write(p: &Path, text: &str) -> std::io::Result<()> {
    let tmp = p.with_extension("tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, p)
}

impl Runtimes {
    pub fn new(data_dir: &Path) -> Runtimes {
        Runtimes {
            root: data_dir.join("runtimes"),
        }
    }

    pub fn dir(&self, sha: &str) -> PathBuf {
        self.root.join(sha)
    }

    pub fn mirror(&self) -> PathBuf {
        self.root.join("source.git")
    }

    pub fn monitors(&self) -> PathBuf {
        self.root.join("monitors")
    }

    fn read(&self, name: &str) -> Option<String> {
        std::fs::read_to_string(self.root.join(name))
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    pub fn current(&self) -> Option<String> {
        self.read("current")
    }

    pub fn previous(&self) -> Option<String> {
        self.read("previous")
    }

    /// Make `sha` the runtime that starts, remembering the one it replaces.
    pub fn set_current(&self, sha: &str) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.root)?;
        if let Some(old) = self.current().filter(|o| o != sha) {
            atomic_write(&self.root.join("previous"), &old)?;
        }
        atomic_write(&self.root.join("current"), sha)
    }

    /// Complete means built, copied and marked: a half-copied directory never counts.
    pub fn is_ready(&self, sha: &str) -> bool {
        self.dir(sha).join("READY").is_file()
    }

    pub fn masond(&self, sha: &str) -> PathBuf {
        self.dir(sha).join("masond")
    }

    pub fn bad(&self) -> Vec<String> {
        self.read("bad")
            .map(|t| t.lines().map(|l| l.trim().to_string()).collect())
            .unwrap_or_default()
    }

    pub fn mark_bad(&self, sha: &str) {
        let mut b = self.bad();
        if !b.iter().any(|x| x == sha) {
            b.push(sha.into());
            let _ = atomic_write(&self.root.join("bad"), &b.join("\n"));
        }
    }

    /// A rollback happened: remembered so the daemon can start a cooldown on self-tasks
    /// and file a proposal about it, once.
    pub fn record_rollback(&self, from: &str, to: &str, at_ms: u64) {
        let _ = std::fs::create_dir_all(&self.root);
        let _ = atomic_write(
            &self.root.join("rollback"),
            &format!("{at_ms} {from} {to} open"),
        );
    }

    /// `(at_ms, from, to, proposal_filed)` of the last rollback.
    pub fn rollback(&self) -> Option<(u64, String, String, bool)> {
        let t = self.read("rollback")?;
        let mut p = t.split_whitespace();
        Some((
            p.next()?.parse().ok()?,
            p.next()?.to_string(),
            p.next()?.to_string(),
            p.next()? == "filed",
        ))
    }

    pub fn mark_rollback_filed(&self) {
        if let Some((at, from, to, _)) = self.rollback() {
            let _ = atomic_write(
                &self.root.join("rollback"),
                &format!("{at} {from} {to} filed"),
            );
        }
    }

    /// A note for the next daemon to raise as an alert (it is read once).
    pub fn leave_alert(&self, text: &str) {
        let _ = std::fs::create_dir_all(&self.root);
        let _ = atomic_write(&self.root.join("alert"), text);
    }

    /// The alert, if one stands (it stays until the owner acknowledges it).
    pub fn alert(&self) -> Option<String> {
        self.read("alert")
    }

    pub fn take_alert(&self) -> Option<String> {
        let t = self.read("alert");
        let _ = std::fs::remove_file(self.root.join("alert"));
        t
    }
}

// ---- which version, and may we go there ------------------------------------------------------------

/// Bring the source mirror up to date (cloning it the first time).
pub fn fetch_source(cfg: &UpdateCfg, rt: &Runtimes) -> Result<(), String> {
    let m = rt.mirror();
    std::fs::create_dir_all(&rt.root).map_err(|e| e.to_string())?;
    if !m.join("HEAD").exists() {
        git::run(
            &rt.root,
            &["clone", "-q", "--bare", &cfg.repo, &m.display().to_string()],
        )
        .map_err(|e| e.to_string())?;
    }
    git::run(
        &m,
        &[
            // A connection that stalls must not hold up an update behind it.
            "-c",
            "http.lowSpeedLimit=1",
            "-c",
            "http.lowSpeedTime=60",
            "fetch",
            "-q",
            "--prune",
            "--tags",
            &cfg.repo,
            "+refs/heads/*:refs/heads/*",
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// The commit the channel points at.
pub fn resolve(channel: &Channel, mirror: &Path) -> Result<String, String> {
    let rev = match channel {
        Channel::Stable => {
            let tags = git::run(mirror, &["tag", "--list", "v*", "--sort=-version:refname"])
                .map_err(|e| e.to_string())?;
            let t = tags
                .lines()
                .next()
                .ok_or("the stable channel has no `v*` tag yet")?;
            format!("refs/tags/{t}")
        }
        Channel::Main => "refs/heads/main".to_string(),
        Channel::Branch(b) => format!("refs/heads/{b}"),
    };
    git::rev_parse(mirror, &rev).map_err(|_| {
        format!(
            "the {} channel points at {rev}, which does not exist in the source",
            channel.name()
        )
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Move {
    /// Already running it.
    Same,
    /// The target descends from what runs.
    Forward,
    /// Neither descends from the other (another branch).
    Sideways,
    /// The target is an ancestor of what runs, and a rollback was asked for.
    Rollback,
}

/// The rule that stops an update from silently dropping work: moving to an
/// ancestor of the running version needs an explicit rollback. A running
/// version the source does not know cannot be compared, which is refused too.
pub fn check_move(
    mirror: &Path,
    running: Option<&str>,
    target: &str,
    rollback: bool,
    bad: &[String],
) -> Result<Move, String> {
    if bad.iter().any(|b| b == target) && !rollback {
        return Err(format!("{target} crashed when it ran and was rolled back from; naming it explicitly with --rollback overrides this"));
    }
    let Some(running) = running else {
        return Ok(Move::Forward); // nothing is running from a runtime yet: nothing to drop
    };
    if running == target {
        return Ok(Move::Same);
    }
    if git::rev_parse(mirror, running).is_err() {
        return if rollback {
            Ok(Move::Sideways)
        } else {
            Err(format!("the running version {running} is not in the source, so whether {target} is older cannot be told; use --rollback if you mean it"))
        };
    }
    if git::is_ancestor(mirror, target, running) {
        return if rollback {
            Ok(Move::Rollback)
        } else {
            let n = git::run(
                mirror,
                &["rev-list", "--count", &format!("{target}..{running}")],
            )
            .unwrap_or_default();
            Err(format!("{target} is an ancestor of the running {running}: moving there would drop {n} commit(s). Use --rollback to do that on purpose"))
        };
    }
    if git::is_ancestor(mirror, running, target) {
        Ok(Move::Forward)
    } else {
        Ok(Move::Sideways)
    }
}

// ---- building ----------------------------------------------------------------------------------------

/// How long a build or check step may run: an hour. `MASON_UPDATE_SHELL_TIMEOUT_SECS`
/// changes it.
fn shell_deadline() -> std::time::Duration {
    std::env::var("MASON_UPDATE_SHELL_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|s| *s > 0.0)
        .map_or(
            std::time::Duration::from_secs(3600),
            std::time::Duration::from_secs_f64,
        )
}

/// Run `cmd`, keeping everything it said in `log`. An error carries its last lines.
fn shell(cwd: &Path, cmd: &str, log: &Path, env: &[(&str, &str)]) -> Result<(), String> {
    let limit = shell_deadline();
    let out = match mason_landing::proc::output(
        Command::new("sh")
            .args(["-c", cmd])
            .current_dir(cwd)
            .envs(env.iter().copied()),
        limit,
    ) {
        Ok(o) => o,
        Err(mason_landing::proc::ProcError::Spawn(e)) => return Err(e.to_string()),
        Err(mason_landing::proc::ProcError::TimedOut { stdout, stderr }) => {
            let mut text = String::from_utf8_lossy(&stdout).to_string();
            text.push_str(&String::from_utf8_lossy(&stderr));
            let _ = std::fs::write(log, &text);
            let tail: Vec<&str> = text.lines().rev().take(12).collect();
            return Err(format!(
                "`{cmd}` timed out after {}s and was killed:\n{}",
                limit.as_secs(),
                tail.into_iter().rev().collect::<Vec<_>>().join("\n")
            ));
        }
    };
    let mut text = String::from_utf8_lossy(&out.stdout).to_string();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    let _ = std::fs::write(log, &text);
    if out.status.success() {
        Ok(())
    } else {
        let tail: Vec<&str> = text.lines().rev().take(12).collect();
        Err(format!(
            "`{cmd}` failed:\n{}",
            tail.into_iter().rev().collect::<Vec<_>>().join("\n")
        ))
    }
}

/// How long one `codesign` may take. Signing with a real identity reads the private key from
/// the login keychain; if macOS is waiting for someone to approve that in a dialog, the update
/// fails after this instead of hanging, and the running runtime stays.
pub const SIGN_TIMEOUT: Duration = Duration::from_secs(120);

/// The `codesign` arguments for one installed binary. With an identity, the binary gets the
/// hardened runtime (needed for notarization, harmless otherwise) and a team-anchored
/// designated requirement; without one it is signed ad hoc. Either way the identifier is
/// `com.brianm998.mason.<name>`, the same across builds.
pub fn codesign_args(path: &Path, identity: Option<&str>) -> Vec<std::ffi::OsString> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut a: Vec<std::ffi::OsString> = vec!["--sign".into()];
    match identity {
        Some(id) => {
            a.push(id.into());
            a.push("--options".into());
            a.push("runtime".into());
        }
        None => a.push("-".into()),
    }
    a.push("--force".into());
    a.push("--identifier".into());
    a.push(format!("com.brianm998.mason.{name}").into());
    a.push(path.into());
    a
}

/// Sign an installed binary (macOS only), with `identity` or ad hoc. An unsigned binary has
/// no identity the system can cache, so the system tccd re-evaluates it on every privacy
/// check. masond is the responsible process for everything the pipeline runs on the external
/// volume, so an unsigned masond kept the system tccd at 100% CPU, and WindowServer, waiting
/// on it for Input Monitoring checks, froze the screen and was killed by its watchdog
/// (2026-11-12). x86_64 linkers do not sign. A signing failure fails the build rather than
/// installing an unsigned runtime.
pub fn sign_installed(path: &Path, identity: Option<&str>) -> Result<(), String> {
    if !cfg!(target_os = "macos") {
        return Ok(());
    }
    let mut child = Command::new("/usr/bin/codesign")
        .args(codesign_args(path, identity))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("running codesign on {}: {e}", path.display()))?;
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if started.elapsed() >= SIGN_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "codesign {} did not finish in {} s; if it is waiting for keychain \
                     approval, sign once from Terminal and choose Always Allow",
                    path.display(),
                    SIGN_TIMEOUT.as_secs()
                ));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(e) => return Err(format!("waiting for codesign: {e}")),
        }
    };
    if !status.success() {
        let mut err = String::new();
        if let Some(mut e) = child.stderr.take() {
            let _ = std::io::Read::read_to_string(&mut e, &mut err);
        }
        return Err(format!(
            "codesign {} failed: {}",
            path.display(),
            err.trim()
        ));
    }
    Ok(())
}

/// Build `sha` into `runtimes/<sha>/`: a clean copy of the source at that
/// commit, the configured build, the binaries copied, then the READY mark.
/// Safe to repeat; an already complete runtime is left alone.
pub fn build(cfg: &UpdateCfg, rt: &Runtimes, sha: &str) -> Result<(), String> {
    if rt.is_ready(sha) {
        return Ok(());
    }
    let src = rt.root.join("src").join(sha);
    let _ = std::fs::remove_dir_all(&src);
    std::fs::create_dir_all(&src).map_err(|e| e.to_string())?;
    let log = rt.root.join(format!("{sha}.log"));
    let sh = |cwd: &Path, cmd: &str| shell(cwd, cmd, &log, &[]);
    sh(
        &rt.root,
        &format!(
            "git --git-dir='{}' archive {sha} | tar -x -C '{}'",
            rt.mirror().display(),
            src.display()
        ),
    )?;
    sh(&src, &cfg.build)?;
    let dest = rt.root.join(format!("{sha}.partial"));
    let _ = std::fs::remove_dir_all(&dest);
    std::fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
    for b in &cfg.bins {
        let from = src.join(&cfg.out_dir).join(b);
        if !from.is_file() {
            if b == "masond" || b == "step-host" || b == "mason-mcp" {
                return Err(format!("the build did not produce {}", from.display()));
            }
            continue; // the optional ones (fake-claude, mason) may be absent
        }
        std::fs::copy(&from, dest.join(b)).map_err(|e| format!("copying {b}: {e}"))?;
        sign_installed(&dest.join(b), cfg.sign_identity.as_deref())?;
    }
    std::fs::write(dest.join("SHA"), sha).map_err(|e| e.to_string())?;
    std::fs::write(dest.join("READY"), "").map_err(|e| e.to_string())?;
    let _ = std::fs::remove_dir_all(rt.dir(sha));
    std::fs::rename(&dest, rt.dir(sha)).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_dir_all(&src);
    Ok(())
}

/// Run the new build's own self-test (`masond selftest --db COPY`) on a copy of
/// the database, with a time limit. Ok carries what it said.
pub fn selftest(
    rt: &Runtimes,
    sha: &str,
    db_copy: &Path,
    limit: std::time::Duration,
) -> Result<String, String> {
    let mut child = Command::new(rt.masond(sha))
        .args(["selftest", "--db"])
        .arg(db_copy)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run the new build: {e}"))?;
    let end = std::time::Instant::now() + limit;
    loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(_) => break,
            None if std::time::Instant::now() > end => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "the self-test did not finish in {} s",
                    limit.as_secs()
                ));
            }
            None => std::thread::sleep(std::time::Duration::from_millis(100)),
        }
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if out.status.success() {
        Ok(said.trim().to_string())
    } else {
        Err(format!("the self-test failed:\n{}", said.trim()))
    }
}

// ---- boots and rollback ------------------------------------------------------------------------------------

/// The record of starts. A start is a crash if the daemon did not live to be
/// called healthy before the next start.
pub struct Boots {
    path: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Boot {
    pub at_ms: u64,
    pub sha: String,
    /// `start`, `healthy` or `refused` (a start that declined to run: not a crash).
    pub event: String,
}

impl Boots {
    pub fn new(rt: &Runtimes) -> Boots {
        Boots {
            path: rt.root.join("boots.log"),
        }
    }

    pub fn record(&self, at_ms: u64, sha: &str, event: &str) {
        use std::io::Write;
        if let Some(p) = self.path.parent() {
            let _ = std::fs::create_dir_all(p);
        }
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            let _ = writeln!(f, "{at_ms} {sha} {event}");
        }
    }

    pub fn all(&self) -> Vec<Boot> {
        std::fs::read_to_string(&self.path)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| {
                let mut p = l.split_whitespace();
                Some(Boot {
                    at_ms: p.next()?.parse().ok()?,
                    sha: p.next()?.to_string(),
                    event: p.next()?.to_string(),
                })
            })
            .collect()
    }

    /// Starts of `sha` within the window that never became healthy before the next start.
    pub fn crashes(&self, sha: &str, now_ms: u64, window_ms: u64) -> usize {
        crashes_in(&self.all(), sha, now_ms, window_ms)
    }
}

/// Pure: the crashes of `sha` in a boot log, counted when a new start is about
/// to be recorded: every earlier start in the window that was not followed by a
/// healthy mark (a healthy mark clears what came before it) is one.
pub fn crashes_in(log: &[Boot], sha: &str, now_ms: u64, window_ms: u64) -> usize {
    let mut open: Vec<u64> = vec![];
    for b in log.iter().filter(|b| b.sha == sha) {
        match b.event.as_str() {
            "start" => open.push(b.at_ms),
            "healthy" | "stopped" => open.clear(),
            // The start declined to run (a daemon already there, a port taken):
            // it cancels the start the loader recorded just before it.
            "refused" => {
                open.pop();
            }
            _ => {}
        }
    }
    open.iter()
        .filter(|t| now_ms.saturating_sub(**t) <= window_ms)
        .count()
}

pub const CRASH_LIMIT: usize = 3;
pub const CRASH_WINDOW_MS: u64 = 5 * 60 * 1000;

/// What a start should do: run `sha` as it is, or roll back first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    Run(String),
    /// Three crashes of `from` in the window: run `to` instead.
    RollBack {
        from: String,
        to: String,
    },
    /// Crashing and nothing to go back to: run it anyway and say so.
    RunAnyway {
        sha: String,
        crashes: usize,
    },
}

/// Called at a managed start, before this start is recorded: the earlier
/// starts of `current` that died, three within five minutes, mean it is rolled back.
pub fn decide(current: &str, previous: Option<&str>, log: &[Boot], now_ms: u64) -> Decision {
    let crashes = crashes_in(log, current, now_ms, CRASH_WINDOW_MS);
    if crashes >= CRASH_LIMIT {
        match previous {
            Some(p) => Decision::RollBack {
                from: current.into(),
                to: p.into(),
            },
            None => Decision::RunAnyway {
                sha: current.into(),
                crashes,
            },
        }
    } else {
        Decision::Run(current.into())
    }
}

// ---- the update itself --------------------------------------------------------------------------------------

/// Where an update is, for `mason update` to show.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    /// `idle`, `fetching`, `building`, `testing`, `waiting for landing`, `taking over`, `failed`.
    pub phase: String,
    pub target: Option<String>,
    pub error: Option<String>,
    pub log: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Request {
    pub channel: Option<Channel>,
    /// An exact commit instead of a channel.
    pub sha: Option<String>,
    pub rollback: bool,
    /// Build and test, but do not take over.
    pub check_only: bool,
    /// Take over without waiting for running landing batches (they are abandoned).
    pub now: bool,
}

fn say(d: &Daemon, phase: &str, line: &str) {
    let mut s = d.update_status.lock().unwrap_or_else(|e| e.into_inner());
    s.phase = phase.into();
    s.log.push(line.into());
    drop(s);
    tracing::info!(phase, "{line}");
    d.poke();
}

fn fail(d: &Daemon, why: String) -> String {
    let mut s = d.update_status.lock().unwrap_or_else(|e| e.into_inner());
    s.phase = "failed".into();
    s.error = Some(why.clone());
    s.log.push(format!("failed: {why}"));
    drop(s);
    d.poke();
    why
}

/// Resolve, check the direction, build, back up, self-test; then (unless asked
/// only to check) take over. Returns the commit it moved to (or would).
/// A failure at any step leaves the running daemon exactly as it was.
pub fn run(d: &Arc<Daemon>, req: &Request) -> Result<String, String> {
    {
        let mut s = d.update_status.lock().unwrap_or_else(|e| e.into_inner());
        if matches!(
            s.phase.as_str(),
            "fetching" | "building" | "testing" | "waiting for landing" | "taking over"
        ) {
            return Err("an update is already in progress".into());
        }
        *s = Status {
            phase: "fetching".into(),
            ..Default::default()
        };
    }
    let go = || -> Result<String, String> {
        let cfg = UpdateCfg::load(&d.cfg.config_dir)?;
        let rt = Runtimes::new(&d.cfg.data_dir);
        say(
            d,
            "fetching",
            &format!("fetching the source of {}", cfg.repo),
        );
        fetch_locked(d, &cfg, &rt)?;
        let channel = req.channel.clone().unwrap_or_else(|| cfg.channel.clone());
        let target = match &req.sha {
            Some(s) => git::rev_parse(&rt.mirror(), s)
                .map_err(|_| format!("{s} is not a commit in the source"))?,
            None => resolve(&channel, &rt.mirror())?,
        };
        d.update_status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .target = Some(target.clone());
        let running = d.cfg.runtime_sha.clone().or_else(|| rt.current());
        let mv = check_move(
            &rt.mirror(),
            running.as_deref(),
            &target,
            req.rollback,
            &rt.bad(),
        )?;
        say(
            d,
            "fetching",
            &format!("{}: {target} ({mv:?})", channel.name()),
        );
        if mv == Move::Same && !req.rollback {
            return Err(format!("already running {target}"));
        }
        say(
            d,
            "building",
            &format!("building {target} into runtimes/{target}/"),
        );
        build(&cfg, &rt, &target)?;
        // Prove it on a copy of the real database, in a sandbox, before it is allowed near the real one.
        say(
            d,
            "testing",
            "backing up the database, then the new build's self-test",
        );
        let backups = rt.root.join("backups");
        std::fs::create_dir_all(&backups).map_err(|e| e.to_string())?;
        let from = running.clone().unwrap_or_else(|| "dev".into());
        let backup = backups.join(format!(
            "store-{}-{}.db",
            now_ms(),
            &from[..from.len().min(12)]
        ));
        d.store
            .backup_to(&backup)
            .map_err(|e| format!("could not back up the database: {e}"))?;
        let copy = backups.join(format!("selftest-{}.db", std::process::id()));
        std::fs::copy(&backup, &copy).map_err(|e| e.to_string())?;
        let said = selftest(&rt, &target, &copy, std::time::Duration::from_secs(600));
        let _ = std::fs::remove_file(&copy);
        for ext in ["db-wal", "db-shm"] {
            let _ = std::fs::remove_file(copy.with_extension(ext));
        }
        let said = said?;
        for l in said.lines() {
            say(d, "testing", l);
        }
        if req.check_only {
            say(
                d,
                "idle",
                &format!(
                    "checked: {target} builds and passes its self-test; not taking over (--check)"
                ),
            );
            d.update_status
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .phase = "idle".into();
            d.poke();
            return Ok(target);
        }
        say(
            d,
            "taking over",
            &format!(
                "taking over: {from} -> {target}; step hosts keep running; backup {}",
                backup.display()
            ),
        );
        Ok(target)
    };
    match go() {
        Ok(t) => Ok(t),
        Err(e) => Err(fail(d, e)),
    }
}

/// One fetch into the mirror at a time: the periodic check and an update share it.
fn fetch_locked(d: &Daemon, cfg: &UpdateCfg, rt: &Runtimes) -> Result<(), String> {
    let _one = d.update_fetch.lock().unwrap_or_else(|e| e.into_inner());
    fetch_source(cfg, rt)
}

// ---- noticing a newer version ----------------------------------------------------------------------

/// The monitors the daemon can build, and the source each is made from. A commit
/// that touches none of a monitor's paths does not make that monitor out of date.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MonitorKind {
    pub name: &'static str,
    paths: &'static [&'static str],
    /// The default build, run in a clean copy of the source, with the commit in
    /// `MASON_BUILD_SHA` and the directory to leave the result in in `MASON_MONITOR_OUT`.
    build: &'static str,
    /// What the build leaves in that directory: the thing a monitor launches.
    product: &'static str,
}

pub const MONITORS: [MonitorKind; 2] = [
    MonitorKind {
        name: "swift",
        // The generated Swift protocol code and tokens are checked in under clients/swift.
        paths: &["clients/swift", "scripts/bundle-monitor.sh"],
        build: "bash scripts/bundle-monitor.sh \"$MASON_MONITOR_OUT\"",
        product: "MASON.app",
    },
    MonitorKind {
        name: "compose",
        // Wire generates the Kotlin protocol code from the schema at build time.
        paths: &["clients/compose", "clients/tokens.toml", "schemas/mason.proto"],
        build: "cd clients/compose && gradle --no-daemon -q -Pmason.build=\"$MASON_BUILD_SHA\" :app:installDist && cp -R app/build/install/app \"$MASON_MONITOR_OUT/app\"",
        product: "app/bin/mason-monitor",
    },
];

pub fn monitor_kind(name: &str) -> Option<MonitorKind> {
    MONITORS.into_iter().find(|m| m.name == name)
}

/// Paths whose changes do not reach the daemon's binaries: a landing that only
/// regenerates the README is not an update worth building.
const NOT_THE_DAEMON: [&str; 4] = [
    ":(exclude)docs",
    ":(exclude)clients",
    ":(exclude).github",
    ":(exclude,top,glob)*.md",
];

/// What the last look at the channel found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Check {
    pub channel: String,
    /// The channel's tip.
    pub latest: String,
    /// How many commits the daemon is behind, when `outdated`.
    pub behind: u32,
    /// The daemon is an ancestor of the tip, and something that goes into it changed.
    pub outdated: bool,
    /// `(kind, the newest commit that changed that monitor)`.
    pub monitors: Vec<(String, String)>,
    pub at_ms: u64,
    /// Why the last look failed; what it found before that stands.
    pub error: Option<String>,
}

/// Compare what runs with the channel, in the mirror. `running` is `None` for a
/// daemon run from a build directory, which has no commit to compare and does not nag.
/// Only a plain step forward counts: a downgrade, a sideways branch and a build that
/// was rolled back from are for someone to ask for by name.
pub fn assess(
    channel: &Channel,
    mirror: &Path,
    running: Option<&str>,
    bad: &[String],
) -> Result<Check, String> {
    let latest = resolve(channel, mirror)?;
    let mut c = Check {
        channel: channel.name(),
        latest: latest.clone(),
        ..Default::default()
    };
    if let Some(r) = running {
        if check_move(mirror, Some(r), &latest, false, bad) == Ok(Move::Forward) {
            let mut diff = vec!["diff", "--name-only", r, &latest, "--"];
            diff.extend(NOT_THE_DAEMON);
            c.outdated = !git::run(mirror, &diff)
                .map_err(|e| e.to_string())?
                .is_empty();
            c.behind = git::commits_ahead(mirror, r, &latest).map_err(|e| e.to_string())?;
        }
    }
    for m in MONITORS {
        c.monitors
            .push((m.name.into(), latest_of(m, mirror, &latest)?));
    }
    Ok(c)
}

/// The newest commit at or before `tip` that changed this monitor.
fn latest_of(m: MonitorKind, mirror: &Path, tip: &str) -> Result<String, String> {
    let mut args = vec!["rev-list", "-1", tip, "--"];
    args.extend(m.paths);
    git::run(mirror, &args).map_err(|e| e.to_string())
}

/// Look at the channel once, and tell the monitors if that changed anything.
/// Returns how long to wait before looking again.
pub fn check_once(d: &Arc<Daemon>) -> std::time::Duration {
    let cfg = match UpdateCfg::load(&d.cfg.config_dir) {
        Ok(c) => c,
        Err(_) => {
            set_check(d, Check::default());
            return std::time::Duration::from_secs(DEFAULT_CHECK_INTERVAL_S);
        }
    };
    if cfg.check_interval_s == 0 {
        set_check(d, Check::default());
        return std::time::Duration::from_secs(DEFAULT_CHECK_INTERVAL_S);
    }
    let rt = Runtimes::new(&d.cfg.data_dir);
    let found = fetch_locked(d, &cfg, &rt).and_then(|()| {
        assess(
            &cfg.channel,
            &rt.mirror(),
            d.cfg.runtime_sha.as_deref(),
            &rt.bad(),
        )
    });
    let next = match found {
        Ok(c) => Check {
            at_ms: now_ms(),
            ..c
        },
        Err(e) => {
            tracing::warn!("could not look for a newer version: {e}");
            Check {
                at_ms: now_ms(),
                error: Some(e),
                ..d.update_check
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone()
            }
        }
    };
    set_check(d, next);
    std::time::Duration::from_secs(cfg.check_interval_s)
}

fn set_check(d: &Daemon, new: Check) {
    let mut c = d.update_check.lock().unwrap_or_else(|e| e.into_inner());
    // A push only when what a monitor would show moved, not every ten minutes.
    let same = Check {
        at_ms: c.at_ms,
        error: c.error.clone(),
        ..new.clone()
    } == *c;
    *c = new;
    drop(c);
    if !same {
        d.poke();
    }
}

/// Look now, and every `check_interval_s` after that, for as long as the daemon lives.
pub fn spawn_checker(d: &Arc<Daemon>) {
    crate::bg::spawn_loop(d, "update-check", move |weak| {
        let Some(d) = weak.upgrade() else {
            return false;
        };
        let wait = check_once(&d);
        drop(d);
        std::thread::sleep(wait);
        true
    });
}

/// What the header says. Text is made here so that the two monitors say the same thing.
pub fn info(d: &Daemon) -> Option<mason_protocol::UpdateInfo> {
    let c = d
        .update_check
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    if c.latest.is_empty() {
        return None;
    }
    let s = d
        .update_status
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let (daemon_text, daemon_can_update) =
        daemon_banner(&c, d.cfg.runtime_sha.as_deref().unwrap_or_default(), &s);
    let builds = d
        .monitor_builds
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let monitors = c
        .monitors
        .iter()
        .map(|(kind, latest)| {
            let b = builds.get(kind).cloned().unwrap_or_default();
            mason_protocol::MonitorBuild {
                kind: kind.clone(),
                latest: latest.clone(),
                phase: b.phase,
                error: b.error,
                path: b.path,
            }
        })
        .collect();
    Some(mason_protocol::UpdateInfo {
        daemon_text,
        daemon_can_update,
        monitors,
    })
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(8)]
}

/// Pure: the sentence about the daemon, and whether the button is live.
fn daemon_banner(c: &Check, running: &str, s: &Status) -> (String, bool) {
    let target = s.target.as_deref().unwrap_or(&c.latest);
    match s.phase.as_str() {
        "fetching" => ("Updating masond: fetching the source…".into(), false),
        "building" => (
            format!("Updating masond: building {}…", short(target)),
            false,
        ),
        "testing" => (
            format!(
                "Updating masond: testing {} on a copy of the database…",
                short(target)
            ),
            false,
        ),
        "taking over" => (
            format!("Updating masond: restarting on {}…", short(target)),
            false,
        ),
        "failed" if c.outdated => {
            let why = s
                .error
                .as_deref()
                .unwrap_or("")
                .lines()
                .next()
                .unwrap_or("");
            (
                format!("The masond update failed and the running daemon is unchanged: {why} (`mason update status` has more)"),
                true,
            )
        }
        _ if c.outdated => {
            let (n, plural) = (c.behind, if c.behind == 1 { "" } else { "s" });
            let channel = c.channel.strip_prefix("branch:").unwrap_or(&c.channel);
            (
                format!(
                    "masond is {n} commit{plural} behind {channel} ({} → {}).",
                    short(running),
                    short(&c.latest)
                ),
                true,
            )
        }
        _ => (String::new(), false),
    }
}

// ---- building a monitor ------------------------------------------------------------------------------

/// Where the build of one kind of monitor is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MonitorState {
    /// `""`, `building`, `ready` or `failed`.
    pub phase: String,
    pub error: String,
    pub path: String,
}

/// Build the monitor at `sha` into `runtimes/monitors/<kind>-<sha>/`, in a clean
/// copy of the source, complete or absent like a daemon runtime. Returns what to
/// launch. Safe to repeat; a finished build is left alone.
pub fn build_monitor(
    cfg: &UpdateCfg,
    rt: &Runtimes,
    kind: MonitorKind,
    sha: &str,
) -> Result<PathBuf, String> {
    let name = format!("{}-{sha}", kind.name);
    let done = rt.monitors().join(&name);
    if done.join("READY").is_file() {
        return Ok(done.join(kind.product));
    }
    let command = match kind.name {
        "swift" => cfg.monitor_swift_build.as_deref(),
        _ => cfg.monitor_compose_build.as_deref(),
    }
    .unwrap_or(kind.build);
    let src = rt.root.join("src").join(format!("monitor-{name}"));
    let partial = rt.monitors().join(format!("{name}.partial"));
    let log = rt.monitors().join(format!("{name}.log"));
    for d in [&src, &partial] {
        let _ = std::fs::remove_dir_all(d);
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    shell(
        &rt.root,
        &format!(
            "git --git-dir='{}' archive {sha} | tar -x -C '{}'",
            rt.mirror().display(),
            src.display()
        ),
        &log,
        &[],
    )?;
    let out = partial.display().to_string();
    let built = shell(
        &src,
        command,
        &log,
        &[("MASON_BUILD_SHA", sha), ("MASON_MONITOR_OUT", &out)],
    );
    let _ = std::fs::remove_dir_all(&src);
    built?;
    if !partial.join(kind.product).exists() {
        return Err(format!(
            "the monitor build did not produce {}",
            kind.product
        ));
    }
    std::fs::write(partial.join("READY"), "").map_err(|e| e.to_string())?;
    let _ = std::fs::remove_dir_all(&done);
    std::fs::rename(&partial, &done).map_err(|e| e.to_string())?;
    prune_monitors(rt, kind, &name);
    Ok(done.join(kind.product))
}

/// Keep the build just made and the newest one before it (the monitor being
/// replaced may still be running from that one).
fn prune_monitors(rt: &Runtimes, kind: MonitorKind, keep: &str) {
    let Ok(entries) = std::fs::read_dir(rt.monitors()) else {
        return;
    };
    let prefix = format!("{}-", kind.name);
    let mut old: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .filter(|e| {
            let n = e.file_name().to_string_lossy().to_string();
            n.starts_with(&prefix)
                && n != keep
                && e.file_type().is_ok_and(|t| t.is_dir())
                && !n.ends_with(".partial")
        })
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    old.sort();
    old.pop();
    for (_, p) in old {
        let _ = std::fs::remove_dir_all(&p);
        let _ = std::fs::remove_file(p.with_extension("log"));
    }
}

fn set_monitor(d: &Daemon, kind: &str, state: MonitorState) {
    d.monitor_builds
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(kind.into(), state);
    d.poke();
}

/// Start building the newest version of a monitor, unless one is being built.
/// The state says `building` before this returns; the rest happens on its own thread.
pub fn start_monitor_build(d: &Arc<Daemon>, kind: MonitorKind) -> Result<(), String> {
    {
        let mut b = d.monitor_builds.lock().unwrap_or_else(|e| e.into_inner());
        if b.get(kind.name).is_some_and(|s| s.phase == "building") {
            return Err(format!("the {} monitor is already being built", kind.name));
        }
        b.insert(
            kind.name.into(),
            MonitorState {
                phase: "building".into(),
                ..Default::default()
            },
        );
    }
    d.poke();
    let d = d.clone();
    let _ = std::thread::Builder::new()
        .name("monitor-build".into())
        .spawn(move || {
            let go = || -> Result<PathBuf, String> {
                let cfg = UpdateCfg::load(&d.cfg.config_dir)?;
                let rt = Runtimes::new(&d.cfg.data_dir);
                fetch_locked(&d, &cfg, &rt)?;
                let tip = resolve(&cfg.channel, &rt.mirror())?;
                let sha = latest_of(kind, &rt.mirror(), &tip)?;
                build_monitor(&cfg, &rt, kind, &sha)
            };
            let state = match go() {
                Ok(p) => MonitorState {
                    phase: "ready".into(),
                    path: p.display().to_string(),
                    ..Default::default()
                },
                Err(e) => {
                    tracing::warn!(kind = kind.name, "monitor build failed: {e}");
                    MonitorState {
                        phase: "failed".into(),
                        error: e.lines().next().unwrap_or_default().to_string(),
                        ..Default::default()
                    }
                }
            };
            set_monitor(&d, kind.name, state);
        });
    Ok(())
}

/// The unfinished landing batches, as `<project> #<batch> (<phase>)`.
pub fn running_batch_lines(d: &Daemon) -> Vec<String> {
    d.store
        .batches(None, 200)
        .unwrap_or_default()
        .into_iter()
        .filter(|b| b.finished_at.is_none())
        .map(|b| format!("{} #{} ({})", b.project, b.id, b.phase))
        .collect()
}

/// Wait until no project has a landing batch running, so the takeover abandons none. While
/// it waits no new batch starts and queued tasks keep their place. Gives up after
/// `takeover_wait_s` (update.toml) and says which batches will be abandoned. The flag is left
/// set: [`drain_and_take_over`] clears it if the takeover fails.
pub fn drain(d: &Arc<Daemon>) {
    let cap =
        UpdateCfg::load(&d.cfg.config_dir).map_or(DEFAULT_TAKEOVER_WAIT_S, |c| c.takeover_wait_s);
    drain_within(d, Duration::from_secs(cap));
}

/// [`drain`] with the cap given (zero: do not wait at all).
pub fn drain_within(d: &Arc<Daemon>, cap: Duration) {
    d.landing.set_draining(true);
    let deadline = Instant::now() + cap;
    let mut said = String::new();
    loop {
        if d.landing.busy_projects().is_empty() {
            return;
        }
        let lines = running_batch_lines(d);
        if Instant::now() >= deadline {
            say(
                d,
                "taking over",
                &format!(
                    "the landing wait of {}s is over; taking over anyway, which abandons: {}",
                    cap.as_secs(),
                    if lines.is_empty() {
                        "a running batch".into()
                    } else {
                        lines.join(", ")
                    }
                ),
            );
            return;
        }
        let text = format!(
            "waiting for {} landing batch(es): {}",
            lines.len(),
            lines.join(", ")
        );
        if text != said {
            say(d, "waiting for landing", &text);
            said = text;
        }
        d.landing.wait_idle_once(Duration::from_secs(1));
    }
}

/// [`take_over`] after waiting for the running landing batches (unless `now`). Only returns
/// if the takeover failed; landing is then let go again.
pub fn drain_and_take_over(d: &Arc<Daemon>, sha: &str, now: bool) -> String {
    if !now {
        drain(d);
    }
    let r = take_over(d, sha);
    d.landing.set_draining(false);
    r
}

/// Replace this process with the new runtime, keeping its pid. The step hosts
/// are other processes and do not notice; the new daemon finds them as it
/// would after any restart. Only returns if the exec fails.
pub fn take_over(d: &Arc<Daemon>, sha: &str) -> String {
    let rt = Runtimes::new(&d.cfg.data_dir);
    if let Err(e) = rt.set_current(sha) {
        return fail(d, format!("could not record the new runtime: {e}"));
    }
    Boots::new(&rt).record(now_ms(), sha, "start");
    // Let an event being appended finish; nothing is half-written when we go.
    d.quiesce();
    let mut args = d.cfg.exec_args.clone();
    // The same port, so clients that reconnect find it where it was.
    if let Some(port) = std::fs::read_to_string(d.cfg.info_path())
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v["port"].as_u64())
    {
        if let Some(i) = args.iter().position(|a| a == "--port") {
            args.drain(i..(i + 2).min(args.len()));
        }
        args.extend(["--port".into(), port.to_string()]);
    }
    args.extend(["--runtime-sha".into(), sha.into()]);
    let err = mason_platform::process::replace_process(Command::new(rt.masond(sha)).args(&args));
    // Still here: the exec failed. Put things back.
    if let Some(prev) = d.cfg.runtime_sha.as_deref() {
        let _ = rt.set_current(prev);
    }
    fail(d, format!("could not start the new runtime: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn boot(at: u64, sha: &str, ev: &str) -> Boot {
        Boot {
            at_ms: at,
            sha: sha.into(),
            event: ev.into(),
        }
    }

    /// The copy starts out Apple-signed and ends ad-hoc signed, so the test never
    /// creates (or deletes) an unsigned Mach-O, which can crash syspolicyd.
    #[test]
    #[cfg(target_os = "macos")]
    fn installed_binaries_are_adhoc_signed_with_a_mason_identifier() {
        let d = tempfile::tempdir().unwrap();
        let bin = d.path().join("masond");
        std::fs::copy("/usr/bin/true", &bin).unwrap();
        sign_installed(&bin, None).unwrap();
        let out = Command::new("/usr/bin/codesign")
            .arg("-dv")
            .arg(&bin)
            .output()
            .unwrap();
        let info = String::from_utf8_lossy(&out.stderr);
        assert!(info.contains("Signature=adhoc"), "{info}");
        assert!(
            info.contains("Identifier=com.brianm998.mason.masond"),
            "{info}"
        );
        assert!(Command::new(&bin).status().unwrap().success());
    }

    #[test]
    fn signing_reports_a_missing_file() {
        let d = tempfile::tempdir().unwrap();
        let missing = d.path().join("absent");
        if cfg!(target_os = "macos") {
            assert!(sign_installed(&missing, None).is_err());
        } else {
            assert!(sign_installed(&missing, None).is_ok());
        }
    }

    /// Checked as arguments, not by signing: a real identity reads the login keychain, which
    /// can put an approval dialog on screen, and nothing automated may do that.
    #[test]
    fn an_identity_signs_with_the_hardened_runtime_and_the_same_identifier() {
        let p = Path::new("/rt/abc/step-host");
        let with: Vec<String> = codesign_args(p, Some("Developer ID Application: X (T1)"))
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            with,
            [
                "--sign",
                "Developer ID Application: X (T1)",
                "--options",
                "runtime",
                "--force",
                "--identifier",
                "com.brianm998.mason.step-host",
                "/rt/abc/step-host"
            ]
        );
        let adhoc: Vec<String> = codesign_args(p, None)
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            adhoc,
            [
                "--sign",
                "-",
                "--force",
                "--identifier",
                "com.brianm998.mason.step-host",
                "/rt/abc/step-host"
            ]
        );
    }

    #[test]
    fn the_signing_identity_is_optional_and_blank_means_ad_hoc() {
        let none = UpdateCfg::parse("repo = \"/src\"\n").unwrap();
        assert_eq!(none.sign_identity, None);
        let blank = UpdateCfg::parse("repo = \"/src\"\nsign_identity = \"  \"\n").unwrap();
        assert_eq!(blank.sign_identity, None);
        let set = UpdateCfg::parse(
            "repo = \"/src\"\nsign_identity = \"79BBD50DFCA34E5242E55366B00A8D950F5A3A59\"\n",
        )
        .unwrap();
        assert_eq!(
            set.sign_identity.as_deref(),
            Some("79BBD50DFCA34E5242E55366B00A8D950F5A3A59")
        );
    }

    #[test]
    fn channels_parse() {
        assert_eq!(Channel::parse("stable").unwrap(), Channel::Stable);
        assert_eq!(Channel::parse("main").unwrap(), Channel::Main);
        assert_eq!(
            Channel::parse("branch:feature/x").unwrap(),
            Channel::Branch("feature/x".into())
        );
        assert!(Channel::parse("branch:-rf").is_err());
        assert!(Channel::parse("nightly").is_err());
        assert_eq!(Channel::Branch("a".into()).name(), "branch:a");
    }

    #[test]
    fn the_config_names_what_is_missing() {
        let c = UpdateCfg::parse("repo = \"/src\"\nchannel = \"main\"\n").unwrap();
        assert_eq!(
            (c.channel, c.bins[0].as_str(), c.out_dir.as_str()),
            (Channel::Main, "masond", "target/release")
        );
        assert!(UpdateCfg::parse("channel = \"main\"")
            .unwrap_err()
            .contains("`repo` is required"));
        assert!(UpdateCfg::parse("repo = \"x\"\nbins = [\"step-host\"]")
            .unwrap_err()
            .contains("start with masond"));
        assert!(
            UpdateCfg::parse("repo = \"x\"\nbins = [\"masond\", \"../evil\"]")
                .unwrap_err()
                .contains("plain file names")
        );
        assert!(UpdateCfg::parse("repo = \"x\"\nchannel = \"nope\"").is_err());
    }

    #[test]
    fn stopped_starts_are_not_crashes() {
        let m = 60_000;
        let mut log = vec![];
        for i in 0..3 {
            log.push(boot(i * m, "new", "start"));
            log.push(boot(i * m + 1, "new", "stopped"));
        }
        assert_eq!(crashes_in(&log, "new", 3 * m, CRASH_WINDOW_MS), 0);
        assert_eq!(
            decide("new", Some("old"), &log, 3 * m),
            Decision::Run("new".into())
        );
        // Starts with no ending still count.
        let log: Vec<Boot> = (0..3).map(|i| boot(i * m, "new", "start")).collect();
        assert_eq!(crashes_in(&log, "new", 3 * m, CRASH_WINDOW_MS), 3);
    }

    #[test]
    fn refused_starts_are_not_crashes() {
        let m = 60_000;
        let mut log = vec![];
        for i in 0..3 {
            log.push(boot(i * m, "new", "start"));
            log.push(boot(i * m + 1, "new", "refused"));
        }
        assert_eq!(crashes_in(&log, "new", 3 * m, CRASH_WINDOW_MS), 0);
        assert_eq!(
            decide("new", Some("old"), &log, 3 * m),
            Decision::Run("new".into())
        );
        // Real crashes still count, refusals among them do not.
        let log = vec![
            boot(0, "new", "start"),
            boot(m, "new", "start"),
            boot(m + 1, "new", "refused"),
            boot(2 * m, "new", "start"),
            boot(3 * m, "new", "start"),
        ];
        assert_eq!(crashes_in(&log, "new", 4 * m, CRASH_WINDOW_MS), 3);
        let log = vec![
            boot(0, "new", "start"),
            boot(m, "new", "start"),
            boot(2 * m, "new", "start"),
        ];
        assert!(matches!(
            decide("new", Some("old"), &log, 3 * m),
            Decision::RollBack { .. }
        ));
    }

    #[test]
    fn three_crashes_in_five_minutes_roll_back() {
        let m = 60_000;
        // Three starts, none healthy: the fourth start rolls back.
        let log = vec![
            boot(0, "new", "start"),
            boot(m, "new", "start"),
            boot(2 * m, "new", "start"),
        ];
        assert_eq!(
            decide("new", Some("old"), &log, 3 * m),
            Decision::RollBack {
                from: "new".into(),
                to: "old".into()
            }
        );
        // Two crashes: the third start still runs.
        let log = vec![boot(0, "new", "start"), boot(m, "new", "start")];
        assert_eq!(
            decide("new", Some("old"), &log, 2 * m),
            Decision::Run("new".into())
        );
        // A healthy mark in between resets the count.
        let log = vec![
            boot(0, "new", "start"),
            boot(m, "new", "start"),
            boot(2 * m, "new", "healthy"),
            boot(3 * m, "new", "start"),
        ];
        assert_eq!(
            decide("new", Some("old"), &log, 4 * m),
            Decision::Run("new".into())
        );
        // Crashes spread over more than five minutes do not add up.
        let log = vec![
            boot(0, "new", "start"),
            boot(4 * m, "new", "start"),
            boot(8 * m, "new", "start"),
        ];
        assert_eq!(
            decide("new", Some("old"), &log, 9 * m),
            Decision::Run("new".into())
        );
        // Nothing to go back to: run anyway.
        let log = vec![
            boot(0, "new", "start"),
            boot(m, "new", "start"),
            boot(2 * m, "new", "start"),
        ];
        assert!(matches!(
            decide("new", None, &log, 3 * m),
            Decision::RunAnyway { crashes: 3, .. }
        ));
        // Another version's crashes are not this one's.
        let log = vec![
            boot(0, "old", "start"),
            boot(m, "old", "start"),
            boot(2 * m, "old", "start"),
        ];
        assert_eq!(
            decide("new", Some("old"), &log, 3 * m),
            Decision::Run("new".into())
        );
    }

    #[test]
    fn the_runtimes_directory_keeps_current_previous_and_bad() {
        let d = tempfile::tempdir().unwrap();
        let rt = Runtimes::new(d.path());
        assert_eq!((rt.current(), rt.previous()), (None, None));
        rt.set_current("aaa").unwrap();
        rt.set_current("bbb").unwrap();
        assert_eq!(
            (rt.current().as_deref(), rt.previous().as_deref()),
            (Some("bbb"), Some("aaa"))
        );
        rt.set_current("bbb").unwrap();
        assert_eq!(
            rt.previous().as_deref(),
            Some("aaa"),
            "setting the same one again keeps the previous"
        );
        rt.mark_bad("ccc");
        rt.mark_bad("ccc");
        assert_eq!(rt.bad(), ["ccc"]);
        rt.leave_alert("rolled back");
        assert_eq!(rt.take_alert().as_deref(), Some("rolled back"));
        assert_eq!(rt.take_alert(), None, "an alert is read once");
        assert!(!rt.is_ready("bbb"));
    }

    fn sh(cwd: &Path, args: &[&str]) -> String {
        git::run(cwd, args).unwrap_or_else(|e| panic!("{e}"))
    }

    /// A source repository: a -> b -> c on main, a side branch off a, a tag per commit.
    fn source() -> (tempfile::TempDir, Runtimes, UpdateCfg, [String; 4]) {
        let d = tempfile::tempdir().unwrap();
        let repo = d.path().join("src");
        std::fs::create_dir_all(&repo).unwrap();
        sh(&repo, &["init", "-q", "-b", "main"]);
        let mut shas = vec![];
        for (i, f) in ["a", "b", "c"].iter().enumerate() {
            std::fs::write(repo.join(f), f).unwrap();
            sh(&repo, &["add", "-A"]);
            sh(
                &repo,
                &[
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@t",
                    "commit",
                    "-qm",
                    f,
                ],
            );
            sh(&repo, &["tag", &format!("v0.{i}.0")]);
            shas.push(sh(&repo, &["rev-parse", "HEAD"]));
        }
        sh(&repo, &["checkout", "-q", "-b", "side", &shas[0]]);
        std::fs::write(repo.join("s"), "s").unwrap();
        sh(&repo, &["add", "-A"]);
        sh(
            &repo,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-qm",
                "s",
            ],
        );
        shas.push(sh(&repo, &["rev-parse", "HEAD"]));
        sh(&repo, &["checkout", "-q", "main"]);
        let cfg = UpdateCfg::parse(&format!(
            "repo = \"{}\"\nchannel = \"main\"\n",
            repo.display()
        ))
        .unwrap();
        let rt = Runtimes::new(&d.path().join("data"));
        fetch_source(&cfg, &rt).unwrap();
        (
            d,
            rt,
            cfg,
            [
                shas[0].clone(),
                shas[1].clone(),
                shas[2].clone(),
                shas[3].clone(),
            ],
        )
    }

    #[test]
    fn channels_resolve_to_commits() {
        let (_d, rt, _cfg, s) = source();
        assert_eq!(resolve(&Channel::Main, &rt.mirror()).unwrap(), s[2]);
        assert_eq!(
            resolve(&Channel::Stable, &rt.mirror()).unwrap(),
            s[2],
            "the newest v* tag"
        );
        assert_eq!(
            resolve(&Channel::Branch("side".into()), &rt.mirror()).unwrap(),
            s[3]
        );
        assert!(resolve(&Channel::Branch("nope".into()), &rt.mirror()).is_err());
    }

    #[test]
    fn an_update_to_an_ancestor_is_refused_without_an_explicit_rollback() {
        let (_d, rt, _cfg, s) = source();
        let m = rt.mirror();
        assert_eq!(
            check_move(&m, Some(&s[1]), &s[2], false, &[]).unwrap(),
            Move::Forward
        );
        assert_eq!(
            check_move(&m, Some(&s[2]), &s[2], false, &[]).unwrap(),
            Move::Same
        );
        let e = check_move(&m, Some(&s[2]), &s[0], false, &[]).unwrap_err();
        assert!(
            e.contains("ancestor") && e.contains("2 commit(s)") && e.contains("--rollback"),
            "{e}"
        );
        assert_eq!(
            check_move(&m, Some(&s[2]), &s[0], true, &[]).unwrap(),
            Move::Rollback
        );
        // Another branch is neither forward nor back: allowed.
        assert_eq!(
            check_move(&m, Some(&s[2]), &s[3], false, &[]).unwrap(),
            Move::Sideways
        );
        // A running version the source has never heard of cannot be compared.
        assert!(check_move(&m, Some("deadbeef"), &s[2], false, &[])
            .unwrap_err()
            .contains("not in the source"));
        // A build that crashed and was rolled back from is not tried again by accident.
        assert!(
            check_move(&m, Some(&s[1]), &s[2], false, std::slice::from_ref(&s[2]))
                .unwrap_err()
                .contains("rolled back")
        );
        assert_eq!(
            check_move(&m, Some(&s[1]), &s[2], true, std::slice::from_ref(&s[2])).unwrap(),
            Move::Forward
        );
        // First runtime ever: nothing to drop.
        assert_eq!(
            check_move(&m, None, &s[0], false, &[]).unwrap(),
            Move::Forward
        );
    }

    /// One more commit on main, changing `files`.
    fn commit(repo: &Path, files: &[&str]) -> String {
        for f in files {
            let p = repo.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, format!("{f} {:?}", std::time::SystemTime::now())).unwrap();
        }
        sh(repo, &["add", "-A"]);
        sh(
            repo,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-qm",
                "more",
            ],
        );
        sh(repo, &["rev-parse", "HEAD"])
    }

    #[test]
    fn a_daemon_is_out_of_date_only_when_a_step_forward_changes_what_goes_into_it() {
        let (d, rt, cfg, s) = source();
        let repo = d.path().join("src");
        let m = rt.mirror();
        let main = Channel::Main;
        // At the tip, and from a build directory: nothing to say.
        assert!(!assess(&main, &m, Some(&s[2]), &[]).unwrap().outdated);
        assert!(!assess(&main, &m, None, &[]).unwrap().outdated);

        // One commit forward: behind by one.
        let code = commit(&repo, &["crates/daemon/x.rs"]);
        fetch_source(&cfg, &rt).unwrap();
        let c = assess(&main, &m, Some(&s[2]), &[]).unwrap();
        assert_eq!(
            (c.outdated, c.behind, c.latest.as_str()),
            (true, 1, code.as_str())
        );

        // Commits that cannot change the daemon do not count on their own.
        let tip = commit(
            &repo,
            &["README.md", "docs/stages/s1.md", "clients/swift/App.swift"],
        );
        fetch_source(&cfg, &rt).unwrap();
        let c = assess(&main, &m, Some(&code), &[]).unwrap();
        assert_eq!(c.latest, tip);
        assert!(!c.outdated, "docs and clients only: {c:?}");
        // But they are counted once the daemon is behind for a real reason.
        assert_eq!(assess(&main, &m, Some(&s[2]), &[]).unwrap().behind, 2);

        // A nested markdown file outside docs is still source.
        let nested = commit(&repo, &["prompts/develop.md"]);
        fetch_source(&cfg, &rt).unwrap();
        assert!(assess(&main, &m, Some(&tip), &[]).unwrap().outdated);

        // Ahead of the channel, on another branch, or moving to a bad build: no offer.
        assert!(
            !assess(&Channel::Branch("side".into()), &m, Some(&s[2]), &[])
                .unwrap()
                .outdated
        );
        assert!(
            !assess(
                &Channel::Main,
                &m,
                Some(&tip),
                std::slice::from_ref(&nested)
            )
            .unwrap()
            .outdated
        );
    }

    #[test]
    fn a_monitor_is_out_of_date_only_when_its_own_source_changed() {
        let (d, rt, cfg, s) = source();
        let repo = d.path().join("src");
        let latest = |c: &Check, k: &str| {
            c.monitors
                .iter()
                .find(|(n, _)| n == k)
                .map(|(_, l)| l.clone())
                .unwrap()
        };
        let swift = commit(&repo, &["clients/swift/App.swift"]);
        let compose = commit(&repo, &["clients/compose/Main.kt"]);
        let schema = commit(&repo, &["schemas/mason.proto"]);
        let _unrelated = commit(&repo, &["crates/daemon/x.rs", "README.md"]);
        fetch_source(&cfg, &rt).unwrap();
        let c = assess(&Channel::Main, &rt.mirror(), Some(&s[0]), &[]).unwrap();
        assert_eq!(latest(&c, "swift"), swift, "generated Swift is checked in");
        assert_eq!(
            latest(&c, "compose"),
            schema,
            "Wire reads the schema at build time"
        );
        assert_ne!(compose, schema);
    }

    fn check(outdated: bool) -> Check {
        Check {
            channel: "branch:develop".into(),
            latest: "bbbbbbbbbbbb".into(),
            behind: 3,
            outdated,
            ..Default::default()
        }
    }

    #[test]
    fn the_banner_says_where_an_update_is_and_when_the_button_is_live() {
        let idle = Status {
            phase: "idle".into(),
            ..Default::default()
        };
        assert_eq!(
            daemon_banner(&check(true), "aaaaaaaaaaaa", &idle),
            (
                "masond is 3 commits behind develop (aaaaaaaa → bbbbbbbb).".to_string(),
                true
            )
        );
        assert_eq!(
            daemon_banner(&check(false), "a", &idle),
            (String::new(), false)
        );
        for phase in ["fetching", "building", "testing", "taking over"] {
            let s = Status {
                phase: phase.into(),
                ..Default::default()
            };
            let (text, live) = daemon_banner(&check(true), "a", &s);
            assert!(
                text.starts_with("Updating masond") && !live,
                "{phase}: {text}"
            );
        }
        let failed = Status {
            phase: "failed".into(),
            error: Some("`cargo build` failed:\nlots\nof lines".into()),
            ..Default::default()
        };
        let (text, live) = daemon_banner(&check(true), "a", &failed);
        assert!(
            text.contains("`cargo build` failed:") && !text.contains("lots") && live,
            "{text}"
        );
        assert_eq!(
            daemon_banner(&check(false), "a", &failed),
            (String::new(), false)
        );
    }

    #[test]
    fn a_monitor_build_is_complete_or_absent_and_gets_its_commit() {
        let (_d, rt, mut cfg, s) = source();
        let kind = monitor_kind("swift").unwrap();
        cfg.monitor_swift_build =
            Some("mkdir \"$MASON_MONITOR_OUT/MASON.app\" && echo \"$MASON_BUILD_SHA\" > \"$MASON_MONITOR_OUT/MASON.app/stamp\"".into());
        let app = build_monitor(&cfg, &rt, kind, &s[0]).unwrap();
        assert!(app.ends_with("MASON.app"));
        assert_eq!(
            std::fs::read_to_string(app.join("stamp")).unwrap().trim(),
            s[0]
        );
        assert!(!rt
            .root
            .join("src")
            .join(format!("monitor-swift-{}", s[0]))
            .exists());
        // A finished build is not repeated.
        cfg.monitor_swift_build = Some("exit 1".into());
        assert_eq!(build_monitor(&cfg, &rt, kind, &s[0]).unwrap(), app);
        // A failing build, or one that makes nothing, leaves nothing that looks complete.
        let e = build_monitor(&cfg, &rt, kind, &s[1]).unwrap_err();
        assert!(e.contains("failed"), "{e}");
        cfg.monitor_swift_build = Some("true".into());
        let e = build_monitor(&cfg, &rt, kind, &s[1]).unwrap_err();
        assert!(e.contains("did not produce"), "{e}");
        assert!(!rt.monitors().join(format!("swift-{}", s[1])).exists());
    }

    #[test]
    fn old_monitor_builds_are_removed_but_the_one_before_stays() {
        let (_d, rt, mut cfg, s) = source();
        let kind = monitor_kind("swift").unwrap();
        cfg.monitor_swift_build = Some("mkdir \"$MASON_MONITOR_OUT/MASON.app\"".into());
        for sha in &s[..3] {
            build_monitor(&cfg, &rt, kind, sha).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let there = |sha: &str| rt.monitors().join(format!("swift-{sha}")).exists();
        assert!(!there(&s[0]) && there(&s[1]) && there(&s[2]));
    }

    #[test]
    fn a_build_lands_in_its_own_directory_and_is_complete_or_absent() {
        let (_d, rt, mut cfg, s) = source();
        cfg.build = "mkdir -p target/release && cp a target/release/masond && cp a target/release/step-host && cp a target/release/mason-mcp".into();
        build(&cfg, &rt, &s[0]).unwrap();
        assert!(rt.is_ready(&s[0]) && rt.masond(&s[0]).is_file());
        assert_eq!(
            std::fs::read_to_string(rt.dir(&s[0]).join("SHA")).unwrap(),
            s[0]
        );
        assert!(
            !rt.root.join("src").join(&s[0]).exists(),
            "the scratch copy is gone"
        );
        // A failing build leaves nothing that looks complete.
        cfg.build = "echo broken >&2; exit 3".into();
        let e = build(&cfg, &rt, &s[1]).unwrap_err();
        assert!(e.contains("broken"), "{e}");
        assert!(!rt.is_ready(&s[1]) && !rt.dir(&s[1]).exists());
        // A build that does not produce the daemon is refused.
        cfg.build = "mkdir -p target/release && cp a target/release/step-host".into();
        assert!(build(&cfg, &rt, &s[2])
            .unwrap_err()
            .contains("did not produce"));
        // Building what is built is a no-op.
        cfg.build = "exit 1".into();
        build(&cfg, &rt, &s[0]).unwrap();
    }
}
