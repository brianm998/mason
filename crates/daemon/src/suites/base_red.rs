// SPDX-License-Identifier: GPL-3.0-or-later

//! A red the base has too (s246, decision 0241).
//!
//! When a suite is red at the test step in a way that names no failing test (its build failed,
//! its run proved nothing, or it was red with no test names), the same suite is looked at at the
//! merge base, once per (base commit, suite). If the base is red the same way, the change is not
//! to blame: the task is not sent back and no round is spent. It waits in its test step with the
//! wait class `base-red`, one urgent task is filed for the base, and when the base branch moves
//! and the suite is green there the new base is folded in and the step runs again.
//!
//! The wait is recorded in the `base_red` table, so a restarted daemon finds it again without
//! running the suite a second time.

use super::*;
use mason_store::{BaseProfile, BaseRedWait};
use mason_testing::basered::{same_red, BaseSide};
use std::sync::Mutex;

/// One base run at a time, and one filing at a time: a second task with the same base waits,
/// then finds the first one's cached result and the first one's task.
static BASE_RUN: Mutex<()> = Mutex::new(());

/// How a suite stands against its base.
pub(super) enum Gate {
    /// The base is not red the same way: the task's red stands.
    NotBase,
    /// The base is green again: the base is folded in and the step runs again.
    Cleared,
    /// The task went away while it waited.
    Gone,
    /// The owner skipped the suite while it waited.
    Skipped,
}

fn side(p: &BaseProfile) -> BaseSide {
    BaseSide {
        build_error: p.build_error.clone(),
        vacuous: p.vacuous,
        red: p.red,
        failing: p.failing.clone(),
        first_error: p.first_error.clone(),
    }
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(10)]
}

/// What the build or the run says, without the header the step wrapped around it.
pub(super) fn output_of(feedback: &str) -> &str {
    feedback
        .split_once("Last output:\n")
        .map_or(feedback, |(_, o)| o)
}

/// The suite at `base`: from the cache, or by building and running it in a throwaway worktree.
/// `None` when it could not be told (no device, the run was stopped).
fn profile(
    c: &Ctx,
    who: &Who,
    lock: &crate::daemon::RepoLock,
    suite: &Suite,
    base: &str,
) -> Option<BaseProfile> {
    let d = &who.d;
    let cached = || d.store.baseline_profile(base, &suite.name).ok().flatten();
    if let Some(p) = cached() {
        return Some(p);
    }
    let _one = BASE_RUN.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(p) = cached() {
        return Some(p);
    }
    let dev = lease_dev(who, &c.def, suite).ok()?;
    let wt = who.log_dir.join(format!("base-{}", suite.name));
    {
        let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
        git::remove_worktree(&c.project.path, &wt);
        git::run(
            &c.project.path,
            &["worktree", "add", "--detach", &wt.to_string_lossy(), base],
        )
        .ok()?;
    }
    let mut who = who.clone();
    let result = (|| {
        if let Some(b) = &suite.build {
            let needs = needs_of(&crate::jobs::build_uses(), None);
            who.env = held_env(c, &needs);
            if !lease(c, needs) {
                return None;
            }
            // Host ids are per command: one for each base, or a second base would get the first's result.
            let n = 100 + usize::from_str_radix(&base[..base.len().min(6)], 16).unwrap_or(0);
            let ran = run_build(&who, suite, b, n, &wt);
            c.release_lease();
            match ran {
                Built::Gone => return None,
                Built::Red(fb) => {
                    return Some(BaseProfile {
                        build_error: Some(basered::first_error(output_of(&fb))),
                        ..Default::default()
                    })
                }
                Built::Ok => {}
            }
        }
        let needs = run_needs(&c.def, &c.step, suite);
        who.env = held_env(c, &needs);
        if !lease(c, needs) {
            return None;
        }
        let out = (|| {
            env_up(&who, &c.def, suite, &wt, dev.as_ref()).ok()?;
            if let Some(f) = &suite.results_file {
                let _ = std::fs::remove_file(wt.join(f));
            }
            let ran = run_hosted(
                &who.d,
                &format!("{}-bp{}-{}", who.id_prefix, short(base), suite.name),
                &crate::jobs::pty_wrap(
                    suite.pty,
                    &lifecycle::with_device(&suite.command, dev_id(dev.as_ref())),
                ),
                &wt,
                suite.timeout.unwrap_or(Duration::from_secs(1800)),
                &who.log_dir,
                who.check(&suite.name),
                who.run(),
                None,
                who.thresholds(),
                who.env.clone(),
            )
            .ok()?;
            if ran.stop != Stop::No {
                return None;
            }
            let j = judge_ran(suite, &ran, &wt);
            let (vacuous, red) = match &j.verdict {
                Verdict::Green => (false, false),
                Verdict::Vacuous { .. } => (true, false),
                Verdict::Red { .. } => (false, true),
            };
            let first_error = if red {
                let e = basered::first_error(&j.output);
                match (&j.verdict, e.is_empty()) {
                    (Verdict::Red { why, .. }, true) => why.clone(),
                    _ => e,
                }
            } else {
                String::new()
            };
            Some(BaseProfile {
                failing: j.results.failed_names(),
                names: j.results.names(),
                executed: j.results.executed(),
                build_error: None,
                vacuous,
                red,
                first_error,
            })
        })();
        env_down(&who, &c.def, suite, &wt, dev.as_ref());
        c.release_lease();
        out
    })();
    {
        let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
        git::remove_worktree(&c.project.path, &wt);
    }
    let p = result?;
    let _ = d
        .store
        .put_baseline_profile(base, &suite.name, &p, now_ms());
    Some(p)
}

/// The open task or tracker issue that already covers this red, if any.
fn existing_filing(c: &Ctx, suite: &str, error: &str) -> Option<crate::basecheck::Fix> {
    use crate::basecheck::Fix;
    if let Ok(w) = c.d.world() {
        let mut open: Vec<&Task> = w
            .tasks
            .values()
            .filter(|t| t.project == c.project_id && !t.state.is_terminal())
            .filter(|t| basered::is_title_for(&t.title, suite, error))
            .collect();
        open.sort_by_key(|t| t.id.0);
        if let Some(t) = open.first() {
            return Some(Fix::Task(t.id.0));
        }
    }
    let norm = basered::normalise(error);
    if norm.is_empty() {
        return None;
    }
    c.d.store
        .tracker_items(&c.project.id)
        .unwrap_or_default()
        .into_iter()
        .find(|it| it.open && it.title.contains(&format!("`{suite}` is red")))
        .map(|it| Fix::Issue(it.number))
}

/// One urgent task for this red on the base, unless one is already open.
fn file_once(
    c: &Ctx,
    suite: &str,
    base: &str,
    kind: RedKind,
    error: &str,
) -> Option<crate::basecheck::Fix> {
    use crate::basecheck::Fix;
    let _g = crate::basecheck::FILING
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(f) = existing_filing(c, suite, error) {
        return Some(f);
    }
    let what = match kind {
        RedKind::Build => "its build fails",
        RedKind::Vacuous => "its run proves nothing (too few tests run)",
        RedKind::Nameless => "its run is red and names no failing test",
    };
    let body = format!(
        "Suite `{suite}` is red on `{base}` (the merge base of task #{}): {what}.\n\n\
         First error:\n```\n{error}\n```\n\n\
         Tasks that meet the same red wait in their test step (wait class `base-red`) and are \
         not sent back; they fold in the base and run the step again once the suite is green \
         there.\n\nFiled by the test step's base check.",
        c.task.0
    );
    let a =
        c.d.emit(EventKind::TaskFiled {
            project: c.project_id,
            title: basered::title(suite, short(base), error),
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

/// Compare a red with the base. If the base is red the same way, wait for it to be fixed.
/// `resume` is the wait a restarted daemon found recorded: no new filing, no new comparison.
#[allow(clippy::too_many_arguments)]
pub(super) fn gate(
    c: &Ctx,
    who: &Who,
    lock: &crate::daemon::RepoLock,
    suite: &Suite,
    tree_base: &str,
    kind: RedKind,
    first_error: &str,
    resume: Option<BaseRedWait>,
) -> Gate {
    let d = &c.d;
    let mut seen = tree_base.to_string();
    let text;
    let fix;
    if let Some(w) = &resume {
        seen = w.base.clone();
        text = w.text.clone();
        fix = None;
    } else {
        let Some(p) = profile(c, who, lock, suite, tree_base) else {
            return Gate::NotBase;
        };
        if !same_red(kind, first_error, &side(&p)) {
            return Gate::NotBase;
        }
        fix = file_once(c, &suite.name, tree_base, kind, first_error);
        text = format!(
            "`{}` is red on {} too: {}",
            suite.name,
            short(tree_base),
            match kind {
                RedKind::Vacuous => "it runs no tests".to_string(),
                _ => first_error.split_whitespace().collect::<Vec<_>>().join(" "),
            }
        );
        let _ = d.store.put_base_red(&BaseRedWait {
            task: c.task.0,
            suite: suite.name.clone(),
            kind: kind.name().into(),
            first_error: first_error.into(),
            base: tree_base.into(),
            text: text.clone(),
            at_ms: now_ms(),
        });
    }
    // The step holds nothing while it waits.
    c.release_lease();
    let wait_text = match &fix {
        Some(f) => format!("{text} (fix: {})", f.text()),
        None => text.clone(),
    };
    let waiting = c.emit(EventKind::WaitStarted {
        task: c.task,
        on: wait_text,
        class: Some("base-red".into()),
    });
    let end = |waiting: bool| {
        if waiting {
            c.emit(EventKind::WaitEnded { task: c.task });
        }
    };
    let check = who.check(&suite.name);
    let mut first = resume.is_some();
    loop {
        match check() {
            Stop::Gone => return Gate::Gone,
            Stop::Skipped => {
                end(waiting);
                let _ = d.store.clear_base_red(c.task.0);
                return Gate::Skipped;
            }
            Stop::No => {}
        }
        if !first {
            std::thread::sleep(d.cfg.base_red_poll);
        }
        first = false;
        let _ = d.fetch_remote(c.project_id);
        let Ok(tip) = git::rev_parse(&c.project.path, &c.project.base_ref()) else {
            continue;
        };
        if tip == seen {
            continue;
        }
        seen = tip.clone();
        let Some(p) = profile(c, who, lock, suite, &tip) else {
            continue;
        };
        if side(&p).green() {
            end(waiting);
            let _ = d.store.clear_base_red(c.task.0);
            let line = format!(
                "`{}` was red on {} too ({}); {} is green, so the step ran again with it folded in{}",
                suite.name,
                short(tree_base),
                text.rsplit_once(": ").map_or(&*text, |(_, e)| e),
                short(&tip),
                fix.as_ref()
                    .map_or(String::new(), |f| format!(" (fix: {})", f.text()))
            );
            let _ = std::fs::write(
                c.task_dir().join(format!("basered-r{}.excused", c.run.0)),
                line,
            );
            return Gate::Cleared;
        }
        // Still red at the new base: keep waiting, now against it.
        let _ = d.store.put_base_red(&BaseRedWait {
            task: c.task.0,
            suite: suite.name.clone(),
            kind: kind.name().into(),
            first_error: first_error.into(),
            base: tip,
            text: text.clone(),
            at_ms: now_ms(),
        });
    }
}
