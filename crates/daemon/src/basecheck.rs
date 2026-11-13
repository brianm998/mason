// SPDX-License-Identifier: GPL-3.0-or-later

//! A red on the command path that also fails at the merge base (s177, decision 0172).
//!
//! A test step that runs a plain `command` (not suites) has no baseline. When a test failed
//! on every run, including the targeted rerun, the same tests are run at the merge base,
//! once per (base commit, command, test set), in a throwaway worktree. A failure is excused
//! only when **every** test that failed on every run also fails there: those are not this
//! change's. One task, "<test> fails on <base>", is filed for the base; a test already named
//! by an open task or an open tracker issue files nothing, and the excuse points at it.
//!
//! The base results share the `baselines` table with the suites path (keyed by a hash of
//! the command in place of a suite name) and the repository lock; `suites.rs` is untouched.

use crate::jobs::{pty_wrap, step_env_pairs, Ctx};
use mason_core::*;
use mason_landing::git;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// One base run at a time, and one filing at a time: a second task with the same base
/// waits, then finds the first one's result (or the first one's fix task).
static BASE_RUN: Mutex<()> = Mutex::new(());
static FILING: Mutex<()> = Mutex::new(());

/// The name a command's base result is kept under in `baselines.suite`.
pub fn key_of(cmd: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in cmd.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("cmd:{h:016x}")
}

/// The tests that fail at `base` for `key`: from the cache, or by `run` (which gives the
/// failing names, or `None` if it could not tell). A result is worked out once.
pub fn cached_failures(
    store: &mason_store::Store,
    base: &str,
    key: &str,
    run: &mut dyn FnMut() -> Option<Vec<String>>,
) -> Option<Vec<String>> {
    if let Ok(Some((failing, _))) = store.baseline(base, key) {
        return Some(failing);
    }
    let failing = run()?;
    let _ = store.put_baseline(base, key, &failing, &failing, 0, crate::daemon::now_ms());
    Some(failing)
}

/// Every test in `persistent` is in `at_base` (and there is something to excuse).
pub fn all_excused(persistent: &[String], at_base: &[String]) -> bool {
    !persistent.is_empty() && persistent.iter().all(|t| at_base.contains(t))
}

/// Where a fix for a base failure is already tracked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fix {
    Task(u64),
    Issue(u64),
}

impl Fix {
    pub fn text(&self) -> String {
        match self {
            Fix::Task(n) => format!("task #{n}"),
            Fix::Issue(n) => format!("issue #{n}"),
        }
    }
}

/// Is `title` the title of a base-failure task for `test`?
pub fn is_fix_title(title: &str, test: &str) -> bool {
    title.contains(test) && title.contains(" fails on ")
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(10)]
}

/// The open task or tracker issue that already covers `test`, if any.
fn existing_fix(
    d: &std::sync::Arc<crate::daemon::Daemon>,
    project: ProjectId,
    project_name: &str,
    test: &str,
) -> Option<Fix> {
    if let Ok(w) = d.world() {
        let mut open: Vec<&Task> = w
            .tasks
            .values()
            .filter(|t| t.project == project && !t.state.is_terminal())
            .filter(|t| is_fix_title(&t.title, test))
            .collect();
        open.sort_by_key(|t| t.id.0);
        if let Some(t) = open.first() {
            return Some(Fix::Task(t.id.0));
        }
    }
    d.store
        .tracker_items(project_name)
        .unwrap_or_default()
        .into_iter()
        .find(|it| it.open && (it.title.contains(test) || it.body.contains(test)))
        .map(|it| Fix::Issue(it.number))
}

/// Where a base failure was seen: enough to file its fix task.
pub struct Seen<'a> {
    pub project: ProjectId,
    pub project_name: &'a str,
    /// The task whose run found it (0 for none: a landing or deferred run).
    pub task: u64,
    /// The step or suite that ran the test.
    pub step: &'a str,
    pub base: &'a str,
    pub cmd: &'a str,
    pub log_tail: &'a str,
}

/// The fix for `test` failing at the base: the one already open, or a new urgent task.
fn file_fix(d: &std::sync::Arc<crate::daemon::Daemon>, s: &Seen, test: &str) -> Option<Fix> {
    let _g = FILING.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(f) = existing_fix(d, s.project, s.project_name, test) {
        return Some(f);
    }
    let found = if s.task == 0 {
        "a run over the base".to_string()
    } else {
        format!("the merge base of task #{} in {}", s.task, s.project_name)
    };
    let body = format!(
        "`{test}` fails on `{}` ({found}), so it fails for every \
         task that runs `{}` there. Tasks that hit it are excused (it fails without their \
         change) until this lands. This task's own test step is not excused for it: \
         `{test}` must pass.\n\nCommand at the base:\n```\n{}\n```\n\nOutput tail:\n```\n{}\n```\n\n\
         Filed by the known-red ledger (decision 0225).",
        s.base, s.step, s.cmd, s.log_tail
    );
    let a = d
        .emit(EventKind::TaskFiled {
            project: s.project,
            title: format!("{test} fails on {}", short(s.base)),
            body,
            priority: Priority::Urgent,
            trust: TrustSource::Owner,
            explored: false,
            proposed: false,
            blockers: vec![],
            not_before: None,
        })
        .ok()?;
    a.effects.iter().find_map(|e| match e {
        Effect::TaskCreated { task } => Some(Fix::Task(task.0)),
        _ => None,
    })
}

/// The tests in `suite`'s ledger whose fix is task `task`.
pub fn own_tests(store: &mason_store::Store, suite: &str, task: u64) -> Vec<String> {
    if task == 0 {
        return vec![];
    }
    store
        .ledger()
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.suite == suite && r.fix_task == Some(task))
        .map(|r| r.test)
        .collect()
}

/// Put the tests seen red at the base into the ledger and give each a fix: the first
/// sighting adds the entry and files (or finds) one task, later sightings only refresh it.
/// Names that are not test names are never added. Returns `(test, fix)` for every test.
pub fn record_red(
    d: &std::sync::Arc<crate::daemon::Daemon>,
    ledger_suite: &str,
    s: &Seen,
    tests: &[String],
) -> Vec<(String, Option<Fix>)> {
    let now = crate::daemon::now_ms();
    let mut out = vec![];
    for t in tests {
        if !mason_testing::gates::is_test_name(t) {
            continue;
        }
        if d.store
            .ledger_seen_red(ledger_suite, t, "base", s.base, now)
            .is_err()
        {
            out.push((t.clone(), None));
            continue;
        }
        let known = d
            .store
            .ledger()
            .unwrap_or_default()
            .into_iter()
            .find(|r| r.suite == ledger_suite && &r.test == t)
            .and_then(|r| r.fix_task);
        let fix = match known {
            Some(n) => Some(Fix::Task(n)),
            None => {
                let f = file_fix(d, s, t);
                if let Some(Fix::Task(n)) = &f {
                    let _ = d.store.ledger_set_fix(ledger_suite, t, *n);
                }
                f
            }
        };
        out.push((t.clone(), fix));
    }
    out
}

/// Run `cmd` (after `build`, if any) in a throwaway worktree at `base`; the failing names.
/// `None` if the worktree, the run or its time limit did not give a result.
fn run_at_base(
    c: &Ctx,
    base: &str,
    build: Option<&str>,
    cmd: &str,
) -> Option<(Vec<String>, String)> {
    let repo = &c.project.path;
    let wt = c.task_dir().join(format!("base-r{}", c.run.0));
    let lock = c.d.repo_lock(c.project_id);
    {
        let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
        git::remove_worktree(repo, &wt);
        git::run(
            repo,
            &["worktree", "add", "--detach", &wt.to_string_lossy(), base],
        )
        .ok()?;
    }
    let log = c.task_dir().join(format!("base-r{}.log", c.run.0));
    let result = (|| {
        let full = match build {
            Some(b) => format!("({b}) && ({cmd})"),
            None => cmd.to_string(),
        };
        let out = std::fs::File::create(&log).ok()?;
        let err = out.try_clone().ok()?;
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg(pty_wrap(c.step.pty, &full))
            .current_dir(&wt)
            .stdin(Stdio::null())
            .stdout(out)
            .stderr(err);
        mason_platform::process::group_leader(&mut command);
        for (k, v) in step_env_pairs(c) {
            let v = if k == "MASON_WORKTREE" {
                wt.to_string_lossy().into_owned()
            } else {
                v
            };
            command.env(k, v);
        }
        let mut child = command.spawn().ok()?;
        let limit = c
            .res
            .budget
            .as_ref()
            .map_or(c.project.test_timeout, |a| a.value);
        let end = Instant::now() + limit;
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() >= end => {
                    let _ = Command::new("kill")
                        .args(["-KILL", &format!("-{}", child.id())])
                        .status();
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(100)),
                Err(_) => return None,
            }
        }
        let text = std::fs::read(&log)
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default();
        Some((
            crate::rerun::failing_names(&text),
            crate::jobs::tail(&text, 30),
        ))
    })();
    {
        let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
        git::remove_worktree(repo, &wt);
    }
    result
}

/// The merge base of the change and the project's base.
fn merge_base(c: &Ctx) -> Option<String> {
    let head = git::run(&c.wt, &["rev-parse", "HEAD"])
        .ok()?
        .trim()
        .to_string();
    git::run(&c.wt, &["merge-base", &c.project.base_ref(), &head])
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// The red step's tests (`persistent`: failed on every run) are checked at the merge base.
/// `base_cmd` is the command to run there: the rerun filter's command with only those
/// tests, or the whole command. `Some(note)` if every one also fails there: the step is
/// excused, the note says so (and is also written next to the step's log for `task show`).
pub fn excuse(
    c: &Ctx,
    suffix: &str,
    build: Option<&str>,
    base_cmd: &str,
    persistent: &[String],
) -> Option<String> {
    if persistent.is_empty() {
        return None;
    }
    // The fix task's own step is not excused for the tests it was filed for.
    if own_tests(&c.d.store, &c.step.name, c.task.0)
        .iter()
        .any(|t| persistent.contains(t))
    {
        return None;
    }
    let base = merge_base(c)?;
    let key = key_of(&match build {
        Some(b) => format!("{b}\n{base_cmd}"),
        None => base_cmd.to_string(),
    });
    let mut tail = String::new();
    let failing = {
        let _g = BASE_RUN.lock().unwrap_or_else(|e| e.into_inner());
        cached_failures(&c.d.store, &base, &key, &mut || {
            let (names, t) = run_at_base(c, &base, build, base_cmd)?;
            tail = t;
            Some(names)
        })?
    };
    if !all_excused(persistent, &failing) {
        return None;
    }
    let seen = Seen {
        project: c.project_id,
        project_name: &c.project.id,
        task: c.task.0,
        step: &c.step.name,
        base: &base,
        cmd: base_cmd,
        log_tail: &tail,
    };
    let fixes = record_red(&c.d, &c.step.name, &seen, persistent);
    let mut lines = vec![];
    for t in persistent {
        let fix = fixes
            .iter()
            .find(|(n, _)| n == t)
            .and_then(|(_, f)| f.clone());
        let line = format!(
            "{t}: also fails at the merge base {}; {}",
            short(&base),
            fix.as_ref()
                .map_or("no fix could be filed".to_string(), |f| format!(
                    "fix: {}",
                    f.text()
                ))
        );
        lines.push(line);
    }
    let note = format!(
        "`{}` was red, but every test that failed on every run also fails at the merge base, so none is this change's (excused):\n{}",
        c.step.name,
        lines.join("\n")
    );
    let _ = std::fs::write(
        c.task_dir().join(format!("{suffix}-r{}.excused", c.run.0)),
        lines.join("\n"),
    );
    Some(note)
}

/// The excuse lines recorded for a task, for `mason task show`.
pub fn recorded(dir: &std::path::Path) -> Vec<String> {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "excused"))
        .collect();
    files.sort();
    files
        .iter()
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .flat_map(|t| t.lines().map(String::from).collect::<Vec<_>>())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failure_is_excused_only_when_every_failing_test_fails_at_the_base() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert!(all_excused(&s(&["a"]), &s(&["a", "z"])));
        assert!(!all_excused(&s(&["a", "b"]), &s(&["a"])));
        assert!(!all_excused(&s(&["a"]), &[]));
        assert!(!all_excused(&[], &s(&["a"])));
    }

    #[test]
    fn the_base_runs_once_per_base_command_and_test_set() {
        let dir = tempfile::tempdir().unwrap();
        let store = mason_store::Store::open(dir.path().join("s.db")).unwrap();
        let runs = std::cell::Cell::new(0);
        let k = key_of("cargo test -- --exact 'a'");
        let mut run = || {
            runs.set(runs.get() + 1);
            Some(vec!["a".to_string()])
        };
        assert_eq!(
            cached_failures(&store, "b1", &k, &mut run),
            Some(vec!["a".to_string()])
        );
        assert_eq!(
            cached_failures(&store, "b1", &k, &mut run),
            Some(vec!["a".to_string()])
        );
        assert_eq!(runs.get(), 1, "the second task used the cache");
        // Another base, or another test set, runs again.
        cached_failures(&store, "b2", &k, &mut run);
        cached_failures(&store, "b1", &key_of("cargo test -- --exact 'b'"), &mut run);
        assert_eq!(runs.get(), 3);
        // A run that gave no result is not cached.
        let mut none = || None;
        assert_eq!(cached_failures(&store, "b3", &k, &mut none), None);
        assert!(store.baseline("b3", &k).unwrap().is_none());
    }

    #[test]
    fn a_fix_title_names_the_test_and_the_base() {
        assert!(is_fix_title("a::b fails on 1234567890", "a::b"));
        assert!(!is_fix_title("a::b is slow", "a::b"));
        assert!(!is_fix_title("c fails on 1234567890", "a::b"));
    }
}
