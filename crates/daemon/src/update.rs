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
}

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
    let sh = |cwd: &Path, cmd: &str| -> Result<(), String> {
        let log = rt.root.join(format!("{sha}.log"));
        let out = Command::new("sh")
            .args(["-c", cmd])
            .current_dir(cwd)
            .output()
            .map_err(|e| e.to_string())?;
        let mut text = String::from_utf8_lossy(&out.stdout).to_string();
        text.push_str(&String::from_utf8_lossy(&out.stderr));
        let _ = std::fs::write(&log, &text);
        if out.status.success() {
            Ok(())
        } else {
            let tail: Vec<&str> = text.lines().rev().take(12).collect();
            Err(format!(
                "`{cmd}` failed:\n{}",
                tail.into_iter().rev().collect::<Vec<_>>().join("\n")
            ))
        }
    };
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
    /// `start` or `healthy`.
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
            "healthy" => open.clear(),
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
    /// `idle`, `fetching`, `building`, `testing`, `taking over`, `failed`.
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
}

fn say(d: &Daemon, phase: &str, line: &str) {
    let mut s = d.update_status.lock().unwrap_or_else(|e| e.into_inner());
    s.phase = phase.into();
    s.log.push(line.into());
    tracing::info!(phase, "{line}");
}

fn fail(d: &Daemon, why: String) -> String {
    let mut s = d.update_status.lock().unwrap_or_else(|e| e.into_inner());
    s.phase = "failed".into();
    s.error = Some(why.clone());
    s.log.push(format!("failed: {why}"));
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
            "fetching" | "building" | "testing" | "taking over"
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
        fetch_source(&cfg, &rt)?;
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

/// Replace this process with the new runtime, keeping its pid. The step hosts
/// are other processes and do not notice; the new daemon finds them as it
/// would after any restart. Only returns if the exec fails.
pub fn take_over(d: &Arc<Daemon>, sha: &str) -> String {
    use std::os::unix::process::CommandExt;
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
    let err = Command::new(rt.masond(sha)).args(&args).exec();
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
