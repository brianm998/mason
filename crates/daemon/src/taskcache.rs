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
}
