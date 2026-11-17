// SPDX-License-Identifier: GPL-3.0-or-later

//! A task's build directory (`task_cache` in the project file, s78).
//!
//! The directory lives outside the worktree, so removing the worktree leaves it behind; at 4 to
//! 11 GB each they filled the owner's disk. When a task lands its directory is removed, and the
//! reconcile sweep removes the ones a restart or a miss left. Removal is refused for any path
//! that could take something else with it, and a refusal or a failure is logged, never fatal.

use crate::config::ProjectCfg;
use crate::daemon::Daemon;
use mason_core::*;
use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Check a `task_cache` template when the project file is read: it must name the task, so that
/// no two tasks share a directory.
pub fn check_template(t: &str) -> Result<(), String> {
    if t.trim().is_empty() {
        return Err("`task_cache` is empty".into());
    }
    if !t.contains("{task}") {
        return Err(
            "`task_cache` must contain `{task}`, so each task has its own directory".into(),
        );
    }
    Ok(())
}

/// Expand `$HOME`, `${HOME}`, other environment variables, a leading `~` and `{task}`.
/// A variable that is not set expands to nothing.
pub fn expand(template: &str, task: u64, home: &Path) -> String {
    let mut s = template.replace("{task}", &task.to_string());
    if s == "~" || s.starts_with("~/") {
        s = format!("{}{}", home.display(), &s[1..]);
    }
    let mut out = String::new();
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        let braced = it.peek() == Some(&'{');
        if braced {
            it.next();
        }
        let mut name = String::new();
        while let Some(&n) = it.peek() {
            if n.is_ascii_alphanumeric() || n == '_' {
                name.push(n);
                it.next();
            } else {
                break;
            }
        }
        if braced && it.peek() == Some(&'}') {
            it.next();
        }
        if name == "HOME" {
            out.push_str(&home.to_string_lossy());
        } else if let Ok(v) = std::env::var(&name) {
            out.push_str(&v);
        }
    }
    out
}

/// Why a directory is not removed.
#[derive(Debug, PartialEq)]
pub enum Refused {
    Empty,
    Relative,
    DotDot,
    Home,
    Protected(PathBuf),
    Symlink,
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refused::Empty => write!(f, "the path expands to nothing"),
            Refused::Relative => write!(f, "the path is not absolute"),
            Refused::DotDot => write!(f, "the path contains `..`"),
            Refused::Home => write!(f, "the path is the home directory or a parent of it"),
            Refused::Protected(p) => write!(f, "the path is or contains {}", p.display()),
            Refused::Symlink => write!(f, "the path is a symlink"),
        }
    }
}

/// The path rules. `keep` is every directory that must not be inside the one removed: the
/// project, the worktrees, the landing area.
pub fn check_path(expanded: &str, home: &Path, keep: &[PathBuf]) -> Result<PathBuf, Refused> {
    if expanded.trim().is_empty() {
        return Err(Refused::Empty);
    }
    let p = PathBuf::from(expanded);
    if p.components().any(|c| c == Component::ParentDir) {
        return Err(Refused::DotDot);
    }
    if !p.is_absolute() {
        return Err(Refused::Relative);
    }
    // `/`, `/Users` and `/Users/me` itself are all "home or a parent of it".
    if home.starts_with(&p) {
        return Err(Refused::Home);
    }
    for k in keep {
        if k.starts_with(&p) {
            return Err(Refused::Protected(k.clone()));
        }
    }
    if std::fs::symlink_metadata(&p).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(Refused::Symlink);
    }
    Ok(p)
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

fn keep_list(d: &Daemon, p: &ProjectCfg) -> Vec<PathBuf> {
    vec![
        p.path.clone(),
        d.cfg.worktrees_dir(),
        d.cfg.landing_dir(),
        d.cfg.data_dir.clone(),
    ]
}

/// The directory of `task` if the project names one and the rules allow it. `None` also when the
/// project has no `task_cache`.
fn target(d: &Daemon, p: &ProjectCfg, task: u64) -> Option<Result<PathBuf, Refused>> {
    let t = p.task_cache.as_ref()?;
    let h = home();
    Some(check_path(&expand(t, task, &h), &h, &keep_list(d, p)))
}

/// Objects younger than this are never pruned: a build may be writing them.
const OBJECT_GRACE_MS: u64 = 15 * 60 * 1000;

/// The crate-and-hash part of a codegen-unit object's name (`name-hash.cgu.rcgu.o`), or `None`
/// for any other file.
fn object_unit(file: &str) -> Option<&str> {
    let stem = file.strip_suffix(".rcgu.o")?;
    stem.split('.').next().filter(|u| !u.is_empty())
}

fn unit_crate(unit: &str) -> &str {
    unit.rsplit_once('-').map_or(unit, |(n, _)| n)
}

/// Which of `files` (name, modified ms) are stale codegen-unit objects. Cargo's macOS default
/// leaves each unit's `.rcgu.o` beside the executable because the debug info is read from it,
/// one more set per crate hash. For each crate the set with the newest object is the live one
/// and stays; sets of older hashes go, except any object modified within the grace period, and
/// none of a crate's older sets go while its newest set is itself within the grace period.
pub fn stale_objects(files: &[(String, u64)], now_ms: u64) -> Vec<String> {
    let mut newest: std::collections::BTreeMap<&str, (&str, u64)> = Default::default();
    for (f, t) in files {
        let Some(unit) = object_unit(f) else { continue };
        let e = newest.entry(unit_crate(unit)).or_insert((unit, *t));
        if *t > e.1 {
            *e = (unit, *t);
        }
    }
    files
        .iter()
        .filter(|(f, t)| {
            let Some(unit) = object_unit(f) else {
                return false;
            };
            // While a crate's newest set is still within the grace period a build may be
            // writing it, and the set before it is the last complete one: keep both until then.
            newest.get(unit_crate(unit)).is_some_and(|(live, live_t)| {
                *live != unit && now_ms.saturating_sub(*live_t) >= OBJECT_GRACE_MS
            }) && now_ms.saturating_sub(*t) >= OBJECT_GRACE_MS
        })
        .map(|(f, _)| f.clone())
        .collect()
}

/// Delete the stale codegen-unit objects in `<dir>/debug/deps` (see [`stale_objects`]). Called
/// before a suite build in a task's build directory. Returns the count removed.
pub fn prune_objects(dir: &Path, now_ms: u64) -> usize {
    let deps = dir.join("debug").join("deps");
    let Ok(rd) = std::fs::read_dir(&deps) else {
        return 0;
    };
    let files: Vec<(String, u64)> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            if !name.ends_with(".rcgu.o") {
                return None;
            }
            let m = e.metadata().ok()?.modified().ok()?;
            let ms = m.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis() as u64;
            Some((name, ms))
        })
        .collect();
    let mut n = 0;
    for f in stale_objects(&files, now_ms) {
        if std::fs::remove_file(deps.join(&f)).is_ok() {
            n += 1;
        }
    }
    n
}

/// Prune the build directory of `task` before one of its suite builds, if the project names one.
pub fn prune_task_objects(d: &Daemon, p: &ProjectCfg, task: u64, now_ms: u64) {
    if let Some(Ok(dir)) = target(d, p, task) {
        let n = prune_objects(&dir, now_ms);
        if n > 0 {
            eprintln!(
                "task {task}: pruned {n} stale object files in {}",
                dir.display()
            );
        }
    }
}

/// Remove `task`'s directory now, on this thread. Logged, never an error.
pub fn remove_now(d: &Daemon, p: &ProjectCfg, task: u64) -> bool {
    let path = match target(d, p, task) {
        None => return false,
        Some(Err(why)) => {
            eprintln!("task {task}: not removing its task_cache directory: {why}");
            return false;
        }
        Some(Ok(path)) => path,
    };
    if std::fs::symlink_metadata(&path).is_err() {
        return false; // nothing there
    }
    match std::fs::remove_dir_all(&path) {
        Ok(()) => true,
        Err(e) => {
            eprintln!("task {task}: removing {} failed: {e}", path.display());
            false
        }
    }
}

/// Removals queued or running, by thread name (`task-cache-7`, `own-cache-7`).
static BUSY: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());
/// Held by the removal that is running, so they run one at a time.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

/// Run `remove` on a thread of its own, outside any daemon lock (it can take minutes), unless a
/// removal of the same name is already queued or running, and never two at once.
///
/// On 2026-11-13 `task_cache` was set on a project with 90 landed tasks' directories (8 GB each).
/// The sweep started a thread for each, and every later pass started another for each one not
/// yet gone. Over a thousand threads unlinked files on one USB disk at once, and every build on
/// it stalled in the kernel. One at a time, a backlog takes longer to clear but stalls nothing.
fn in_background(name: String, remove: impl FnOnce() + Send + 'static) {
    if !BUSY
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(name.clone())
    {
        return;
    }
    let done = |key: &str| {
        BUSY.lock().unwrap_or_else(|e| e.into_inner()).remove(key);
    };
    let key = name.clone();
    let spawned = std::thread::Builder::new()
        .name(name.clone())
        .spawn(move || {
            {
                let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
                remove();
            }
            done(&key);
        });
    if spawned.is_err() {
        done(&name);
    }
}

/// Remove it in the background (see [`in_background`]).
pub fn remove_in_background(d: &Arc<Daemon>, p: &ProjectCfg, task: u64) {
    if p.task_cache.is_none() {
        return;
    }
    let (d, p) = (d.clone(), p.clone());
    in_background(format!("task-cache-{task}"), move || {
        remove_now(&d, &p, task);
    });
}

/// Every `task_cache` directory that exists for a task in `w`, for sizing.
pub fn dirs(d: &Daemon, w: &mason_engine::World) -> Vec<PathBuf> {
    w.tasks
        .values()
        .filter_map(|t| match target(d, d.project(t.project)?, t.id.0)? {
            Ok(p) if std::fs::symlink_metadata(&p).is_ok() => Some(p),
            _ => None,
        })
        .collect()
}

/// The directory MASON owns for `task` (`MASON_TASK_CACHE`), created now, as text for the
/// environment. Creation is best effort: a step that needs it will say so itself.
pub fn own_dir(d: &Daemon, task: u64) -> String {
    let p = d.cfg.task_cache_dir(task);
    let _ = std::fs::create_dir_all(&p);
    p.to_string_lossy().into_owned()
}

/// Remove the directory MASON owns for `task`. Only ever the path MASON built under its own
/// data directory, and never a symlink.
pub fn remove_own_now(d: &Daemon, task: u64) -> bool {
    let p = d.cfg.task_cache_dir(task);
    if !std::fs::symlink_metadata(&p).is_ok_and(|m| m.file_type().is_dir()) {
        return false;
    }
    match std::fs::remove_dir_all(&p) {
        Ok(()) => true,
        Err(e) => {
            eprintln!("task {task}: removing {} failed: {e}", p.display());
            false
        }
    }
}

/// Remove it in the background (see [`in_background`]).
pub fn remove_own_in_background(d: &Arc<Daemon>, task: u64) {
    let d = d.clone();
    in_background(format!("own-cache-{task}"), move || {
        remove_own_now(&d, task);
    });
}

/// The sweep's part for the owned directories: each one whose task landed or was canceled (or
/// failed more than the keep days ago) and has no job. Never one whose task is running.
pub fn sweep_own(d: &Arc<Daemon>, w: &mason_engine::World) -> usize {
    let Ok(rd) = std::fs::read_dir(d.cfg.data_dir.join("c")) else {
        return 0;
    };
    let now = crate::daemon::now_ms();
    let mut n = 0;
    for e in rd.flatten() {
        let Some(id) = e.file_name().to_str().and_then(|s| s.parse::<u64>().ok()) else {
            continue;
        };
        let Some(t) = w.tasks.get(&TaskId(id)) else {
            continue;
        };
        let keep_days = d
            .project(t.project)
            .and_then(|p| p.task_cache_keep_days)
            .unwrap_or(DEFAULT_KEEP_DAYS);
        let go = match t.state {
            State::Landed | State::Canceled => true,
            State::Failed => expired(t.state, last_step_ms(w, t.id), now, keep_days),
            _ => false,
        };
        if go && !d.has_job_for_task(t.id) {
            remove_own_in_background(d, id);
            n += 1;
        }
    }
    n
}

/// Days a canceled or failed task's directory is kept after its last step, unless the project
/// says otherwise (`task_cache_keep_days`).
pub const DEFAULT_KEEP_DAYS: u64 = 7;

/// Is a canceled or failed task's directory old enough to go? Counted from the task's last
/// step, and never while the task has a job (a revived task is running again).
pub fn expired(state: State, last_step_ms: u64, now_ms: u64, keep_days: u64) -> bool {
    matches!(state, State::Canceled | State::Failed)
        && now_ms.saturating_sub(last_step_ms) > keep_days.saturating_mul(86_400_000)
}

/// When the task last started a step (when it entered its state, if it never did).
pub fn last_step_ms(w: &mason_engine::World, task: TaskId) -> u64 {
    w.step_runs
        .values()
        .filter(|r| r.task == task)
        .map(|r| r.started_at.0)
        .max()
        .unwrap_or_else(|| w.tasks.get(&task).map_or(0, |t| t.state_entered_at.0))
}

/// The sweep's part: the directories of landed tasks that are still there and that no running
/// job uses, and those of canceled and failed tasks older than `task_cache_keep_days`. Returns
/// how many removals were started.
pub fn sweep(d: &Arc<Daemon>, w: &mason_engine::World) -> usize {
    let mut n = 0;
    let now = crate::daemon::now_ms();
    for t in w.tasks.values() {
        let Some(p) = d.project(t.project) else {
            continue;
        };
        let go = match t.state {
            State::Landed => true,
            State::Canceled | State::Failed => {
                let last = last_step_ms(w, t.id);
                expired(
                    t.state,
                    last,
                    now,
                    p.task_cache_keep_days.unwrap_or(DEFAULT_KEEP_DAYS),
                )
            }
            _ => false,
        };
        if !go || p.task_cache.is_none() || d.has_job_for_task(t.id) {
            continue;
        }
        if let Some(Ok(path)) = target(d, p, t.id.0) {
            if std::fs::symlink_metadata(&path).is_ok() {
                remove_in_background(d, p, t.id.0);
                n += 1;
            }
        }
    }
    n
}

/// First line of the `.cargo/config.toml` MASON writes, so a later run knows the file is its own.
const CARGO_CONFIG_MARK: &str = "# Written by MASON: this task's build directory. Not committed.";

/// The text of the cargo config that points `[build] target-dir` at `dir`. The path is a TOML
/// basic string, so `\` and `"` and control characters are escaped.
pub fn cargo_config_text(dir: &str) -> String {
    let mut q = String::new();
    for c in dir.chars() {
        match c {
            '\\' => q.push_str("\\\\"),
            '"' => q.push_str("\\\""),
            '\n' => q.push_str("\\n"),
            '\r' => q.push_str("\\r"),
            '\t' => q.push_str("\\t"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                q.push_str(&format!("\\u{:04X}", c as u32));
            }
            c => q.push(c),
        }
    }
    format!("{CARGO_CONFIG_MARK}\n[build]\ntarget-dir = \"{q}\"\n")
}

fn git_out(wt: &Path, args: &[&str]) -> Result<(bool, String), String> {
    let o = std::process::Command::new("git")
        .args(args)
        .current_dir(wt)
        .output()
        .map_err(|e| format!("git {}: {e}", args.join(" ")))?;
    Ok((
        o.status.success(),
        String::from_utf8_lossy(&o.stdout).trim().to_string(),
    ))
}

/// Point cargo in the checkout `wt` at `task`'s build directory by writing an untracked
/// `.cargo/config.toml` (a config-file `target-dir`, not `CARGO_TARGET_DIR`: sccache hashes every
/// `CARGO_*` variable) and listing it in the repository's `info/exclude`. Returns whether a file
/// was written. A repository that tracks its own `.cargo/config.toml` keeps it (`Ok(false)`).
pub fn write_cargo_config(
    wt: &Path,
    template: &str,
    task: u64,
    home: &Path,
    keep: &[PathBuf],
) -> Result<bool, String> {
    let dir = check_path(&expand(template, task, home), home, keep)
        .map_err(|why| format!("task_cache directory refused: {why}"))?;
    let rel = ".cargo/config.toml";
    let (tracked, _) = git_out(wt, &["ls-files", "--error-unmatch", "--", rel])?;
    if tracked {
        return Ok(false);
    }
    let file = wt.join(rel);
    if let Ok(old) = std::fs::read_to_string(&file) {
        if !old.starts_with(CARGO_CONFIG_MARK) {
            return Ok(false); // someone else's untracked file
        }
    }
    let (ok, ex) = git_out(wt, &["rev-parse", "--git-path", "info/exclude"])?;
    if !ok || ex.is_empty() {
        return Err("git would not say where info/exclude is".into());
    }
    let ex = wt.join(ex);
    let cur = std::fs::read_to_string(&ex).unwrap_or_default();
    if !cur.lines().any(|l| l.trim() == "/.cargo/config.toml") {
        if let Some(parent) = ex.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let sep = if cur.is_empty() || cur.ends_with('\n') {
            ""
        } else {
            "\n"
        };
        std::fs::write(&ex, format!("{cur}{sep}/.cargo/config.toml\n"))
            .map_err(|e| format!("{}: {e}", ex.display()))?;
    }
    std::fs::create_dir_all(wt.join(".cargo")).map_err(|e| e.to_string())?;
    std::fs::write(&file, cargo_config_text(&dir.to_string_lossy()))
        .map_err(|e| format!("{}: {e}", file.display()))?;
    Ok(true)
}

/// [`write_cargo_config`] for a task's worktree, when its project has a `task_cache`. Logged,
/// never fatal.
pub fn point_cargo_at_task_cache(d: &Daemon, p: &ProjectCfg, task: u64, wt: &Path) {
    let Some(t) = p.task_cache.as_ref() else {
        return;
    };
    match write_cargo_config(wt, t, task, &home(), &keep_list(d, p)) {
        Ok(true) => {}
        Ok(false) => eprintln!(
            "task {task}: {} has its own .cargo/config.toml; leaving it",
            wt.display()
        ),
        Err(e) => eprintln!(
            "task {task}: no .cargo/config.toml in {}: {e}",
            wt.display()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> PathBuf {
        PathBuf::from("/Users/me")
    }
    fn keep() -> Vec<PathBuf> {
        vec![
            PathBuf::from("/Users/me/git/proj"),
            PathBuf::from("/Users/me/Library/mason/w"),
        ]
    }
    fn chk(s: &str) -> Result<PathBuf, Refused> {
        check_path(s, &home(), &keep())
    }

    #[test]
    fn only_objects_of_older_hashes_are_pruned_and_a_running_build_keeps_its_files() {
        let now = 10_000_000_000u64;
        let old = now - 2 * OBJECT_GRACE_MS;
        let f = |n: &str, t: u64| (n.to_string(), t);
        let files = vec![
            f("mason_daemon-aaa.x-cgu.0.rcgu.o", old),
            f("mason_daemon-aaa.x-cgu.1.rcgu.o", old),
            f("mason_daemon-bbb.x-cgu.0.rcgu.o", old + 5),
            f("mason_core-ccc.y-cgu.0.rcgu.o", old),
            // an older set that is still being written, and a file that is no object
            f("mason_core-ddd.y-cgu.0.rcgu.o", now - 1000),
            f("libmason_core-ccc.rlib", old - 9),
        ];
        let mut gone = stale_objects(&files, now);
        gone.sort();
        assert_eq!(
            gone,
            vec![
                "mason_daemon-aaa.x-cgu.0.rcgu.o".to_string(),
                "mason_daemon-aaa.x-cgu.1.rcgu.o".to_string(),
            ]
        );
    }

    #[test]
    fn pruning_a_directory_removes_only_object_files() {
        let t = tempfile::tempdir().unwrap();
        let deps = t.path().join("debug/deps");
        std::fs::create_dir_all(&deps).unwrap();
        for n in ["k-1.a.rcgu.o", "k-2.a.rcgu.o", "libk-1.rlib"] {
            std::fs::write(deps.join(n), b"x").unwrap();
        }
        let n = prune_objects(t.path(), u64::MAX / 4);
        assert!(n <= 1);
        assert!(deps.join("libk-1.rlib").exists());
        assert_eq!(prune_objects(&t.path().join("none"), 0), 0);
    }

    #[test]
    fn a_good_path_passes() {
        let e = expand("$HOME/.cache/mason-target/{task}", 7, &home());
        assert_eq!(e, "/Users/me/.cache/mason-target/7");
        assert_eq!(chk(&e), Ok(PathBuf::from(&e)));
        assert_eq!(expand("${HOME}/c/{task}", 3, &home()), "/Users/me/c/3");
        assert_eq!(expand("~/c/{task}", 3, &home()), "/Users/me/c/3");
    }

    #[test]
    fn a_path_that_expands_to_nothing_is_refused() {
        assert_eq!(chk(""), Err(Refused::Empty));
        assert_eq!(chk("  "), Err(Refused::Empty));
        let e = expand("$MASON_SURELY_UNSET_VARIABLE", 1, &home());
        assert_eq!(chk(&e), Err(Refused::Empty));
    }

    #[test]
    fn a_path_with_dot_dot_is_refused() {
        assert_eq!(chk("/Users/me/.cache/../x"), Err(Refused::DotDot));
        assert_eq!(chk("/Users/me/.cache/.."), Err(Refused::DotDot));
    }

    #[test]
    fn a_relative_path_is_refused() {
        assert_eq!(chk("cache/7"), Err(Refused::Relative));
    }

    #[test]
    fn the_home_directory_and_its_parents_are_refused() {
        for p in ["/Users/me", "/Users", "/"] {
            assert_eq!(chk(p), Err(Refused::Home), "{p}");
        }
    }

    #[test]
    fn the_project_or_a_parent_of_it_is_refused() {
        assert!(matches!(
            chk("/Users/me/git/proj"),
            Err(Refused::Protected(_))
        ));
        assert!(matches!(chk("/Users/me/git"), Err(Refused::Protected(_))));
    }

    #[test]
    fn a_worktree_or_a_parent_of_one_is_refused() {
        assert!(matches!(
            chk("/Users/me/Library/mason/w"),
            Err(Refused::Protected(_))
        ));
        assert!(matches!(
            chk("/Users/me/Library/mason"),
            Err(Refused::Protected(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_is_refused() {
        let t = tempfile::tempdir().unwrap();
        let real = t.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let link = t.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let r = check_path(link.to_str().unwrap(), &home(), &[]);
        assert_eq!(r, Err(Refused::Symlink));
        assert!(check_path(real.to_str().unwrap(), &home(), &[]).is_ok());
    }

    #[test]
    fn a_canceled_or_failed_directory_goes_only_after_the_keep_days() {
        let day = 86_400_000;
        let now = 100 * day;
        for s in [State::Canceled, State::Failed] {
            assert!(expired(s, now - 8 * day, now, 7), "{s:?} older than 7 days");
            assert!(!expired(s, now - 6 * day, now, 7), "{s:?} younger");
            assert!(
                !expired(s, now - 7 * day, now, 7),
                "{s:?} exactly at the limit"
            );
        }
        // A task that is alive again, or landed (handled elsewhere), is never "expired".
        for s in [State::Queued, State::Developing, State::Landed] {
            assert!(!expired(s, 0, now, 7), "{s:?}");
        }
    }

    #[test]
    fn the_template_must_name_the_task() {
        assert!(check_template("$HOME/c/{task}").is_ok());
        assert!(check_template("$HOME/c").is_err());
        assert!(check_template("").is_err());
    }

    /// A removal already queued or running is not started again, and removals never overlap.
    /// Ordered by channels, not by time.
    #[test]
    fn a_removal_is_not_started_twice_and_they_run_one_at_a_time() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::mpsc::channel;
        static RAN: AtomicUsize = AtomicUsize::new(0);
        static RUNNING: AtomicUsize = AtomicUsize::new(0);
        static MOST: AtomicUsize = AtomicUsize::new(0);
        let work = || {
            let now = RUNNING.fetch_add(1, Ordering::SeqCst) + 1;
            MOST.fetch_max(now, Ordering::SeqCst);
            RAN.fetch_add(1, Ordering::SeqCst);
            RUNNING.fetch_sub(1, Ordering::SeqCst);
        };
        // The first holds its slot until told to go.
        let (go_tx, go_rx) = channel::<()>();
        let (started_tx, started_rx) = channel::<()>();
        let (end_tx, end_rx) = channel::<()>();
        in_background("test-dedupe-a".into(), move || {
            started_tx.send(()).unwrap();
            go_rx.recv().unwrap();
            work();
        });
        started_rx.recv().unwrap();
        // Same name while it runs: dropped. Another name: queued behind it.
        in_background("test-dedupe-a".into(), work);
        in_background("test-dedupe-b".into(), move || {
            work();
            end_tx.send(()).unwrap();
        });
        go_tx.send(()).unwrap();
        end_rx.recv().unwrap();
        assert_eq!(RAN.load(Ordering::SeqCst), 2);
        assert_eq!(MOST.load(Ordering::SeqCst), 1);
    }

    fn repo() -> PathBuf {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let d = std::env::temp_dir().join(format!(
            "mason-cc-{}-{}",
            std::process::id(),
            N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&d).unwrap();
        for a in [
            &["init", "-q"][..],
            &["config", "user.email", "t@t"],
            &["config", "user.name", "t"],
            &["commit", "-q", "--allow-empty", "-m", "x"],
        ] {
            assert!(git_out(&d, a).unwrap().0);
        }
        d
    }

    #[test]
    fn the_cargo_config_text_escapes_the_path() {
        let t = cargo_config_text("/a b/\"q\"/c\\d");
        assert!(t.contains("target-dir = \"/a b/\\\"q\\\"/c\\\\d\""), "{t}");
    }

    #[test]
    fn a_task_worktree_gets_an_untracked_cargo_config() {
        let wt = repo();
        let h = home();
        assert_eq!(
            write_cargo_config(&wt, "$HOME/.cache/t/{task}", 9, &h, &keep()),
            Ok(true)
        );
        let t = std::fs::read_to_string(wt.join(".cargo/config.toml")).unwrap();
        assert!(t.contains("target-dir = \"/Users/me/.cache/t/9\""), "{t}");
        assert_eq!(git_out(&wt, &["status", "--porcelain"]).unwrap().1, "");
        // Again: same file, one exclude line.
        assert_eq!(
            write_cargo_config(&wt, "$HOME/.cache/t/{task}", 9, &h, &keep()),
            Ok(true)
        );
        let ex = std::fs::read_to_string(wt.join(".git/info/exclude")).unwrap();
        assert_eq!(ex.matches("/.cargo/config.toml").count(), 1);
        let _ = std::fs::remove_dir_all(&wt);
    }

    #[test]
    fn a_tracked_cargo_config_is_kept_byte_for_byte() {
        let wt = repo();
        std::fs::create_dir_all(wt.join(".cargo")).unwrap();
        let mine = "[build]\njobs = 3\n";
        std::fs::write(wt.join(".cargo/config.toml"), mine).unwrap();
        assert!(git_out(&wt, &["add", ".cargo/config.toml"]).unwrap().0);
        assert_eq!(
            write_cargo_config(&wt, "/x/{task}", 9, &home(), &keep()),
            Ok(false)
        );
        assert_eq!(
            std::fs::read_to_string(wt.join(".cargo/config.toml")).unwrap(),
            mine
        );
        let _ = std::fs::remove_dir_all(&wt);
    }

    #[test]
    fn a_refused_directory_writes_nothing() {
        let wt = repo();
        assert!(write_cargo_config(&wt, "cache/{task}", 9, &home(), &keep()).is_err());
        assert!(!wt.join(".cargo").exists());
        let _ = std::fs::remove_dir_all(&wt);
    }
}
