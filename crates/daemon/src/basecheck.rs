// SPDX-License-Identifier: GPL-3.0-or-later

//! A red on the command path that also fails at the merge base (s177, decision 0172).
//!
//! A test step that runs a plain `command` (not suites) has no baseline. When a test failed
//! on every run, including the targeted rerun, the same tests are run at the merge base,
//! once per (base commit, command, test set), in a throwaway worktree. A failure is excused
//! per test (s281): each test that failed on every run and also fails there is not this
//! change's; the others are. One task, "<test> fails on <base>", is filed for the base; a test already named
//! by an open task or an open tracker issue files nothing, and the excuse points at it.
//!
//! The base results share the `baselines` table with the suites path (keyed by a hash of
//! the command in place of a suite name) and the repository lock; `suites.rs` is untouched.

use crate::jobs::{pty_wrap, step_env_pairs, Ctx};
use mason_core::*;
use mason_landing::git;
use std::process::{Command, Stdio};
use std::sync::{Mutex, MutexGuard, TryLockError};
use std::time::{Duration, Instant};

/// One base run at a time, and one filing at a time: a second task with the same base
/// waits, then finds the first one's result (or the first one's fix task).
pub(crate) static BASE_RUN: BaseRunLock = BaseRunLock::new();
pub(crate) static FILING: Mutex<()> = Mutex::new(());

/// Who holds the base-run lock: the task, what it runs there, and since when.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BaseHolder {
    pub task: u64,
    /// For example "`cargo-workspace` at 7f5d57d812".
    pub what: String,
    pub since_ms: u64,
}

/// The lock behind "one base run at a time" that also remembers its holder, so a task that
/// has to wait for it can say who it waits for (s536, decision 0529).
pub(crate) struct BaseRunLock {
    lock: Mutex<()>,
    holder: Mutex<Option<BaseHolder>>,
}

/// Held for the length of one base run; clears the holder when dropped.
pub(crate) struct BaseRunGuard<'a> {
    owner: &'a BaseRunLock,
    _held: MutexGuard<'a, ()>,
}

impl Drop for BaseRunGuard<'_> {
    // Runs before the fields drop, so the holder is cleared while the lock is still held.
    fn drop(&mut self) {
        *self.owner.holder.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

/// "Waiting for the base check: #452 is running `cargo-workspace` at 7f5d57d (22 min)".
pub(crate) fn base_wait_text(holder: Option<&BaseHolder>, now_ms: u64) -> String {
    match holder {
        Some(h) => {
            let mins = now_ms.saturating_sub(h.since_ms) / 60_000;
            let age = if mins == 0 {
                "under 1 min".to_string()
            } else {
                format!("{mins} min")
            };
            format!(
                "Waiting for the base check: #{} is running {} ({age})",
                h.task, h.what
            )
        }
        None => "Waiting for the base check: another task is running one (holder unknown)".into(),
    }
}

/// A command shortened to its first line and 60 characters, for a wait text.
pub(crate) fn command_label(cmd: &str) -> String {
    let line = cmd.lines().next().unwrap_or("").trim();
    let mut out: String = line.chars().take(60).collect();
    if out.len() < line.len() {
        out.push_str("...");
    }
    format!("`{out}`")
}

impl BaseRunLock {
    pub(crate) const fn new() -> Self {
        BaseRunLock {
            lock: Mutex::new(()),
            holder: Mutex::new(None),
        }
    }

    pub(crate) fn holder(&self) -> Option<BaseHolder> {
        self.holder
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Take the lock for `task` running `what` (which already names the base). Free: no
    /// callback runs. Held: `on_wait` gets the text and says whether it announced a wait,
    /// then this blocks, and `on_end` runs once the lock is ours if it did.
    pub(crate) fn take_with(
        &self,
        task: u64,
        what: &str,
        now_ms: impl Fn() -> u64,
        on_wait: impl FnOnce(String) -> bool,
        on_end: impl FnOnce(),
    ) -> BaseRunGuard<'_> {
        let held = match self.lock.try_lock() {
            Ok(g) => g,
            Err(TryLockError::Poisoned(e)) => e.into_inner(),
            Err(TryLockError::WouldBlock) => {
                let text = base_wait_text(self.holder().as_ref(), now_ms());
                let waiting = on_wait(text);
                let g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
                if waiting {
                    on_end();
                }
                g
            }
        };
        *self.holder.lock().unwrap_or_else(|e| e.into_inner()) = Some(BaseHolder {
            task,
            what: what.to_string(),
            since_ms: now_ms(),
        });
        BaseRunGuard {
            owner: self,
            _held: held,
        }
    }

    /// As `take_with`, announcing a wait through the step's own events (class `base-run`).
    pub(crate) fn take(&self, c: &Ctx, what: &str) -> BaseRunGuard<'_> {
        self.take_with(
            c.task.0,
            what,
            crate::daemon::now_ms,
            |text| {
                c.emit(EventKind::WaitStarted {
                    task: c.task,
                    on: text,
                    class: Some("base-run".into()),
                })
            },
            || {
                c.emit(EventKind::WaitEnded { task: c.task });
            },
        )
    }
}

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

/// The test class (or file) of a test name: everything before the last `::`, ` > `, `/` or
/// `.`, in that order of preference. `None` for a name with no such part.
pub fn class_of(test: &str) -> Option<String> {
    for sep in ["::", " > ", "/", "."] {
        if let Some((head, tail)) = test.rsplit_once(sep) {
            if !head.is_empty() && !tail.is_empty() {
                return Some(head.to_string());
            }
        }
    }
    None
}

/// The line in a fix task's body that says which class and base it covers.
fn class_marker(class: &str, base: &str) -> String {
    format!("Test class: `{class}` at base `{}`", short(base))
}

/// Does a grouped fix task's body list `test`?
fn lists_test(t: &Task, test: &str) -> bool {
    t.title.contains(" fail on ") && t.body.contains(&format!("\n- `{test}`\n"))
}

pub(crate) fn short(sha: &str) -> &str {
    &sha[..sha.len().min(10)]
}

/// The open task or tracker issue that already covers `test`, if any.
pub(crate) fn existing_fix(c: &Ctx, test: &str) -> Option<Fix> {
    existing_fix_in(&c.d, c.project_id, &c.project.id, test)
}

fn existing_fix_in(
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
            .filter(|t| is_fix_title(&t.title, test) || lists_test(t, test))
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

/// What a fix task's exit script must do when the failing suite is the Swift or the Compose
/// client: CI's `exit-scripts` job (Linux) sets `MASON_SKIP_SWIFT` / `MASON_SKIP_COMPOSE`
/// because the clients have jobs of their own, and `scripts/check-exit-guards.sh` fails an
/// exit script that runs `swift` or `gradle` without testing the matching variable.
/// Empty when `text` (the command, step or test name) names neither client.
pub(crate) fn client_guard_note(text: &str) -> String {
    let t = text.to_lowercase();
    let mut out = String::new();
    if t.contains("swift") {
        out.push_str(
            "\n\nExit script: it runs `swift`, so it must start with the skip guard of \
             scripts/exit/s12.sh: print a reason and `exit 0` when `MASON_SKIP_SWIFT` is set, \
             `uname` is not Darwin or `swift` is missing.",
        );
    }
    if t.contains("gradle") || t.contains("compose") || t.contains("kotlin") {
        out.push_str(
            "\n\nExit script: it runs `gradle`, so it must start with the skip guard of \
             scripts/exit/s79.sh: print a reason and `exit 0` when `MASON_SKIP_COMPOSE` is set or \
             `gradle` is missing.",
        );
    }
    out
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
    /// A fix task that landed with the test still red: the new task links it.
    pub follows: Option<u64>,
}

/// The fix for `test` failing at the base: the one already open, or a new urgent task.
fn file_fix(d: &std::sync::Arc<crate::daemon::Daemon>, s: &Seen, test: &str) -> Option<Fix> {
    file_group(d, s, &[test.to_string()])
}

/// The open task for a class and base, if one was filed (s512).
fn existing_group(d: &std::sync::Arc<crate::daemon::Daemon>, s: &Seen, class: &str) -> Option<Fix> {
    let marker = class_marker(class, s.base);
    let w = d.world().ok()?;
    let mut open: Vec<&Task> = w
        .tasks
        .values()
        .filter(|t| t.project == s.project && !t.state.is_terminal())
        .filter(|t| t.body.contains(&marker))
        .collect();
    open.sort_by_key(|t| t.id.0);
    open.first().map(|t| Fix::Task(t.id.0))
}

/// One fix for `tests`, which failed together at one base (s512): a task already open for
/// any of them, or for their class at this base, or one new urgent task naming all of them.
/// Tests of different classes are filed separately by the caller.
fn file_group(
    d: &std::sync::Arc<crate::daemon::Daemon>,
    s: &Seen,
    tests: &[String],
) -> Option<Fix> {
    let _g = FILING.lock().unwrap_or_else(|e| e.into_inner());
    for t in tests {
        if let Some(f) = existing_fix_in(d, s.project, s.project_name, t) {
            return Some(f);
        }
    }
    let class = tests.first().and_then(|t| class_of(t));
    if let Some(c) = &class {
        if let Some(f) = existing_group(d, s, c) {
            return Some(f);
        }
    }
    let found = if s.task == 0 {
        "a run over the base".to_string()
    } else {
        format!("the merge base of task #{} in {}", s.task, s.project_name)
    };
    let (title, names, verb) = match tests {
        [one] => (
            format!("{one} fails on {}", short(s.base)),
            format!("`{one}`"),
            "fails",
        ),
        many => (
            format!(
                "{}: {} tests fail on {}",
                class.as_deref().unwrap_or("tests"),
                many.len(),
                short(s.base)
            ),
            many.iter()
                .map(|t| format!("`{t}`"))
                .collect::<Vec<_>>()
                .join(", "),
            "fail",
        ),
    };
    let list = if tests.len() > 1 {
        let lines: Vec<String> = tests.iter().map(|t| format!("- `{t}`")).collect();
        format!(
            "\n\nThe tests, all of one class, to fix together:\n{}\n",
            lines.join("\n")
        )
    } else {
        String::new()
    };
    let marker = class.as_ref().map_or(String::new(), |c| {
        format!("\n\n{}", class_marker(c, s.base))
    });
    let all = tests.join(" ");
    let body = format!(
        "{names} {verb} on `{}` ({found}), so {} for every \
         task that runs `{}` there. Tasks that hit it are excused (it fails without their \
         change) until this lands. This task's own test step is not excused for {}: \
         {} must pass.{list}{marker}\n\nCommand at the base:\n```\n{}\n```\n\nOutput tail:\n```\n{}\n```\n\n\
         Filed by the known-red ledger (decision 0225). Reds of the same class at this base \
         are added to this task rather than filed again (decision 0505).{}{}",
        s.base,
        if tests.len() > 1 { "they fail" } else { "it fails" },
        s.step,
        if tests.len() > 1 { "them" } else { "it" },
        if tests.len() > 1 { "each of them" } else { names.as_str() },
        s.cmd,
        s.log_tail,
        s.follows.map_or(String::new(), |n| format!("\n\nFollow-up to task #{n}, which landed while this test was still red.")),
        client_guard_note(&format!("{} {} {all}", s.step, s.cmd))
    );
    let a = d
        .emit(EventKind::TaskFiled {
            project: s.project,
            title,
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

/// The fix for `test` failing at `base`: the one already open, or a new urgent task.
#[allow(dead_code)]
pub(crate) fn fix_for(c: &Ctx, test: &str, base: &str, cmd: &str, log_tail: &str) -> Option<Fix> {
    let s = Seen {
        project: c.project_id,
        project_name: &c.project.id,
        task: c.task.0,
        step: &c.step.name,
        base,
        cmd,
        log_tail,
        follows: None,
    };
    file_fix(&c.d, &s, test)
}

/// The fix landed and `test` is still red: file a follow-up that links it and point the
/// ledger entry at it. `None` if nothing could be filed.
pub fn follow_up(
    d: &std::sync::Arc<crate::daemon::Daemon>,
    ledger_suite: &str,
    s: &Seen,
    test: &str,
) -> Option<u64> {
    // The landed task is terminal, so it is not found as "already open".
    if let Fix::Task(n) = file_fix(d, s, test)? {
        let _ = d.store.ledger_set_fix(ledger_suite, test, n);
        return Some(n);
    }
    None
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
    let mut out: Vec<(String, Option<Fix>)> = vec![];
    // Tests with no fix yet, in order; grouped by class below.
    let mut todo: Vec<String> = vec![];
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
        match known {
            Some(n) => out.push((t.clone(), Some(Fix::Task(n)))),
            None => todo.push(t.clone()),
        }
    }
    let mut groups: Vec<Vec<String>> = vec![];
    for t in todo {
        let class = class_of(&t);
        match class.as_ref().and_then(|c| {
            groups
                .iter_mut()
                .find(|g| class_of(&g[0]).as_ref() == Some(c))
        }) {
            Some(g) => g.push(t),
            None => groups.push(vec![t]),
        }
    }
    for g in groups {
        let f = file_group(d, s, &g);
        for t in g {
            if let Some(Fix::Task(n)) = &f {
                let _ = d.store.ledger_set_fix(ledger_suite, &t, *n);
            }
            out.push((t, f.clone()));
        }
    }
    out
}

/// Run `cmd` (after `build`, if any) in a throwaway worktree at `base`; the failing names.
/// `None` if the worktree, the run or its time limit did not give a result.
/// The third part says whether the output shows every test of `expect` ran (true when
/// `expect` is empty): a run that ran none of them proves nothing about them.
pub(crate) fn run_at_base(
    c: &Ctx,
    base: &str,
    build: Option<&str>,
    cmd: &str,
    expect: &[String],
) -> Option<(Vec<String>, String, bool)> {
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
    if let Err(e) = crate::jobs::setup_checkout(&c.def, &wt) {
        tracing::warn!(task = c.task.0, "base check at {base}: {e}");
        let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
        git::remove_worktree(repo, &wt);
        return None;
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
        for (k, v) in step_env_pairs(c) {
            let v = if k == "MASON_WORKTREE" {
                wt.to_string_lossy().into_owned()
            } else {
                v
            };
            command.env(k, v);
        }
        let mut child = mason_platform::process::spawn_group(&mut command).ok()?;
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
            crate::rerun::ran_them(&text, expect),
        ))
    })();
    {
        let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
        git::remove_worktree(repo, &wt);
    }
    result
}

/// The merge base of the change and the project's base.
pub(crate) fn merge_base(c: &Ctx) -> Option<String> {
    let head = git::run(&c.wt, &["rev-parse", "HEAD"])
        .ok()?
        .trim()
        .to_string();
    git::run(&c.wt, &["merge-base", &c.project.base_ref(), &head])
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// What the base check excused: the tests that also fail at the merge base (s281: per test,
/// so one new failure beside a known one no longer sends both back), and a line for each.
pub struct Excused {
    pub tests: Vec<String>,
    pub lines: Vec<String>,
}

/// The red step's tests (`persistent`: failed on every run) are checked at the merge base.
/// `base_cmd` is the command to run there: the rerun filter's command with only those
/// tests, or the whole command. Each one that also fails there is excused (and its fix filed
/// or found); the rest are the change's. `None` if there is no base result or nothing is
/// excused. The lines are also written next to the step's log for `task show`.
pub fn excuse(
    c: &Ctx,
    suffix: &str,
    build: Option<&str>,
    base_cmd: &str,
    persistent: &[String],
) -> Option<Excused> {
    if persistent.is_empty() {
        return None;
    }
    // The fix task's own step is not excused for the tests it was filed for.
    let own = own_tests(&c.d.store, &c.step.name, c.task.0);
    let base = merge_base(c)?;
    let key = key_of(&match build {
        Some(b) => format!("{b}\n{base_cmd}"),
        None => base_cmd.to_string(),
    });
    let mut tail = String::new();
    let failing = {
        let _g = BASE_RUN.take(
            c,
            &format!("{} at {}", command_label(base_cmd), short(&base)),
        );
        cached_failures(&c.d.store, &base, &key, &mut || {
            let (names, t, _) = run_at_base(c, &base, build, base_cmd, &[])?;
            tail = t;
            Some(names)
        })?
    };
    let mut tests = at_base(persistent, &failing);
    tests.retain(|t| !own.contains(t));
    if tests.is_empty() {
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
        follows: None,
    };
    let fixes = record_red(&c.d, &c.step.name, &seen, &tests);
    let mut lines = vec![];
    for t in &tests {
        let fix = fixes
            .iter()
            .find(|(n, _)| n == t)
            .and_then(|(_, f)| f.clone());
        lines.push(format!(
            "{t}: also fails at the merge base {}; {}",
            short(&base),
            fix.as_ref()
                .map_or("no fix could be filed".to_string(), |f| format!(
                    "fix: {}",
                    f.text()
                ))
        ));
    }
    let _ = std::fs::write(
        c.task_dir().join(format!("{suffix}-r{}.excused", c.run.0)),
        lines.join("\n"),
    );
    Some(Excused { tests, lines })
}

/// The tests of `persistent` that also fail at the base.
pub fn at_base(persistent: &[String], at_base: &[String]) -> Vec<String> {
    persistent
        .iter()
        .filter(|t| at_base.contains(t))
        .cloned()
        .collect()
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
    fn an_uncontended_base_run_take_announces_nothing() {
        let lock = BaseRunLock::new();
        let mut waits = 0;
        let mut ends = 0;
        let g = lock.take_with(
            7,
            "`x` at abc",
            || 5,
            |_| {
                waits += 1;
                true
            },
            || ends += 1,
        );
        assert_eq!(lock.holder().map(|h| h.task), Some(7));
        drop(g);
        assert_eq!((waits, ends), (0, 0));
        assert_eq!(lock.holder(), None);
    }

    #[test]
    fn a_task_waiting_for_the_base_run_names_the_holder_and_takes_over() {
        use std::sync::mpsc::channel;
        let lock = BaseRunLock::new();
        let (text_tx, text_rx) = channel::<String>();
        let (end_tx, end_rx) = channel::<()>();
        std::thread::scope(|s| {
            let a = lock.take_with(452, "`cargo-workspace` at 7f5d57d", || 0, |_| true, || {});
            let b = s.spawn(|| {
                let g = lock.take_with(
                    451,
                    "`other` at 7f5d57d",
                    || 22 * 60_000,
                    |t| {
                        text_tx.send(t).unwrap();
                        true
                    },
                    || end_tx.send(()).unwrap(),
                );
                let holder = lock.holder();
                drop(g);
                holder
            });
            let text = text_rx.recv().unwrap();
            assert_eq!(
                text,
                "Waiting for the base check: #452 is running `cargo-workspace` at 7f5d57d (22 min)"
            );
            drop(a);
            let holder = b.join().unwrap();
            assert_eq!(holder.map(|h| h.task), Some(451));
        });
        end_rx.recv().unwrap();
        assert!(end_rx.try_recv().is_err(), "on_end ran once");
        assert_eq!(lock.holder(), None);
    }

    #[test]
    fn an_unknown_holder_is_said_to_be_unknown() {
        assert!(base_wait_text(None, 0).contains("holder unknown"));
    }

    #[test]
    fn a_client_suite_gets_the_exit_script_guard_requirement() {
        let sw = client_guard_note("swift test --filter TopBarFitTests");
        assert!(sw.contains("MASON_SKIP_SWIFT") && !sw.contains("MASON_SKIP_COMPOSE"));
        let kt = client_guard_note("cd clients/compose && gradle :core:test");
        assert!(kt.contains("MASON_SKIP_COMPOSE") && !kt.contains("MASON_SKIP_SWIFT"));
        assert_eq!(client_guard_note("cargo test -p mason-core"), "");
    }

    #[test]
    fn a_failure_is_excused_only_when_every_failing_test_fails_at_the_base() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert!(all_excused(&s(&["a"]), &s(&["a", "z"])));
        assert!(!all_excused(&s(&["a", "b"]), &s(&["a"])));
        assert!(!all_excused(&s(&["a"]), &[]));
        assert!(!all_excused(&[], &s(&["a"])));
    }

    #[test]
    fn each_test_is_excused_on_its_own() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(at_base(&s(&["a", "b"]), &s(&["a", "z"])), s(&["a"]));
        assert!(at_base(&s(&["b"]), &s(&["a"])).is_empty());
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
    fn a_test_class_is_what_precedes_the_last_separator() {
        assert_eq!(class_of("MascotTest > a b").as_deref(), Some("MascotTest"));
        assert_eq!(class_of("a::b::c").as_deref(), Some("a::b"));
        assert_eq!(
            class_of("pkg.MascotTest.go").as_deref(),
            Some("pkg.MascotTest")
        );
        assert_eq!(
            class_of("MascotTests/testA").as_deref(),
            Some("MascotTests")
        );
        assert_eq!(class_of("plain"), None);
    }

    #[test]
    fn a_fix_title_names_the_test_and_the_base() {
        assert!(is_fix_title("a::b fails on 1234567890", "a::b"));
        assert!(!is_fix_title("a::b is slow", "a::b"));
        assert!(!is_fix_title("c fails on 1234567890", "a::b"));
    }
}
