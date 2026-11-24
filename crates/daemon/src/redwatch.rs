// SPDX-License-Identifier: GPL-3.0-or-later

//! A red develop that gets fixed moves the tasks it held up (#1023).
//!
//! On 2026-11-22 twelve developing tasks failed the same tests in their checks for hours. The
//! tests were red on develop, develop was fixed, and nothing told the agents or brought the fix
//! to them. MASON had every fact: a red baseline, the same failures across many tasks, a later
//! develop commit that fixed them.
//!
//! Three things happen here, on a background pass:
//!
//! 1. A suite whose newest whole baseline is red at a base that develop has left is rerun, only
//!    those tests, at the new tip. The result is a *partial* baseline: it says what those tests
//!    did there and nothing about the rest of the suite. `Store::latest_baseline` reads it, so a
//!    test that is green at the newer tip is no longer "red on develop".
//! 2. For each developing task whose last check failed a test that is now green, develop is
//!    merged into its worktree if that is safe (`fixsync::merge_clean`), and a note is left for
//!    the agent's next `run_check` result. If it is not safe, the note says why and what to do,
//!    and an owner alert follows when the task is still behind half an hour later.
//! 3. The same test, not known red and not in a current baseline, failing in the checks of three
//!    or more tasks within six hours is run at the tip. If it fails there it is red on develop:
//!    a partial baseline row and one urgent task. If it passes, the tasks are behind (step 2).
//!
//! No test is skipped, excused or weakened: a base-red test still runs and still fails a check.
//! Only the explanation and the follow-up change.

use crate::basefold::BaseFold;
use crate::daemon::{now_ms, Daemon};
use mason_core::{EventKind, Priority, State, TaskId, TrustSource};
use mason_landing::git;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// How far back a sighting counts toward a red develop.
pub const WINDOW_MS: u64 = 6 * 60 * 60 * 1000;
/// How many different tasks must have seen a test fail.
pub const MIN_TASKS: usize = 3;
/// How long a task may stay behind a fix it could not take before the owner is told.
pub const ALERT_AFTER_MS: u64 = 30 * 60 * 1000;

/// The tests that were red and did not fail when run again: they are fixed.
pub fn fixed(probed: &[String], failing_now: &[String]) -> Vec<String> {
    probed
        .iter()
        .filter(|t| !failing_now.contains(t))
        .cloned()
        .collect()
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(10)]
}

/// Told to the agent once develop was merged into its branch.
pub fn merged_note(tests: &[String], old: &str, new: &str) -> String {
    if old.is_empty() {
        return format!(
            "{} failed in your checks and {} on develop at {}, so your branch was behind. \
             develop is merged into your branch: run your check again.",
            tests.join(", "),
            if tests.len() == 1 { "passes" } else { "pass" },
            short(new)
        );
    }
    format!(
        "{} red on develop at {}; {} at {}. develop is merged into your branch: run your check \
         again.",
        tests.join(", "),
        short(old),
        if tests.len() == 1 {
            "it passes"
        } else {
            "they pass"
        },
        short(new)
    )
}

/// Told to the agent when develop could not be merged for it.
pub fn refused_note(tests: &[String], fold: &BaseFold) -> String {
    let what = format!(
        "develop has fixed {}, which your branch still fails;",
        tests.join(", ")
    );
    match fold {
        BaseFold::Conflicts(_, files) => format!(
            "{what} merging it conflicts in {}: merge develop and resolve them before anything \
             else.",
            files.lines().collect::<Vec<_>>().join(", ")
        ),
        BaseFold::Uncommitted(_) => format!(
            "{what} it was not merged because your worktree has uncommitted changes: commit \
             your work and merge develop before anything else."
        ),
        BaseFold::Failed(e) => {
            format!("{what} merging it failed ({e}): merge develop before anything else.")
        }
        BaseFold::Merged(_) | BaseFold::Current => {
            format!("{what} merge develop before anything else.")
        }
    }
}

/// A test that failed in the checks of several tasks and is not explained.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suspect {
    pub suite: String,
    pub test: String,
    pub tasks: Vec<u64>,
}

/// The tests that failed, in `(task, suite, test, at_ms)` sightings, in `MIN_TASKS` or more
/// different tasks within [`WINDOW_MS`] of `now`, and that `explained(suite, test)` does not
/// account for (known red, or in a current baseline).
pub fn suspects(
    sightings: &[(u64, String, String, u64)],
    now: u64,
    explained: &dyn Fn(&str, &str) -> bool,
) -> Vec<Suspect> {
    let mut by: BTreeMap<(String, String), BTreeSet<u64>> = BTreeMap::new();
    for (task, suite, test, at) in sightings {
        if now.saturating_sub(*at) <= WINDOW_MS && !explained(suite, test) {
            by.entry((suite.clone(), test.clone()))
                .or_default()
                .insert(*task);
        }
    }
    by.into_iter()
        .filter(|(_, t)| t.len() >= MIN_TASKS)
        .map(|((suite, test), t)| Suspect {
            suite,
            test,
            tasks: t.into_iter().collect(),
        })
        .collect()
}

/// The fixes that a branch has not taken for [`ALERT_AFTER_MS`], from `(task, since_ms)`.
pub fn overdue(pending: &[(u64, u64)], now: u64) -> Vec<u64> {
    pending
        .iter()
        .filter(|(_, since)| now.saturating_sub(*since) >= ALERT_AFTER_MS)
        .map(|(t, _)| *t)
        .collect()
}

/// Text for the tasks held by a red develop, or `None` when none is: `held` is
/// `(task, test)` for developing tasks whose last check failed a test that is red on develop.
pub fn held_text(held: &[(u64, String)]) -> Option<String> {
    let tasks: BTreeSet<u64> = held.iter().map(|h| h.0).collect();
    if tasks.is_empty() {
        return None;
    }
    let tests: BTreeSet<&str> = held.iter().map(|h| h.1.as_str()).collect();
    Some(format!(
        "{} task{} held by a red develop ({})",
        tasks.len(),
        if tasks.len() == 1 { "" } else { "s" },
        tests.into_iter().collect::<Vec<_>>().join(", ")
    ))
}

/// The condition alerts: tasks held by a red develop while that is true, and a task still
/// behind a fix half an hour after it was told.
pub fn alerts(d: &Daemon) -> Vec<(String, String, String)> {
    alerts_at(d, now_ms())
}

/// A task whose last check's failures are all red on develop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Held {
    pub suite: String,
    pub tests: Vec<String>,
    pub fix: Option<u64>,
}

/// The tasks held by a red develop: developing, and every test their last check of a suite
/// failed is red on develop (in the newest baseline that says anything of it). Keyed by task.
pub fn held(d: &Daemon, w: &mason_engine::World, now: u64) -> BTreeMap<u64, Held> {
    let red = |suite: &str, test: &str| {
        d.store
            .latest_baseline(suite)
            .ok()
            .flatten()
            .is_some_and(|(_, r)| r.iter().any(|t| t == test))
    };
    let mut by: BTreeMap<(u64, String), Vec<String>> = BTreeMap::new();
    for (task, suite, test, _) in d
        .store
        .red_sightings_since(now.saturating_sub(WINDOW_MS))
        .unwrap_or_default()
    {
        if w.tasks
            .get(&TaskId(task))
            .is_some_and(|t| t.state == State::Developing)
        {
            by.entry((task, suite)).or_default().push(test);
        }
    }
    let mut out: BTreeMap<u64, Held> = BTreeMap::new();
    for ((task, suite), tests) in by {
        if !tests.iter().all(|t| red(&suite, t)) {
            continue;
        }
        let head = format!("`{suite}` is red on ");
        let fix = w
            .tasks
            .values()
            .filter(|t| !t.state.is_terminal() && t.title.starts_with(&head))
            .map(|t| t.id.0)
            .min();
        let e = out.entry(task).or_insert_with(|| Held {
            suite: suite.clone(),
            tests: vec![],
            fix,
        });
        e.tests.extend(tests);
    }
    out
}

/// What a task held by a red develop is waiting for, as its wait line.
pub fn held_sentence(h: &Held) -> String {
    format!(
        "`{}` is red on develop: {} fail{} in your checks and on develop too{}",
        h.suite,
        h.tests.join(", "),
        if h.tests.len() == 1 { "s" } else { "" },
        h.fix
            .map_or(String::new(), |t| format!("; the fix is task #{t}")),
    )
}

/// [`alerts`] on a given clock.
pub fn alerts_at(d: &Daemon, now: u64) -> Vec<(String, String, String)> {
    let Ok(w) = d.world() else { return vec![] };
    let mut out = vec![];
    let held: Vec<(u64, String)> = held(d, &w, now)
        .into_iter()
        .flat_map(|(t, h)| h.tests.into_iter().map(move |x| (t, x)))
        .collect();
    if let Some(text) = held_text(&held) {
        out.push(("red-develop".to_string(), "landing".to_string(), text));
    }
    let pending = d.store.red_fix_pending().unwrap_or_default();
    let ids: Vec<(u64, u64)> = pending.iter().map(|p| (p.0, p.4)).collect();
    let late = overdue(&ids, now);
    out.extend(
        pending
            .into_iter()
            .filter(|p| late.contains(&p.0))
            .filter_map(|(task, tests, base, why, _)| {
                let t = w.tasks.get(&TaskId(task))?;
                if t.state != State::Developing {
                    return None;
                }
                let p = d.project(t.project)?;
                if git::is_ancestor(&p.worktree(&d.cfg, task), &base, "HEAD") {
                    let _ = d.store.clear_red_fix_pending(task);
                    return None;
                }
                Some((
                    format!("red-fix:{task}"),
                    "needs_you".to_string(),
                    format!(
                        "Task #{task} is still behind develop's fix for {}: {why}",
                        tests.join(", ")
                    ),
                ))
            }),
    );
    out
}

/// Tasks of `pid` that are developing, with their worktrees.
fn developing(d: &Arc<Daemon>, pid: mason_core::ProjectId) -> Vec<TaskId> {
    let Ok(w) = d.world() else { return vec![] };
    w.tasks
        .values()
        .filter(|t| t.project == pid && t.state == State::Developing)
        .map(|t| t.id)
        .collect()
}

/// Tell the tasks whose last check failed `tests` of `suite` that develop (`new`) fixes them,
/// and merge develop into each worktree where that is safe.
fn deliver(
    d: &Arc<Daemon>,
    pid: mason_core::ProjectId,
    suite: &str,
    tests: &[String],
    (old, new): (&str, &str),
    only: Option<&[u64]>,
    now: u64,
) {
    let Some(p) = d.project(pid) else { return };
    let base = p.base_ref();
    let sightings = d.store.red_sightings_since(0).unwrap_or_default();
    let live = developing(d, pid);
    for task in live {
        if only.is_some_and(|o| !o.contains(&task.0)) {
            continue;
        }
        let mine: Vec<String> = tests
            .iter()
            .filter(|t| {
                sightings
                    .iter()
                    .any(|(k, s, x, _)| *k == task.0 && s == suite && x == *t)
            })
            .cloned()
            .collect();
        if mine.is_empty() {
            continue;
        }
        let wt = p.worktree(&d.cfg, task.0);
        if !wt.exists() {
            continue;
        }
        let fold = {
            let lock = d.repo_lock(pid);
            let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
            crate::fixsync::merge_clean(&wt, &base)
        };
        tracing::info!(task = task.0, ?fold, tests = ?mine, "red develop fixed: fold");
        let note = match &fold {
            BaseFold::Merged(_) | BaseFold::Current => {
                let _ = d.store.bump_red_folds(task.0);
                let _ = d.store.clear_red_fix_pending(task.0);
                merged_note(&mine, old, new)
            }
            other => {
                let n = refused_note(&mine, other);
                let _ = d.store.put_red_fix_pending(task.0, &mine, &base, &n, now);
                n
            }
        };
        let _ = d.store.push_red_note(task.0, &note, now);
        let _ = d.store.clear_red_sightings(task.0, suite, Some(&mine));
    }
}

/// Run `tests` of `suite` narrowed at `tip` in a throwaway worktree. `None` when that cannot be
/// told: the project has no `rerun_filter`, the checkout could not be set up, or the run was
/// stopped. Otherwise the tests that failed.
fn probe(
    d: &Arc<Daemon>,
    pid: mason_core::ProjectId,
    suite: &mason_testing::Suite,
    tip: &str,
    tests: &[String],
) -> Option<Vec<String>> {
    static ONE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _one = ONE.lock().unwrap_or_else(|e| e.into_inner());
    let p = d.project(pid)?;
    let filter = p.rerun_filter.as_deref()?;
    let def = d.current_def(pid);
    let test_cmd = crate::rerun::command(&suite.command, Some(filter), tests);
    let cmd = match &suite.build {
        Some(b) => format!("( {b} ) && ( {test_cmd} )"),
        None => test_cmd,
    };
    let wt = d
        .cfg
        .worktrees_dir()
        .join(&p.id)
        .join(format!("redwatch-{}", suite.name));
    let lock = d.repo_lock(pid);
    {
        let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
        git::remove_worktree(&p.path, &wt);
        git::run(
            &p.path,
            &["worktree", "add", "--detach", &wt.to_string_lossy(), tip],
        )
        .ok()?;
    }
    let log_dir = d.cfg.worktrees_dir().join(&p.id).join("redwatch-logs");
    let result = (|| {
        crate::jobs::setup_checkout(&def, &wt).ok()?;
        let mut spec = crate::hostedcheck::CheckSpec {
            id_prefix: format!("redwatch-{}-{}", suite.name, short(tip)),
            log_dir: log_dir.clone(),
            cmd,
            build: None,
            cwd: wt.clone(),
            env: vec![],
            limit: p.test_timeout,
            tree: crate::hostedcheck::tree_key(&wt),
            stop: None,
            holds: Default::default(),
            label: format!("red develop at {}", short(tip)),
            place: None,
            global: true,
        };
        // Placed as any run is (the project's mode, through the one function that decides): a
        // run bound for GitHub waits in the broker for its runner units and is pushed once they
        // are granted, behind every ranked request; one that runs here takes the suite's slots
        // as owed work. Nothing runs outside the queue.
        let units = crate::hostedcheck::pool_units(d, &spec)
            .filter(|_| !crate::hostedcheck::pending(d, &spec));
        let mut uses = suite.uses.clone();
        if suite.build.is_some() {
            *uses
                .entry(mason_scheduler::COMPILE.to_string())
                .or_insert(0) += 1;
        }
        let local = units.is_none();
        let ask = units.unwrap_or_else(|| uses.clone());
        let lease = crate::remote::fallback_lease(d, &p.id, None, &ask, &spec.label).ok()?;
        // Bound for GitHub the lease is the push's grant; here it is held while the run lasts.
        let (_slots, grant) = if local {
            (lease, crate::remote::Grant::attached())
        } else {
            (
                None,
                lease.map_or_else(crate::remote::Grant::attached, crate::remote::Grant::of),
            )
        };
        spec.holds = if local { uses } else { Default::default() };
        let o = crate::hostedcheck::run(d, &spec, &grant).ok()?;
        drop(grant);
        // Only the tests asked for count: a run that named others proves nothing about them.
        let failing: Vec<String> = crate::rerun::failing_names(&o.full)
            .into_iter()
            .filter(|f| tests.contains(f))
            .collect();
        // A run that did not run the tests, or that failed without naming one, proves nothing
        // about them either way.
        if o.passed {
            return crate::rerun::ran_them(&o.full, tests).then(Vec::new);
        }
        (!failing.is_empty()).then_some(failing)
    })();
    {
        let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
        git::remove_worktree(&p.path, &wt);
    }
    result
}

/// One urgent task for a test that is red on develop at `tip`, unless one is open.
fn file_once(d: &Arc<Daemon>, pid: mason_core::ProjectId, suite: &str, tip: &str, test: &str) {
    let _g = crate::basecheck::FILING
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let error = format!("test {test} fails");
    if let Ok(w) = d.world() {
        if w.tasks.values().any(|t| {
            t.project == pid
                && !t.state.is_terminal()
                && mason_testing::basered::is_title_for(&t.title, suite, &error)
        }) {
            return;
        }
    }
    let body = format!(
        "Test `{test}` of suite `{suite}` fails on develop at `{tip}`. It failed in the checks \
         of {MIN_TASKS} or more tasks within six hours and fails when run alone at the tip, so \
         it is red on develop and not those tasks' change.\n\nTasks that meet it are told \"red \
         on develop\" in their check results.\n\nFiled by the red-develop watch (#1023)."
    );
    let _ = d.emit(EventKind::TaskFiled {
        project: pid,
        title: mason_testing::basered::title(suite, short(tip), &error),
        body,
        priority: Priority::Urgent,
        trust: TrustSource::Owner,
        explored: false,
        proposed: false,
        blockers: vec![],
        not_before: None,
    });
}

/// Of a baseline's red entries, the ones that can be a test name. A broken build is recorded
/// under pseudo-tests (`build`, `** BUILD FAILED **`, a compiler line such as
/// `/x/Y.h:15:10: fatal error: 'z.h' file not found`): none of them is something a narrowed run
/// can run, so none is probed.
pub fn probeable(red: &[String]) -> Vec<String> {
    red.iter()
        .filter(|t| {
            !crate::baselift::is_pseudo(t)
                && mason_testing::gates::is_test_name(t)
                && !t.contains("error:")
                && !t.contains("fatal error")
                && !t.contains("BUILD FAILED")
                && !has_location(t)
        })
        .cloned()
        .collect()
}

/// Does `t` carry a `:<line>:` source location (`file.h:15:10:`, `file.rs:3:`)?
fn has_location(t: &str) -> bool {
    let parts: Vec<&str> = t.split(':').collect();
    parts.windows(2).skip(1).any(|w| {
        !w[0].is_empty()
            && w[0].bytes().all(|c| c.is_ascii_digit())
            && (w[1].is_empty()
                || w[1].starts_with(' ')
                || w[1].bytes().all(|c| c.is_ascii_digit()))
    })
}

/// [`probe`], at most once for each (project, suite, tip, tests): the attempt is recorded
/// whatever it found, so a run that could say nothing is not repeated until the tip moves.
fn probe_once(
    d: &Arc<Daemon>,
    pid: mason_core::ProjectId,
    suite: &mason_testing::Suite,
    tip: &str,
    tests: &[String],
    now: u64,
) -> Option<Vec<String>> {
    let project = d.project(pid)?.id.clone();
    if d.store
        .red_probed(&project, &suite.name, tip, tests)
        .unwrap_or(false)
    {
        return None;
    }
    let r = probe(d, pid, suite, tip, tests);
    let _ = d
        .store
        .put_red_probe(&project, &suite.name, tip, tests, now);
    r
}

/// One pass over every project.
pub fn pass(d: &Arc<Daemon>) {
    pass_at(d, now_ms());
}

/// [`pass`] on a given clock.
pub fn pass_at(d: &Arc<Daemon>, now: u64) {
    for (pid, p) in d.projects() {
        let Ok(tip) = git::rev_parse(&p.path, &p.base_ref()) else {
            continue;
        };
        let def = d.current_def(pid);
        let suites: Vec<mason_testing::Suite> = def
            .testing
            .bound(mason_testing::When::Development)
            .into_iter()
            .cloned()
            .collect();
        // 1 and 2: a stale red baseline, and the tasks it held up.
        for suite in &suites {
            let Some((base, red)) = d.store.latest_baseline(&suite.name).ok().flatten() else {
                continue;
            };
            // Baselines are keyed by suite name, not project: one from another repository (its
            // base is not a commit of this one) is not this project's.
            if red.is_empty() || base == tip || !git::is_ancestor(&p.path, &base, &tip) {
                continue;
            }
            let real = probeable(&red);
            if real.is_empty() {
                continue;
            }
            let Some(failing) = probe_once(d, pid, suite, &tip, &real, now) else {
                continue;
            };
            let _ = d
                .store
                .put_partial_baseline(&tip, &suite.name, &failing, &real, now);
            let ok = fixed(&real, &failing);
            if !ok.is_empty() {
                deliver(d, pid, &suite.name, &ok, (&base, &tip), None, now);
            }
        }
        // 3: the same unexplained failure in several tasks.
        let sightings = d
            .store
            .red_sightings_since(now.saturating_sub(WINDOW_MS))
            .unwrap_or_default();
        let known = |suite: &str, test: &str| {
            crate::knownred::known_for(&d.store, suite)
                .iter()
                .any(|k| k.test == test)
        };
        let explained = |suite: &str, test: &str| {
            known(suite, test)
                || d.store
                    .latest_baseline(suite)
                    .ok()
                    .flatten()
                    .is_some_and(|(_, red)| red.iter().any(|t| t == test))
        };
        for s in suspects(&sightings, now, &explained) {
            let Some(suite) = suites.iter().find(|x| x.name == s.suite) else {
                continue;
            };
            // Already looked at at this tip.
            if d.store
                .partial_baseline(&tip, &s.suite)
                .ok()
                .flatten()
                .is_some_and(|(_, tested)| tested.contains(&s.test))
            {
                continue;
            }
            let one = [s.test.clone()];
            if probeable(&one).is_empty() {
                continue;
            }
            let Some(failing) = probe_once(d, pid, suite, &tip, &one, now) else {
                continue;
            };
            let _ = d
                .store
                .put_partial_baseline(&tip, &s.suite, &failing, &one, now);
            if failing.contains(&s.test) {
                file_once(d, pid, &s.suite, &tip, &s.test);
            } else {
                deliver(d, pid, &s.suite, &one, ("", &tip), Some(&s.tasks), now);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }
    fn sight(task: u64, test: &str, at: u64) -> (u64, String, String, u64) {
        (task, "cargo".into(), test.into(), at)
    }

    #[test]
    fn only_real_test_names_are_probed() {
        let red = s(&[
            "a::b::works",
            "build",
            "** BUILD FAILED **",
            "/Volumes/x/SIFTDetector.h:15:10: fatal error: 'opencv2/core.hpp' file not found",
            "src/View.swift:42",
            "error: linking with `cc` failed",
            "-[Suite testThing]",
        ]);
        assert_eq!(probeable(&red), s(&["a::b::works", "-[Suite testThing]"]));
        assert!(probeable(&s(&["build", "** BUILD FAILED **"])).is_empty());
    }

    #[test]
    fn a_test_that_passes_again_is_fixed() {
        assert_eq!(fixed(&s(&["a", "b"]), &s(&["b"])), s(&["a"]));
        assert!(fixed(&s(&["a"]), &s(&["a"])).is_empty());
    }

    #[test]
    fn the_notes_say_what_passes_where_and_what_to_do() {
        let m = merged_note(&s(&["a::t"]), "aaaaaaaaaaaaaaaa", "bbbbbbbbbbbbbbbb");
        assert!(m.contains("a::t red on develop at aaaaaaaaaa; it passes at bbbbbbbbbb"));
        assert!(m.contains("develop is merged into your branch: run your check again."));
        let c = refused_note(&s(&["a::t"]), &BaseFold::Conflicts(3, "x.rs\ny.rs".into()));
        assert!(c.contains("develop has fixed a::t, which your branch still fails;"));
        assert!(c.contains(
            "conflicts in x.rs, y.rs: merge develop and resolve them before anything else."
        ));
        assert!(refused_note(&s(&["a"]), &BaseFold::Uncommitted(1)).contains("uncommitted"));
    }

    #[test]
    fn three_tasks_in_the_window_make_a_suspect_and_two_do_not() {
        let now = WINDOW_MS + 10;
        let sights = vec![
            sight(1, "u", now - 5),
            sight(2, "u", now - 5),
            sight(3, "u", now - 5),
            sight(1, "v", now - 5),
            sight(2, "v", now - 5),
        ];
        let r = suspects(&sights, now, &|_, _| false);
        assert_eq!(r.len(), 1);
        assert_eq!(
            (r[0].test.as_str(), r[0].tasks.clone()),
            ("u", vec![1, 2, 3])
        );
        // One sighting is outside the window.
        let old = vec![
            sight(1, "u", 1),
            sight(2, "u", now - 5),
            sight(3, "u", now - 5),
        ];
        assert!(suspects(&old, now, &|_, _| false).is_empty());
        // The same task twice is one task.
        let same = vec![sight(1, "u", now), sight(1, "u", now), sight(2, "u", now)];
        assert!(suspects(&same, now, &|_, _| false).is_empty());
    }

    #[test]
    fn a_known_or_baseline_red_test_is_never_a_suspect() {
        let now = 100;
        let sights = vec![sight(1, "u", now), sight(2, "u", now), sight(3, "u", now)];
        assert!(suspects(&sights, now, &|_, t| t == "u").is_empty());
    }

    #[test]
    fn the_home_line_counts_tasks_and_names_tests() {
        assert!(held_text(&[]).is_none());
        let h = vec![
            (1, "a".to_string()),
            (2, "a".to_string()),
            (2, "b".to_string()),
        ];
        assert_eq!(
            held_text(&h).unwrap(),
            "2 tasks held by a red develop (a, b)"
        );
        assert!(held_text(&h[..1]).unwrap().starts_with("1 task held"));
    }

    #[test]
    fn the_alert_waits_for_the_delay_on_the_clock_it_is_given() {
        let pending = [(7, 1_000)];
        assert!(overdue(&pending, 1_000 + ALERT_AFTER_MS - 1).is_empty());
        assert_eq!(overdue(&pending, 1_000 + ALERT_AFTER_MS), vec![7]);
    }
}
