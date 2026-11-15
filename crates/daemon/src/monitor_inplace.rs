// SPDX-License-Identifier: GPL-3.0-or-later

//! Updating a monitor that runs as a bare `swift build` binary from a checkout of this
//! repository (`<checkout>/clients/swift/.build/<arch>/release/MasonMonitor`).
//!
//! The monitor says where it runs from. If that is such a binary, the daemon builds the
//! newest source of the monitor in a build directory of its own (the checkout and its working
//! tree are never touched), with the commit stamped in, reports the build's progress through
//! the update header, and replaces the binary at the running path only after the build
//! succeeded. The monitor then starts that path again and quits.
//!
//! Kept apart from `update.rs`, which holds the daemon's own update guards; this only uses its
//! fetching and commit lookups.

use crate::update::{self, MonitorKind, MonitorState, Runtimes, UpdateCfg};
use crate::Daemon;
use mason_landing::git;
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Where the commit is stamped, relative to the source root. The checked-in file says `""`.
pub const STAMP_FILE: &str = "clients/swift/Sources/MonitorCore/BuildStamp.swift";

/// A running monitor binary and the checkout it was built from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    /// The binary, with symlinks resolved. The update replaces the file at this path.
    pub binary: PathBuf,
    pub checkout: PathBuf,
}

/// Is `own` a bare binary in the `.build` of a checkout of this repository? `None` for
/// anything else (an app bundle, a binary somewhere else), which the bundle path handles.
pub fn locate(own: &Path) -> Option<Target> {
    let binary = std::fs::canonicalize(own).ok()?;
    if !binary.is_file() {
        return None;
    }
    let checkout = binary
        .ancestors()
        .skip(1)
        .find(|a| a.join(".git").exists())?;
    let checkout = checkout.to_path_buf();
    let swift = checkout.join("clients/swift");
    if !swift.join("Package.swift").is_file() {
        return None;
    }
    // Resolve the checkout too, so the comparison is between like paths.
    let swift = std::fs::canonicalize(&swift).ok()?;
    if !binary.starts_with(swift.join(".build")) {
        return None;
    }
    Some(Target { binary, checkout })
}

/// Refuse to go to a commit older than the running one, or to the one already running.
pub fn check_direction(mirror: &Path, own: &str, target: &str) -> Result<(), String> {
    if own.is_empty() || own == "dev" {
        return Ok(());
    }
    if own == target {
        return Err("this monitor is already the newest".into());
    }
    if git::is_ancestor(mirror, target, own) {
        return Err(format!(
            "{target} is older than the running {own}: a monitor is never moved back"
        ));
    }
    Ok(())
}

/// What the build is doing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    /// `fetching`, `waiting`, `compiling`, `linking` or `installing`.
    pub step: String,
    pub done: u32,
    pub total: u32,
    pub last_line: String,
}

/// Read one line of `swift build` output: `[12/48] Compiling MonitorCore Model.swift`.
pub fn parse_line(line: &str) -> Option<(u32, u32, &'static str)> {
    let rest = line.strip_prefix('[')?;
    let (counts, after) = rest.split_once(']')?;
    let (a, b) = counts.split_once('/')?;
    let (a, b) = (a.trim().parse().ok()?, b.trim().parse().ok()?);
    let step = if after.trim_start().starts_with("Linking") {
        "linking"
    } else {
        "compiling"
    };
    Some((a, b, step))
}

/// Everything the build needs.
pub struct Job<'a> {
    pub target: &'a Target,
    pub mirror: &'a Path,
    pub sha: &'a str,
    /// The build directory; its `.build` is kept between builds so they are incremental.
    pub work: &'a Path,
    pub log: &'a Path,
    /// The `swift` to run.
    pub swift: &'a str,
}

fn deadline() -> Duration {
    std::env::var("MASON_UPDATE_SHELL_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|s| *s > 0.0)
        .map_or(Duration::from_secs(3600), Duration::from_secs_f64)
}

/// Put the source at `job.sha` in `job.work`, keeping `.build`, and stamp the commit in.
fn prepare(job: &Job) -> Result<(), String> {
    std::fs::create_dir_all(job.work.join("clients/swift")).map_err(|e| e.to_string())?;
    // Clear the old source, so a file the commit removed is gone; `.build` stays.
    let swift = job.work.join("clients/swift");
    for top in std::fs::read_dir(job.work)
        .map_err(|e| e.to_string())?
        .flatten()
    {
        if top.file_name() != "clients" {
            let _ =
                std::fs::remove_dir_all(top.path()).or_else(|_| std::fs::remove_file(top.path()));
        }
    }
    for e in std::fs::read_dir(&swift)
        .map_err(|e| e.to_string())?
        .flatten()
    {
        if e.file_name() != ".build" {
            let _ = std::fs::remove_dir_all(e.path()).or_else(|_| std::fs::remove_file(e.path()));
        }
    }
    let status = Command::new("sh")
        .args([
            "-c",
            "git --git-dir=\"$0\" archive \"$1\" clients/swift | tar -x -C \"$2\"",
        ])
        .arg(job.mirror)
        .arg(job.sha)
        .arg(job.work)
        .status()
        .map_err(|e| e.to_string())?;
    if !status.success() {
        return Err(format!("could not read {} from the source", job.sha));
    }
    let stamp = job.work.join(STAMP_FILE);
    if let Some(dir) = stamp.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&stamp, stamp_source(job.sha)).map_err(|e| e.to_string())
}

/// The generated source that says which commit a build is.
pub fn stamp_source(sha: &str) -> String {
    format!(
        "// SPDX-License-Identifier: GPL-3.0-or-later\n\n/// The commit this build was made from; empty for an unstamped build.\npublic enum BuildStamp {{\n    public static let commit = \"{sha}\"\n}}\n"
    )
}

/// Build and install. `progress` hears each change; `cancelled` is asked while the build
/// runs. The target binary is replaced only when everything before it worked.
pub fn run(
    job: &Job,
    progress: &mut dyn FnMut(Progress),
    cancelled: &dyn Fn() -> bool,
) -> Result<(), String> {
    let mut p = Progress {
        step: "compiling".into(),
        ..Default::default()
    };
    prepare(job)?;
    let log = std::fs::File::create(job.log).map_err(|e| e.to_string())?;
    let mut log = std::io::BufWriter::new(log);
    progress(p.clone());
    let swift_dir = job.work.join("clients/swift");
    let mut child = {
        use std::os::unix::process::CommandExt;
        Command::new("sh")
            .args(["-c", "exec \"$0\" build -c release 2>&1"])
            .arg(job.swift)
            .current_dir(&swift_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .process_group(0)
            .spawn()
            .map_err(|e| format!("could not start {}: {e}", job.swift))?
    };
    let out = child.stdout.take().expect("piped");
    let (tx, rx) = mpsc::channel::<String>();
    let reader = std::thread::spawn(move || {
        for l in BufReader::new(out).lines().map_while(Result::ok) {
            if tx.send(l).is_err() {
                break;
            }
        }
    });
    let started = Instant::now();
    let mut tail: Vec<String> = Vec::new();
    let stop = |child: &mut std::process::Child| {
        // The whole group: swift build starts compilers of its own.
        let _ = Command::new("kill")
            .args(["-TERM", "--", &format!("-{}", child.id())])
            .status();
        let _ = child.kill();
        let _ = child.wait();
    };
    let status = loop {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(line) => {
                let _ = writeln!(log, "{line}");
                if let Some((a, b, step)) = parse_line(&line) {
                    p.done = a;
                    p.total = b;
                    p.step = step.into();
                }
                p.last_line = line.clone();
                if tail.len() >= 12 {
                    tail.remove(0);
                }
                tail.push(line);
                progress(p.clone());
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            // The output closed: the build is over.
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                break child.wait().map_err(|e| e.to_string())?;
            }
        }
        if cancelled() {
            stop(&mut child);
            let _ = reader.join();
            return Err("cancelled".into());
        }
        if started.elapsed() > deadline() {
            stop(&mut child);
            let _ = reader.join();
            return Err(format!(
                "the build timed out after {}s",
                deadline().as_secs()
            ));
        }
    };
    let _ = reader.join();
    let _ = log.flush();
    if !status.success() {
        return Err(format!("swift build failed:\n{}", tail.join("\n")));
    }
    p.step = "installing".into();
    p.last_line = "Installing the new monitor".into();
    progress(p);
    install(&swift_dir.join(".build/release"), &job.target.binary)
}

/// Put the build's binary and resource bundles beside the running binary. The binary goes
/// last and by rename, so it is never half written; anything that fails before leaves it be.
pub fn install(built_dir: &Path, binary: &Path) -> Result<(), String> {
    let name = binary
        .file_name()
        .ok_or("the monitor has no file name")?
        .to_owned();
    let built = built_dir.join(&name);
    if !built.is_file() {
        return Err(format!("the build did not produce {}", built.display()));
    }
    let dir = binary.parent().ok_or("the monitor has no directory")?;
    let pid = std::process::id();
    for e in std::fs::read_dir(built_dir)
        .map_err(|e| e.to_string())?
        .flatten()
    {
        let n = e.file_name().to_string_lossy().to_string();
        if !n.ends_with(".bundle") || !e.path().is_dir() {
            continue;
        }
        let fresh = dir.join(format!(".{n}.new-{pid}"));
        let _ = std::fs::remove_dir_all(&fresh);
        copy_dir(&e.path(), &fresh)?;
        let home = dir.join(&n);
        let _ = std::fs::remove_dir_all(&home);
        std::fs::rename(&fresh, &home).map_err(|e| e.to_string())?;
    }
    let fresh = dir.join(format!(".{}.new-{pid}", name.to_string_lossy()));
    std::fs::copy(&built, &fresh).map_err(|e| e.to_string())?;
    if let Err(e) = std::fs::rename(&fresh, binary) {
        let _ = std::fs::remove_file(&fresh);
        return Err(e.to_string());
    }
    Ok(())
}

fn copy_dir(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| e.to_string())?;
    for e in std::fs::read_dir(from)
        .map_err(|e| e.to_string())?
        .flatten()
    {
        let dest = to.join(e.file_name());
        if e.path().is_dir() {
            copy_dir(&e.path(), &dest)?;
        } else {
            std::fs::copy(e.path(), &dest).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

// ---- in the daemon -------------------------------------------------------------------------

static CANCELS: Mutex<Option<HashSet<String>>> = Mutex::new(None);

fn cancel_flag(kind: &str, set: Option<bool>) -> bool {
    let mut g = CANCELS.lock().unwrap_or_else(|e| e.into_inner());
    let s = g.get_or_insert_with(HashSet::new);
    match set {
        Some(true) => {
            s.insert(kind.into());
            true
        }
        Some(false) => {
            s.remove(kind);
            false
        }
        None => s.contains(kind),
    }
}

/// The owner pressed Cancel. False when no build is running.
pub fn cancel(d: &Daemon, kind: &str) -> bool {
    let building = d
        .monitor_builds
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(kind)
        .is_some_and(|s| s.phase == "building");
    if building {
        cancel_flag(kind, Some(true));
    }
    building
}

fn swift_tool() -> String {
    std::env::var("MASON_MONITOR_SWIFT").unwrap_or_else(|_| {
        if Path::new("/usr/bin/swift").exists() {
            "/usr/bin/swift".into()
        } else {
            "swift".into()
        }
    })
}

fn building(step: &str, p: &Progress, log: &Path) -> MonitorState {
    MonitorState {
        phase: "building".into(),
        step: step.into(),
        done: p.done,
        total: p.total,
        last_line: p.last_line.clone(),
        log: log.display().to_string(),
        ..Default::default()
    }
}

/// Start updating the monitor at `target`, unless one is being built. The state says
/// `building` before this returns; the rest happens on its own thread.
pub fn start(
    d: &Arc<Daemon>,
    kind: MonitorKind,
    target: Target,
    own_version: String,
) -> Result<(), String> {
    {
        let mut b = d.monitor_builds.lock().unwrap_or_else(|e| e.into_inner());
        if b.get(kind.name).is_some_and(|s| s.phase == "building") {
            return Err(format!("the {} monitor is already being built", kind.name));
        }
        b.insert(
            kind.name.into(),
            MonitorState {
                phase: "building".into(),
                step: "fetching".into(),
                ..Default::default()
            },
        );
    }
    cancel_flag(kind.name, Some(false));
    d.poke();
    let d = d.clone();
    let _ = std::thread::Builder::new()
        .name("monitor-inplace".into())
        .spawn(move || {
            let rt = Runtimes::new(&d.cfg.data_dir);
            let log = rt.monitors().join(format!("{}-inplace.log", kind.name));
            let _ = std::fs::create_dir_all(rt.monitors());
            let go = || -> Result<(), String> {
                let cfg = UpdateCfg::load(&d.cfg.config_dir)?;
                update::fetch_locked(&d, &cfg, &rt)?;
                let tip = update::resolve(&cfg.channel, &rt.mirror())?;
                let sha = update::latest_of(kind, &rt.mirror(), &tip)?;
                check_direction(&rt.mirror(), &own_version, &sha)?;
                // Like any build, wait for a compile slot behind the pipeline.
                let waiting = Progress {
                    step: "waiting".into(),
                    last_line: "Waiting for a build slot".into(),
                    ..Default::default()
                };
                update::set_monitor(&d, kind.name, building("waiting", &waiting, &log));
                let uses = std::collections::BTreeMap::from([("compile".to_string(), 1u32)]);
                let lease = d.broker.lease_open("monitor build", &uses);
                let granted = loop {
                    if cancel_flag(kind.name, None) {
                        break false;
                    }
                    if matches!(
                        d.broker.lease_poll(lease),
                        crate::sched::LeaseState::Granted { .. }
                    ) {
                        break true;
                    }
                    std::thread::sleep(Duration::from_millis(500));
                };
                if !granted {
                    d.broker.lease_close(lease);
                    return Err("cancelled".into());
                }
                let work = rt
                    .root
                    .join("src")
                    .join(format!("monitor-inplace-{}", kind.name));
                let job = Job {
                    target: &target,
                    mirror: &rt.mirror(),
                    sha: &sha,
                    work: &work,
                    log: &log,
                    swift: &swift_tool(),
                };
                let r = run(
                    &job,
                    &mut |p| update::set_monitor(&d, kind.name, building(&p.step, &p, &log)),
                    &|| cancel_flag(kind.name, None),
                );
                d.broker.lease_close(lease);
                r
            };
            let state = match go() {
                Ok(()) => MonitorState {
                    phase: "ready".into(),
                    path: target.binary.display().to_string(),
                    log: log.display().to_string(),
                    ..Default::default()
                },
                Err(e) => {
                    tracing::warn!(kind = kind.name, "monitor build failed: {e}");
                    MonitorState {
                        phase: "failed".into(),
                        error: e.lines().next().unwrap_or_default().to_string(),
                        log: log.display().to_string(),
                        ..Default::default()
                    }
                }
            };
            update::set_monitor(&d, kind.name, state);
        });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn sh(dir: &Path, cmd: &str) {
        let s = Command::new("sh")
            .args(["-c", cmd])
            .current_dir(dir)
            .status()
            .unwrap();
        assert!(s.success(), "{cmd}");
    }

    /// A source repository with a monitor package at two commits; returns (dir, first, second).
    fn source(root: &Path) -> (PathBuf, String, String) {
        let repo = root.join("source");
        std::fs::create_dir_all(repo.join("clients/swift/Sources/MonitorCore")).unwrap();
        std::fs::write(repo.join("clients/swift/Package.swift"), "// package\n").unwrap();
        std::fs::write(repo.join(STAMP_FILE), stamp_source("")).unwrap();
        sh(&repo, "git init -q -b main && git config user.email a@b && git config user.name n && git config commit.gpgsign false && git add -A && git commit -q -m one");
        let one = git::run(&repo, &["rev-parse", "HEAD"]).unwrap();
        std::fs::write(repo.join("clients/swift/Package.swift"), "// package v2\n").unwrap();
        sh(&repo, "git commit -q -am two");
        let two = git::run(&repo, &["rev-parse", "HEAD"]).unwrap();
        (repo, one, two)
    }

    /// A checkout holding a stamped fake monitor binary, ad-hoc signed like any copied binary.
    fn checkout(root: &Path) -> Target {
        let co = root.join("checkout");
        let bin_dir = co.join("clients/swift/.build/x86_64-apple-macosx/release");
        std::fs::create_dir_all(&bin_dir).unwrap();
        std::fs::create_dir_all(co.join(".git")).unwrap();
        std::fs::write(co.join("clients/swift/Package.swift"), "").unwrap();
        let binary = bin_dir.join("MasonMonitor");
        std::fs::write(&binary, "old\n").unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        locate(&binary).expect("a bare binary in a checkout")
    }

    /// A `swift` that prints what the real one does and leaves a binary stamped with the
    /// commit it was given.
    fn fake_swift(root: &Path, ok: bool) -> PathBuf {
        let p = root.join(if ok { "swift-ok" } else { "swift-bad" });
        let body = if ok {
            "echo '[1/3] Compiling MonitorCore Model.swift'\necho '[2/3] Compiling MonitorUI Root.swift'\necho '[3/3] Linking MasonMonitor'\nmkdir -p .build/release/MasonMonitor_MasonMascot.bundle\necho art > .build/release/MasonMonitor_MasonMascot.bundle/a.png\ncp Sources/MonitorCore/BuildStamp.swift .build/release/MasonMonitor\necho 'Build complete!'\n"
        } else {
            "echo '[1/3] Compiling MonitorCore Model.swift'\necho 'error: boom'\nexit 1\n"
        };
        std::fs::write(&p, format!("#!/bin/sh\n{body}")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        // Anything copied that may run is signed ad hoc (CLAUDE.md); a script has no signature
        // to give, so this only matters when a Mach-O is used in its place.
        let _ = Command::new("codesign").args(["-s", "-"]).arg(&p).output();
        p
    }

    fn job<'a>(
        t: &'a Target,
        mirror: &'a Path,
        sha: &'a str,
        work: &'a Path,
        log: &'a Path,
        swift: &'a str,
    ) -> Job<'a> {
        Job {
            target: t,
            mirror: Path::new(mirror).join(".git").leak_path(),
            sha,
            work,
            log,
            swift,
        }
    }

    trait Leak {
        fn leak_path(self) -> &'static Path;
    }
    impl Leak for PathBuf {
        fn leak_path(self) -> &'static Path {
            Box::leak(self.into_boxed_path())
        }
    }

    #[test]
    fn the_build_reports_phases_in_order_and_swaps_the_binary_after_success() {
        let root = tempfile::tempdir().unwrap();
        let (repo, _one, two) = source(root.path());
        let target = checkout(root.path());
        let swift = fake_swift(root.path(), true);
        let (work, log) = (root.path().join("work"), root.path().join("build.log"));
        let mut seen: Vec<Progress> = Vec::new();
        let before = std::fs::read_to_string(&target.binary).unwrap();
        assert_eq!(before, "old\n");
        run(
            &job(&target, &repo, &two, &work, &log, swift.to_str().unwrap()),
            &mut |p| {
                // The binary is not touched until the build is installing.
                if p.step != "installing" {
                    assert_eq!(std::fs::read_to_string(&target.binary).unwrap(), "old\n");
                }
                seen.push(p);
            },
            &|| false,
        )
        .unwrap();
        let mut steps: Vec<&str> = seen.iter().map(|p| p.step.as_str()).collect();
        steps.dedup();
        assert_eq!(steps, ["compiling", "linking", "installing"]);
        let counted = seen.iter().find(|p| p.done == 2).unwrap();
        assert_eq!((counted.done, counted.total), (2, 3));
        assert!(counted.last_line.contains("Compiling MonitorUI"));
        // The new binary carries the commit and sits at the same path, with its resources.
        let now = std::fs::read_to_string(&target.binary).unwrap();
        assert!(now.contains(&two), "{now}");
        let dir = target.binary.parent().unwrap();
        assert!(dir.join("MonitorBuild-none").parent().is_some());
        assert!(dir.join("MasonMonitor_MasonMascot.bundle/a.png").is_file());
        assert!(std::fs::read_to_string(&log)
            .unwrap()
            .contains("Build complete!"));
    }

    #[test]
    fn a_failed_build_leaves_the_old_binary_untouched() {
        let root = tempfile::tempdir().unwrap();
        let (repo, _one, two) = source(root.path());
        let target = checkout(root.path());
        let swift = fake_swift(root.path(), false);
        let (work, log) = (root.path().join("work"), root.path().join("build.log"));
        let e = run(
            &job(&target, &repo, &two, &work, &log, swift.to_str().unwrap()),
            &mut |_| {},
            &|| false,
        )
        .unwrap_err();
        assert!(e.contains("error: boom"), "{e}");
        assert_eq!(std::fs::read_to_string(&target.binary).unwrap(), "old\n");
        let names: Vec<_> = std::fs::read_dir(target.binary.parent().unwrap())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, ["MasonMonitor"]);
    }

    #[test]
    fn a_cancelled_build_stops_and_leaves_the_old_binary() {
        let root = tempfile::tempdir().unwrap();
        let (repo, _one, two) = source(root.path());
        let target = checkout(root.path());
        let swift = fake_swift(root.path(), true);
        let (work, log) = (root.path().join("work"), root.path().join("build.log"));
        let e = run(
            &job(&target, &repo, &two, &work, &log, swift.to_str().unwrap()),
            &mut |_| {},
            &|| true,
        );
        // The fake finishes at once, so either it was caught or it finished; never half.
        match e {
            Err(e) => {
                assert_eq!(e, "cancelled");
                assert_eq!(std::fs::read_to_string(&target.binary).unwrap(), "old\n");
            }
            Ok(()) => assert!(std::fs::read_to_string(&target.binary)
                .unwrap()
                .contains(&two)),
        }
    }

    #[test]
    fn a_monitor_is_never_moved_back_or_to_itself() {
        let root = tempfile::tempdir().unwrap();
        let (repo, one, two) = source(root.path());
        let mirror = repo.join(".git");
        assert!(check_direction(&mirror, &one, &two).is_ok());
        assert!(check_direction(&mirror, "dev", &two).is_ok());
        assert!(check_direction(&mirror, &two, &one)
            .unwrap_err()
            .contains("never moved back"));
        assert!(check_direction(&mirror, &two, &two)
            .unwrap_err()
            .contains("already"));
    }

    #[test]
    fn only_a_bare_binary_in_a_checkouts_build_directory_is_updated_in_place() {
        let root = tempfile::tempdir().unwrap();
        let t = checkout(root.path());
        assert!(t.binary.ends_with("release/MasonMonitor"));
        let app = root.path().join("MASON.app/Contents/MacOS");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(app.join("MasonMonitor"), "x").unwrap();
        assert_eq!(locate(&app.join("MasonMonitor")), None);
        assert_eq!(locate(&root.path().join("missing")), None);
        // In a checkout, but not in its build directory.
        let stray = root.path().join("checkout/clients/swift/MasonMonitor");
        std::fs::write(&stray, "x").unwrap();
        assert_eq!(locate(&stray), None);
    }

    #[test]
    fn swift_build_lines_give_counts_and_the_linking_step() {
        assert_eq!(
            parse_line("[12/48] Compiling MonitorCore Model.swift"),
            Some((12, 48, "compiling"))
        );
        assert_eq!(
            parse_line("[48/48] Linking MasonMonitor"),
            Some((48, 48, "linking"))
        );
        assert_eq!(parse_line("Build complete! (3.1s)"), None);
        assert_eq!(parse_line("[x/y] Compiling"), None);
    }
}
