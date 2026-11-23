// SPDX-License-Identifier: GPL-3.0-or-later

//! Test steps that run suites (Part 8). For each suite the change needs, in
//! order: reuse green evidence for the exact tree if there is some; bring the
//! environment up; run the command; read its output with the suite's parser;
//! judge it (green requires proof); and classify a red before blaming the task
//! (environmental, flaky, baseline, quarantined, or product). Then the optional
//! gates, and the closing verdict.
//!
//! Skipping by force works at any moment, including mid-run: the suite is
//! stopped, the lanes already green keep their results, and a debt is recorded.

use crate::daemon::{now_ms, Daemon};
use crate::jobs::{run_shell, tail, Ctx};
use crate::progress::FileTail;
use crate::sched::needs_of;
use crate::watch::Thresholds;
use mason_core::progress::{Change, Figure, Report, Source, SubProgress};
use mason_core::*;
use mason_definition::graph::Outcome;
use mason_definition::{Definition, Step};
use mason_landing::git;
use mason_scheduler::Needs;
use mason_step_host::client::{collect_to_file, collect_to_file_observed, ClientError, HostClient};
use mason_step_host::launcher::{spawn_host, SpawnOptions};
use mason_step_host::protocol::{CURRENT, PREVIOUS};
use mason_testing::basered::{self, RedKind};
use mason_testing::env as lifecycle;
use mason_testing::gates;
use mason_testing::parse::parse;
use mason_testing::verdict::{classify, closing, judge, Inputs};
use mason_testing::LiveCount;
use mason_testing::{
    Closing, RedClass, Results, Selection, Signature, Suite, SuiteOutcome, Verdict, When,
};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// ---- which suites a change needs -------------------------------------------

pub struct Sel {
    pub suite: Suite,
    pub selected: bool,
    pub why: String,
}

/// Paths the task's branch changes against its base.
pub(crate) fn changed_paths(repo: &Path, base_ref: &str, branch: &str) -> Option<Vec<String>> {
    let out = git::run(
        repo,
        &["diff", "--name-only", &format!("{base_ref}...{branch}")],
    )
    .ok()?;
    Some(
        out.lines()
            .map(String::from)
            .filter(|l| !l.is_empty())
            .collect(),
    )
}

/// Paths the worktree differs in from the merge base with `base_ref`: committed,
/// uncommitted and new files, so a `run_check` sees work not yet committed.
pub(crate) fn worktree_paths(wt: &Path, base_ref: &str) -> Option<Vec<String>> {
    let base = git::run(wt, &["merge-base", base_ref, "HEAD"]).ok()?;
    let tracked = git::run(wt, &["diff", "--name-only", base.trim()]).ok()?;
    let new = git::run(wt, &["ls-files", "--others", "--exclude-standard"]).unwrap_or_default();
    let mut v: Vec<String> = tracked
        .lines()
        .chain(new.lines())
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect();
    v.sort();
    v.dedup();
    Some(v)
}

/// The suites of a test step, and which of them this change needs: the impact
/// map says (an unmapped path runs everything) and the bindings must put the
/// suite's level in the test step.
pub fn plan(
    project: &crate::config::ProjectCfg,
    def: &Definition,
    step: &Step,
    branch: &str,
) -> Vec<Sel> {
    let paths = changed_paths(&project.path, &project.base_ref(), branch);
    plan_for(def, step, paths, When::TestStep)
}

/// `plan` for any moment (`run_check` plans for `When::Development`) and a given change.
/// `paths` of `None` (unreadable) runs everything that is bound.
pub fn plan_for(def: &Definition, step: &Step, paths: Option<Vec<String>>, when: When) -> Vec<Sel> {
    let t = &def.testing;
    let paths_for_why = paths.clone();
    let selection = match paths {
        Some(paths) => t.impact.select(&paths, &step.suites),
        None => Selection::All,
    };
    let bound: Vec<String> = t.bound(when).iter().map(|s| s.name.clone()).collect();
    let why_not = match when {
        When::Development => "its level is not bound to development",
        _ => "its level is not bound to the test step",
    };
    let unmapped_why: String = match paths_for_why
        .as_deref()
        .and_then(|p| t.impact.first_unmapped(p))
    {
        Some(f) if matches!(selection, Selection::All) => {
            format!("an unmapped path runs everything ({f})")
        }
        _ => "an unmapped path runs everything".to_string(),
    };
    step.suites
        .iter()
        .filter_map(|n| t.suite(n))
        .map(|s| {
            let (selected, why) = match &selection {
                _ if !bound.contains(&s.name) => (false, why_not.to_string()),
                Selection::All => (true, unmapped_why.clone()),
                Selection::Some(v) if v.contains(&s.name) => {
                    (true, "the impact map selects it".to_string())
                }
                Selection::Some(_) => (false, "the impact map does not select it".to_string()),
            };
            Sel {
                suite: s.clone(),
                selected,
                why,
            }
        })
        .collect()
}

/// What a suite holds while its command runs: its own `uses` and those of its environment.
pub(crate) fn suite_uses(
    def: &Definition,
    suite: &Suite,
) -> std::collections::BTreeMap<String, u32> {
    let mut uses = suite.uses.clone();
    if let Some(e) = def.testing.environment(&suite.env) {
        // A device environment leases one device of its kind.
        if let Some(kind) = crate::targets::kind_of_env(e) {
            let u = uses.entry(kind.capability()).or_insert(0);
            *u = (*u).max(1);
        }
        for (k, v) in &e.uses {
            let u = uses.entry(k.clone()).or_insert(0);
            *u = (*u).max(*v);
        }
    }
    uses
}

/// Whether the step queues suite by suite instead of holding everything for the whole step:
/// some selected suite builds first, or needs the whole machine (s340), or two or more only
/// build and can run side by side (s345). A step-wide lease with `exclusive` in it would keep
/// every other build and test off the machine for the whole step, though only the exclusive
/// suites need that.
pub fn builds_per_suite(
    project: &crate::config::ProjectCfg,
    def: &Definition,
    step: &Step,
    branch: &str,
) -> bool {
    let chosen: Vec<Suite> = plan(project, def, step, branch)
        .into_iter()
        .filter(|s| s.selected)
        .map(|s| s.suite)
        .collect();
    queues_per_suite(def, step, &chosen)
}

/// [`builds_per_suite`] over the selected suites.
fn queues_per_suite(def: &Definition, step: &Step, chosen: &[Suite]) -> bool {
    chosen
        .iter()
        .any(|s| s.build.is_some() || run_needs(def, step, s).exclusive)
        || chosen.iter().filter(|s| compile_only(def, step, s)).count() > 1
}

/// What one suite's run holds in a step that queues suite by suite: the step's `uses` and the
/// suite's own. The step's `compile` is dropped for every suite: one that built first has had
/// its compile slot already, and one with no build does not compile, so holding `compile` for it
/// only reserved slots the compile queue needed (s471). A suite that does compile without a
/// build of its own says so in its own `uses` (`compile = 1`).
pub(crate) fn run_needs(def: &Definition, step: &Step, suite: &Suite) -> Needs {
    let mut uses = crate::jobs::tests_uses(&step.uses);
    for (k, v) in suite_uses(def, suite) {
        let have = uses.entry(k).or_insert(0);
        *have = (*have).max(v);
    }
    needs_of(&uses, None)
}

/// Everything a test step with suites holds while it runs: the step's own
/// `uses` plus those of each selected suite and its environment.
pub fn needs_for(
    project: &crate::config::ProjectCfg,
    def: &Definition,
    step: &Step,
    branch: &str,
) -> Needs {
    let mut n = needs_of(&step.uses, None);
    for s in plan(project, def, step, branch)
        .into_iter()
        .filter(|s| s.selected)
    {
        for (k, v) in suite_uses(def, &s.suite) {
            let have = n.amounts.entry(k).or_insert(0);
            *have = (*have).max(v);
        }
    }
    n
}

/// The files a suite's `results_file` names under `cwd`: the path itself, or, when its last
/// segment has a wildcard (`build/test-results/test/*.xml`, one file per test class as
/// Gradle writes them), the matching files in that directory, sorted (s286). A wildcard in
/// an earlier segment is not expanded.
pub(crate) fn results_paths(cwd: &Path, f: &str) -> Vec<PathBuf> {
    let (dir, name) = f.rsplit_once('/').unwrap_or(("", f));
    if !name.contains(['*', '?']) {
        return vec![cwd.join(f)];
    }
    let mut out: Vec<PathBuf> = std::fs::read_dir(cwd.join(dir))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| mason_testing::glob::matches(name, &e.file_name().to_string_lossy()))
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .collect();
    out.sort();
    out
}

/// A suite's results, read before it judges them: every file `results_file` names, one
/// after another (the parsers read test cases from the text, so several files read as one).
pub(crate) fn read_results(cwd: &Path, f: &str) -> std::io::Result<String> {
    let paths = results_paths(cwd, f);
    if paths.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no file matches",
        ));
    }
    let mut text = String::new();
    for p in paths {
        text.push_str(&crate::jobs::read_lossy(&p)?);
        text.push('\n');
    }
    Ok(text)
}

/// Remove a suite's results before it runs, so a run that writes none is not judged on the
/// last run's (and a tool that skips an up-to-date task runs it again).
pub(crate) fn remove_results(cwd: &Path, f: &str) {
    for p in results_paths(cwd, f) {
        let _ = std::fs::remove_file(p);
    }
}

const PROJECT_TEST: &str = "{project_test}";

/// Whether any of the suite's commands names `{project_test}`.
fn uses_project_test(s: &Suite) -> bool {
    let uses = |c: &str| c.contains(PROJECT_TEST);
    uses(&s.command)
        || s.build.as_deref().is_some_and(uses)
        || s.rerun.as_deref().is_some_and(uses)
        || s.skip.as_deref().is_some_and(uses)
        || s.shard_command.as_deref().is_some_and(uses)
        || s.dev_shard_command.as_deref().is_some_and(uses)
        || s.dev_command.as_deref().is_some_and(uses)
        || s.dev_build.as_deref().is_some_and(uses)
}

/// A suite with the project's `test` command written in: `{project_test}` in its
/// `command`, `build` and `rerun` becomes that command, so the owner's per-worktree
/// build directory (set in the project file's `test`) applies to a suite too.
pub(crate) fn with_project_test(suite: &Suite, test: Option<&str>) -> Result<Suite, String> {
    const KEY: &str = PROJECT_TEST;
    let mut s = suite.clone();
    if !uses_project_test(&s) {
        return Ok(s);
    }
    let Some(t) = test else {
        return Err(format!(
            "suite `{}` uses {KEY}, but the project has no `test` command",
            suite.name
        ));
    };
    s.command = s.command.replace(KEY, t);
    s.build = s.build.map(|b| b.replace(KEY, t));
    s.rerun = s.rerun.map(|b| b.replace(KEY, t));
    s.skip = s.skip.map(|b| b.replace(KEY, t));
    s.shard_command = s.shard_command.map(|b| b.replace(KEY, t));
    // A development command lists packages itself (`-p a -p b`); `--workspace` would override
    // that list, so the project's test command goes in without it (its prefix, such as the
    // owner's CARGO_TARGET_DIR, stays).
    let narrow = t.replace(" --workspace", "");
    s.dev_command = s.dev_command.map(|b| b.replace(KEY, &narrow));
    s.dev_shard_command = s.dev_shard_command.map(|b| b.replace(KEY, &narrow));
    s.dev_build = s.dev_build.map(|b| b.replace(KEY, &narrow));
    Ok(s)
}

// ---- running commands ------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stop {
    No,
    /// A skip was requested.
    Skipped,
    /// The task or its step run is gone.
    Gone,
}

struct Ran {
    code: i32,
    signal: i32,
    output: String,
    stop: Stop,
    timed_out: bool,
    /// The stall watcher killed the tree: why.
    stalled: Option<String>,
    /// How long the command ran, from its host's own records (so a run collected late, after
    /// a daemon restart, is not counted as instant). `None` when they cannot be read.
    ran_ms: Option<u64>,
    /// It ran only some of the suite's tests (s386): its time is not the suite's.
    narrowed: bool,
    /// After a lost host, the finished executables of the lost run were kept and only the rest
    /// ran again (#621): what was reused, for the step's note. Its time is not the suite's either.
    resumed: Option<String>,
    /// It ran on GitHub Actions, not here (#697): where, for the step's note.
    ran_on: Option<String>,
}

/// Record how long a suite's build or run took, when it finished on its own (s385): the
/// scheduler's estimates come from these.
fn record_time(who: &Who, suite: &str, phase: &str, ran: &Ran) {
    if ran.stop != Stop::No || ran.timed_out || ran.stalled.is_some() || ran.narrowed {
        return;
    }
    if let Some(ms) = ran.ran_ms {
        let _ = who
            .d
            .store
            .record_suite_time(&who.project, suite, phase, ms, now_ms());
    }
}

/// Record each test's own time from a finished run, whole or narrowed (s634). Tests whose
/// output carried no time are left out.
fn record_test_times(who: &Who, suite: &str, results: &Results) {
    let times: Vec<(String, u64)> = results
        .cases
        .iter()
        .filter_map(|c| c.ms.map(|ms| (c.name.clone(), ms)))
        .collect();
    let _ = who
        .d
        .store
        .record_test_times(&who.project, suite, &times, now_ms());
}

/// Record each test executable's own time from a finished cargo run's log, whole or narrowed
/// (s649).
fn record_target_times(who: &Who, suite: &str, output: &str) {
    let times = crate::targettimes::parse_log(output);
    let _ = who
        .d
        .store
        .record_target_times(&who.project, suite, &times, now_ms());
}

/// The estimate in ms when a narrowed run of `groups` is small enough for the `test-small`
/// lane: the definition declares it and every selected target has a recorded median whose sum
/// is under the project's `small_run_s` (s649).
fn small_run(who: &Who, c: &Ctx, suite: &Suite, groups: &[String]) -> Option<u64> {
    if !c.def.resources.contains_key(crate::jobs::TEST_SMALL) {
        return None;
    }
    let medians = who.d.store.target_medians(&who.project, &suite.name).ok()?;
    crate::targettimes::small_estimate_ms(groups, &medians, c.project.small_run_s)
}

/// What a build or run of `suite` is expected to take at most, from its recorded times (s385).
fn estimate(who: &Who, suite: &str, phase: &str) -> Option<u64> {
    who.d
        .store
        .suite_estimate(&who.project, suite, phase)
        .ok()
        .flatten()
}

type Check = Arc<dyn Fn() -> Stop + Send + Sync>;

/// Run a suite command under a step host. If the host is lost before the command
/// finished, run it once more on a fresh host (`<id>-host1`) and judge that run; a
/// second loss in a row is an error naming both. A lost host is never a result.
#[allow(clippy::too_many_arguments)]
fn run_hosted(
    d: &Arc<Daemon>,
    project: &str,
    id: &str,
    cmd: &str,
    cwd: &Path,
    limit: Duration,
    log_dir: &Path,
    check: Check,
    run: Option<StepRunId>,
    watch: Option<&Suite>,
    th: Thresholds,
    env: Vec<(String, String)>,
    place: Option<(Option<TaskId>, &Suite)>,
    cover: &Cover,
) -> Result<Ran, String> {
    run_hosted_resuming(
        d, project, id, cmd, cwd, limit, log_dir, check, run, watch, th, env, None, place, cover,
    )
}

/// [`run_hosted`]; with a `resume` template (`resume::template`), a lost host's finished cargo
/// test executables are kept and only the others run on the fresh host, the verdict reading
/// both (#621). A second loss in a row still fails the step.
#[allow(clippy::too_many_arguments)]
fn run_hosted_resuming(
    d: &Arc<Daemon>,
    project: &str,
    id: &str,
    cmd: &str,
    cwd: &Path,
    limit: Duration,
    log_dir: &Path,
    check: Check,
    run: Option<StepRunId>,
    watch: Option<&Suite>,
    th: Thresholds,
    env: Vec<(String, String)>,
    resume: Option<&str>,
    place: Option<(Option<TaskId>, &Suite)>,
    cover: &Cover,
) -> Result<Ran, String> {
    use crate::hostloss as hl;
    let covered = cover.covered;
    // Where the run goes (#697, #698). Only a task's test step asks (`place`); a landing
    // batch, a base run and a debt stay here until #702. A remote run is returned as the same
    // `Ran`, so it is judged by the code below this call, not by it. A suite whose build runs
    // as its own step here has none when it goes remote: the build comes first in the command.
    // Held until this function returns, so for the whole local run.
    let mut fallback: Option<crate::sched::Lease> = None;
    if let crate::remote::Executor::GitHub { runs_on } =
        crate::remote::executor_for(d, project, crate::remote::Kind::Suite, place)
    {
        let build = place.and_then(|(_, s)| s.build.as_deref());
        if let Some(ran) = run_remote(
            d, project, id, build, cmd, cwd, limit, log_dir, &check, run, watch, &env, &runs_on,
            covered,
        )? {
            return Ok(ran);
        }
        // `fallback = "local"` sent it here. Its step holds runner pool units, not this
        // machine's slots (#761), so it takes what the definition's test step `uses` before it
        // starts, under the same limits and CPU and thermal holds as a local test step.
        if !d.cfg.hosts_dir().join(id).join("host.kv").exists() {
            let uses = test_step_uses(d, project);
            let task = place.and_then(|(t, _)| t);
            let label = format!(
                "{} {id} (no GitHub runner, run here)",
                task.map_or_else(|| "suite".to_string(), |t| format!("#{}", t.0))
            );
            fallback = match (&cover.batch, covered) {
                // A landing batch's suite whose slots its lease does not hold takes them now,
                // ranked as the batch's own work (s943).
                (Some((owner, _)), false) => {
                    crate::remote::fallback_lease_for_batch(d, project, owner, &uses, &label)?
                }
                // `covered`: a landing batch holds this machine's slots for this very run, so
                // it does not ask again (s911, decision 0904).
                _ => crate::remote::fallback_lease_unless_covered(
                    d, project, task, &uses, &label, covered,
                )?,
            };
        }
    } else if let (Some((owner, needs)), false) = (&cover.batch, covered) {
        // Placed here, though the batch's lease holds nothing for this suite (it was bound for
        // GitHub when the batch took its slots, or the run is a baseline or a build that never
        // goes there): it takes its slots as the batch before it starts (s943).
        if !d.cfg.hosts_dir().join(id).join("host.kv").exists() {
            fallback = crate::remote::batch_lease(
                d,
                project,
                owner,
                needs.clone(),
                &format!("{id} (batch, run here)"),
            )?;
        }
    }
    let _fallback = fallback;
    let hosts = d.cfg.hosts_dir();
    let again_id = hl::rerun_id(id);
    let mut first = None;
    let mut on = id.to_string();
    let mut cmd = cmd.to_string();
    let mut plan: Option<crate::resume::Plan> = None;
    // The finished part of the lost run, read from its log.
    let planned = |cmd: &mut String, plan: &mut Option<crate::resume::Plan>| {
        if let Some(t) = resume {
            *plan = crate::resume::plan_for(cwd, &log_dir.join(format!("{id}.log")));
            if let Some(p) = plan {
                *cmd = crate::resume::command(t, p);
            }
        }
    };
    // A progress file left by an earlier attempt says nothing about this one.
    if !hosts.join(id).join("host.kv").exists() && !hosts.join(&again_id).join("host.kv").exists() {
        if let Some(f) = watch.and_then(|w| w.progress.as_ref()) {
            remove_results(cwd, f);
        }
    }
    if hosts.join(&again_id).join("host.kv").exists() {
        first = mason_step_host::registry::lost_report(&hosts.join(id)).ok();
        on = again_id.clone();
        planned(&mut cmd, &mut plan);
    }
    loop {
        match run_hosted_once(
            d,
            &on,
            &cmd,
            cwd,
            limit,
            log_dir,
            check.clone(),
            run,
            watch,
            th,
            env.clone(),
        ) {
            Ok(mut r) => {
                if let Some(p) = plan.take() {
                    let (code, output) = crate::resume::combine(&p.reused, r.code, &r.output);
                    (r.code, r.output) = (code, output);
                    r.resumed = Some(p.note());
                }
                return Ok(r);
            }
            Err(HostedErr::Other(e)) => return Err(e),
            Err(HostedErr::Lost(rep)) if on == id => {
                hl::record(d, run, id, &rep, true);
                first = Some(*rep);
                on = again_id.clone();
                planned(&mut cmd, &mut plan);
            }
            Err(HostedErr::Lost(rep)) => {
                hl::record(d, run, id, &rep, false);
                return Err(match first {
                    Some(f) => hl::both(&f, &rep),
                    None => hl::lost_text(&rep),
                });
            }
        }
    }
}

/// What the definition's test step `uses`: the slots a run that falls back to this machine takes.
fn test_step_uses(d: &Arc<Daemon>, project: &str) -> std::collections::BTreeMap<String, u32> {
    d.project_id_by_name(project)
        .map(|p| d.current_def(p))
        .and_then(|def| {
            def.steps
                .iter()
                .find(|s| s.kind == mason_definition::StepType::Test)
                .map(|s| s.uses.clone())
        })
        .unwrap_or_default()
}

/// A suite command run on GitHub Actions. `covered`: the caller holds this machine's slots for
/// the run, so a fallback's room check does not count them against it. `Ok(None)`: it could not
/// go remote and the project's `fallback` is `local`, so the caller runs it here. An outage or a missing runner is an `Err`
/// that [`crate::remote::environmental`] recognizes.
#[allow(clippy::too_many_arguments)]
fn run_remote(
    d: &Arc<Daemon>,
    project: &str,
    id: &str,
    build: Option<&str>,
    cmd: &str,
    cwd: &Path,
    limit: Duration,
    log_dir: &Path,
    check: &Check,
    run: Option<StepRunId>,
    watch: Option<&Suite>,
    env: &[(String, String)],
    runs_on: &str,
    covered: bool,
) -> Result<Option<Ran>, String> {
    use crate::remote as rm;
    let task = run.and_then(|r| d.world().ok().and_then(|w| w.task_of_open_run(r)));
    // The slots a fallback takes when it is placed here: the same ones `run_one` asks for.
    let uses = test_step_uses(d, project);
    let Some(site) = rm::site_for_run(d, project, cwd, task.map(|t| t.0), &uses, covered) else {
        return Err(rm::env_err("the project has no [remote_runs] section"));
    };
    // A suite whose build runs as its own step here goes in `RunSpec.build`, which the runner
    // runs as a separate `sh -c` before the command: joined with `&&`, two leading `cd`s break.
    let mut spec = rm::with_site_env(
        rm::with_build(
            rm::spec_for(project, id, watch, cmd, limit, env, runs_on, ""),
            build,
        ),
        &site,
    );
    // The whole command of a suite that has shards is split across runners (#701); a narrowed
    // run, a run without the known-red tests and the like are not the whole suite and run on one.
    let sharded = watch.and_then(|s| {
        let sc = s.shard_command.as_deref()?;
        (s.shards > 1 && !s.narrowed && cmd == s.command).then_some((s, sc))
    });
    // A narrowed run split the same way (#880): its `dev_shard_command` was filled in with the
    // same pick as `command` (`dev_shard_for`); one still holding a `{...}` was not, and runs whole.
    let sharded = sharded.or_else(|| {
        let s = watch?;
        let sc = s.dev_shard_command.as_deref()?;
        (s.shards > 1 && s.narrowed && cmd == s.command && !sc.contains('{')).then_some((s, sc))
    });
    if let Some((s, sc)) = sharded {
        spec.shards = s.shards;
        spec.shard_command = Some(sc.to_string());
        spec.shard_lists = shard_lists(d, project, s);
    }
    let stop = || match check() {
        Stop::No => None,
        Stop::Skipped => Some(rm::StopWhy::Skipped),
        Stop::Gone => Some(rm::StopWhy::Gone),
    };
    // The units the run holds, on record until it ends, for a daemon that restarts (#700).
    let (key, label) = match task {
        Some(t) => (format!("task {}", t.0), format!("#{}", t.0)),
        None => (id.to_string(), id.to_string()),
    };
    let _hold = rm::HoldGuard::new(
        &d.cfg.hosts_dir(),
        &spec,
        &key,
        &label,
        spec.shards.max(1),
        log_dir,
        match task {
            Some(_) => "test_step",
            None if id.starts_with("debt") || id.starts_with("ledger") => "debt",
            None => "batch",
        },
        task.map_or(0, |t| t.0),
    );
    Ok(match rm::run_or_fallback(&site, &spec, log_dir, &stop)? {
        None => None,
        Some(rm::Outcome::Stopped(why)) => Some(Ran {
            code: -1,
            signal: 9,
            output: String::new(),
            stop: match why {
                rm::StopWhy::Skipped => Stop::Skipped,
                rm::StopWhy::Gone => Stop::Gone,
            },
            timed_out: false,
            stalled: None,
            ran_ms: None,
            narrowed: false,
            resumed: None,
            ran_on: None,
        }),
        Some(rm::Outcome::Done(f)) => Some(remote_ran(f, watch)),
    })
}

/// Which tests go to which shard, from the times and names this daemon has recorded for the
/// suite (#701). Nothing recorded gives empty lists: every test then runs on its hash shard.
fn shard_lists(d: &Arc<Daemon>, project: &str, suite: &Suite) -> Vec<Vec<String>> {
    let weights = d
        .store
        .test_medians(project, &suite.name)
        .unwrap_or_default();
    let mut names: std::collections::BTreeSet<String> = weights.keys().cloned().collect();
    names.extend(d.store.passing_names(&suite.name).unwrap_or_default());
    crate::shards::assign(&names, &weights, suite.shards)
}

/// `suite`'s `dev_shard_command` filled in with the pick `n`, when one runner's share of it can be
/// run by scripts/test-shard.py: the packages, or one whole test target. A pick of several groups
/// (a compound shell command) or one filtered by name stays on one runner, so `None` (#880).
pub fn dev_shard_for(suite: &Suite, n: &crate::scope::Narrow) -> Option<String> {
    use crate::scope::Narrow;
    let t = suite.dev_shard_command.as_deref()?;
    match n {
        Narrow::Targets(g) if g.len() != 1 || g[0].contains(" -- ") => None,
        _ => Some(crate::scope::expand(t, n, false)),
    }
}

/// A finished remote run as the `Ran` a step host would have given.
fn remote_ran(mut f: crate::remote::Finished, suite: Option<&Suite>) -> Ran {
    // The shards' outputs are one output, judged by the suite's own parser; a shard that ran none
    // of its tests is red even when the merged count passes `min_tests`.
    if !f.parts.is_empty() {
        let count = |t: &str| match suite {
            Some(s) if s.plugin_parser.is_none() => {
                parse(s.parser, t, s).map_or(0, |r| r.executed())
            }
            // A parser this cannot call cannot say how many ran.
            _ => 1,
        };
        (f.code, f.signal, f.output) = crate::shards::merge_judged(&f.parts, &count);
    }
    Ran {
        code: f.code,
        signal: f.signal,
        output: f.output,
        stop: Stop::No,
        timed_out: f.timed_out,
        stalled: None,
        ran_ms: f.ran_ms,
        narrowed: false,
        resumed: None,
        ran_on: Some(f.ran_on),
    }
}

enum HostedErr {
    /// Boxed: the report is large.
    Lost(Box<mason_step_host::registry::LostReport>),
    Other(String),
}

#[allow(clippy::too_many_arguments)]
fn run_hosted_once(
    d: &Arc<Daemon>,
    id: &str,
    cmd: &str,
    cwd: &Path,
    limit: Duration,
    log_dir: &Path,
    check: Check,
    run: Option<StepRunId>,
    watch: Option<&Suite>,
    th: Thresholds,
    env: Vec<(String, String)>,
) -> Result<Ran, HostedErr> {
    let hosts = d.cfg.hosts_dir();
    let dir = hosts.join(id);
    if !dir.join("host.kv").exists() {
        let o = SpawnOptions {
            host_bin: d.cfg.host_bin.clone(),
            root: hosts.clone(),
            id: id.to_string(),
            cmd: vec!["sh".into(), "-c".into(), cmd.to_string()],
            cwd: Some(cwd.to_path_buf()),
            env,
            clear_env: false,
            stdin: None,
            linger: Duration::from_secs(600),
            heartbeat: d.cfg.heartbeat,
        };
        spawn_host(&o).map_err(|e| HostedErr::Other(format!("cannot start `{cmd}`: {e}")))?;
    }
    let done = Arc::new(AtomicBool::new(false));
    let why = Arc::new(AtomicU8::new(0)); // 0 none, 1 timed out, 2 skipped, 3 gone
    {
        let (done, why, sock, hdir) = (done.clone(), why.clone(), dir.join("sock"), dir.clone());
        std::thread::spawn(move || {
            let end = Instant::now() + mason_step_host::registry::remaining_limit(limit, &hdir);
            loop {
                if done.load(Ordering::SeqCst) {
                    return;
                }
                let stop = if Instant::now() >= end {
                    why.store(1, Ordering::SeqCst);
                    true
                } else {
                    match check() {
                        Stop::No => false,
                        Stop::Skipped => {
                            why.store(2, Ordering::SeqCst);
                            true
                        }
                        Stop::Gone => {
                            why.store(3, Ordering::SeqCst);
                            true
                        }
                    }
                };
                if stop {
                    if let Ok(mut h) = HostClient::connect(&sock, PREVIOUS, CURRENT, "daemon") {
                        let _ = h.stop(1000);
                    }
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        });
    }
    let _ = std::fs::create_dir_all(log_dir);
    let log = log_dir.join(format!("{id}.log"));
    // The step's generic progress: lines and bytes of this suite's output as it arrives.
    let exit = log_dir.join(format!("{id}.exit"));
    // Silence, judged on the whole tree: continue what is stopped, kill what is stuck.
    let stall = crate::watch::Watch::start(d, id, log.clone(), th, {
        let d = d.clone();
        let (name, id) = (
            watch.map(|w| w.name.clone()).unwrap_or_default(),
            id.to_string(),
        );
        move |text| {
            if let Some(run) = run {
                let text = format!("{}: {text}", if name.is_empty() { &id } else { &name });
                d.progress_report(run, |p, now| p.set_message(&text, now));
            }
        }
    });
    let rec = match run {
        Some(run) => {
            if watch.is_some_and(|w| w.pty) {
                d.progress_terminal(run, id);
            }
            d.progress_catch_up(run, id, &log);
            // Live test counts, for progress only: the verdict is the full parse at the end.
            // After a restart the log is read again from its start, so the counts are exact.
            let mut live = watch.and_then(|w| LiveCount::new(w.parser, w));
            let name = watch.map(|w| w.name.clone()).unwrap_or_default();
            if let Some(l) = live.as_mut() {
                if let Ok(old) = std::fs::read(&log) {
                    l.feed(&old);
                    suite_counts(d, run, &name, l);
                }
            }
            let _tail = watch.and_then(|w| w.progress.as_ref()).map(|f| {
                let (d, name) = (d.clone(), name.clone());
                FileTail::start(cwd.join(f), move |r| suite_report(&d, run, &name, &r))
            });
            collect_to_file_observed(&dir, &log, &exit, &mut |off, b| {
                d.progress_observe(run, id, off, b);
                if let Some(l) = live.as_mut() {
                    l.feed(b);
                    suite_counts(d, run, &name, l);
                }
            })
        }
        None => collect_to_file(&dir, &log, &exit),
    };
    done.store(true, Ordering::SeqCst);
    let rec = match rec {
        Ok(r) => r,
        Err(ClientError::HostLost { .. }) => {
            return Err(match mason_step_host::registry::lost_report(&dir) {
                Ok(rep) => HostedErr::Lost(Box::new(rep)),
                Err(_) => HostedErr::Other("the command was lost (its host died)".into()),
            })
        }
        Err(e) => {
            return Err(HostedErr::Other(format!(
                "cannot collect the command's result: {e}"
            )))
        }
    };
    Ok(Ran {
        code: rec.code,
        signal: rec.signal,
        output: crate::jobs::read_lossy(&log).unwrap_or_default(),
        stop: match why.load(Ordering::SeqCst) {
            2 => Stop::Skipped,
            3 => Stop::Gone,
            _ => Stop::No,
        },
        // The timer may fire for a run that ended on its own before its limit: a restarted
        // daemon re-attaching to a finished host counts the limit from the host's start (s355).
        timed_out: why.load(Ordering::SeqCst) == 1
            && mason_step_host::registry::ran_past(limit, &dir),
        stalled: stall.as_ref().and_then(|w| w.stalled()),
        ran_ms: match (
            mason_step_host::registry::HostInfo::read(&dir),
            mason_step_host::registry::ExitRecord::read(&dir),
        ) {
            (Ok(h), Ok(e)) => Some(e.ended_ms.saturating_sub(h.started_ms)),
            _ => None,
        },
        narrowed: false,
        resumed: None,
        ran_on: None,
    })
}

/// A suite's live count, as its sub-count: passed out of what is expected, with the
/// failed and skipped in the message. A count the suite itself reports is kept.
fn suite_counts(d: &Arc<Daemon>, run: StepRunId, name: &str, l: &LiveCount) {
    let c = l.counts();
    d.progress_sub(run, name, None, |s| {
        if s.done.source != Source::Reported {
            s.unit = "tests".into();
            s.done = Figure::counted(c.passed);
        }
    });
    let note = format!("{name}: {} failed, {} skipped", c.failed, c.skipped);
    d.progress_report(run, |p, now| {
        if c.failed + c.skipped == 0 {
            return Change::None;
        }
        p.set_message(&note, now)
    });
}

/// A line of a suite's own progress file.
fn suite_report(d: &Arc<Daemon>, run: StepRunId, name: &str, r: &Report) {
    let msg = r.message.as_ref().map(|m| format!("{name}: {m}"));
    d.progress_sub(run, name, msg, |s| {
        if let Some(u) = &r.unit {
            s.unit = u.clone();
        } else if s.unit.is_empty() {
            s.unit = "tests".into();
        }
        if let Some(n) = r.done {
            s.done = Figure::reported(n);
        }
        if let Some(n) = r.total {
            s.total = Figure::reported(n);
        }
    });
}

/// The expected total of a suite: the last green run's count for this base, else the
/// declared minimum, else none.
fn expected_total(d: &Arc<Daemon>, project: &str, suite: &Suite, base: &str) -> Figure {
    match d.store.suite_green(project, &suite.name, base) {
        Ok(Some(n)) if n > 0 => Figure::expected(n as u64),
        _ if suite.min_tests > 0 => Figure::minimum(suite.min_tests as u64),
        _ => Figure::default(),
    }
}

/// A narrowed (or not-run) suite does not run the whole suite, so the whole suite's expected
/// size is no denominator for it: the line reads "N tests" until a run reports its own total.
fn forget_whole_total(s: &mut SubProgress) {
    if s.total.source != Source::Reported {
        s.total = Figure::default();
    }
}

fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Reads a suite's output with its parser plugin.
type PluginParse = dyn Fn(&str) -> Result<Results, String>;

/// What a finished attempt says, once parsed and judged.
struct Judged {
    results: Results,
    verdict: Verdict,
    output: String,
}

fn judge_ran(suite: &Suite, ran: &Ran, cwd: &Path) -> Judged {
    judge_ran_with(suite, ran, cwd, None)
}

/// [`judge_ran`], reading the output with `plugin` when the suite names a parser plugin
/// (`Who::plugin_parser`). The verdict is judged from the cases exactly as for a built-in parser.
fn judge_ran_with(suite: &Suite, ran: &Ran, cwd: &Path, plugin: Option<&PluginParse>) -> Judged {
    // A tree the stall watcher killed is red for that reason, whatever the partial
    // output parses to.
    if let Some(why) = &ran.stalled {
        return Judged {
            results: Results::default(),
            verdict: Verdict::Red {
                failures: vec![],
                why: why.clone(),
            },
            output: ran.output.clone(),
        };
    }
    let text = match &suite.results_file {
        Some(f) => match read_results(cwd, f) {
            Ok(t) => t,
            Err(e) => {
                return Judged {
                    results: Results::default(),
                    verdict: Verdict::Red {
                        failures: vec![],
                        why: format!("the results file `{f}` was not written ({e})"),
                    },
                    output: ran.output.clone(),
                }
            }
        },
        None => ran.output.clone(),
    };
    let parsed = match plugin {
        Some(f) => f(&text),
        None => parse(suite.parser, &text, suite),
    };
    match parsed {
        Ok(results) => {
            let exit = if ran.signal != 0 {
                Some(128 + ran.signal)
            } else {
                Some(ran.code)
            };
            let verdict = if ran.timed_out {
                Verdict::Red {
                    failures: results.failed_names(),
                    why: RAN_OUT_OF_TIME.into(),
                }
            } else {
                judge(&results, suite.min_tests, exit)
            };
            Judged {
                results,
                verdict,
                output: ran.output.clone(),
            }
        }
        Err(e) => Judged {
            results: Results::default(),
            verdict: Verdict::Red {
                failures: vec![],
                why: e,
            },
            output: ran.output.clone(),
        },
    }
}

// ---- one suite -------------------------------------------------------------

/// Who is running, for skips and cancellation.
#[derive(Clone)]
struct Who {
    d: Arc<Daemon>,
    /// `None` for a deferred run, which belongs to no task.
    task: Option<(TaskId, StepRunId)>,
    step: String,
    project: String,
    log_dir: PathBuf,
    id_prefix: String,
    /// A step run that shows this run's progress though no task owns it (a landing batch's
    /// suites report into its first member's run).
    sink: Option<StepRunId>,
    /// The environment every command gets: for a task's step, its `MASON_*` facts with
    /// `MASON_HELD` naming what is leased for the command now running.
    env: Vec<(String, String)>,
    /// A red that also fails at `Tree::base` is excused. Off when `base` is the very tree
    /// under test (the post-land tier), where every red would "also fail at the base".
    trust_base: bool,
    /// For a landing batch's run (`task` is `None`): the tasks it verifies, whose skips and
    /// forces stop it.
    members: Members,
    /// The reuse plan of the suite now running (s707): what it ran is recorded as green or red
    /// against its input keys.
    reuse: Option<Arc<crate::reuse::Plan>>,
    /// What a landing batch's lease holds of this machine for the suite now running (s911,
    /// s943); nothing for a task's or a debt's run.
    cover: Cover,
}

/// What the caller holds of this machine for a run, so that `run_hosted_resuming` takes only
/// what is not held already (s911, decision 0904; s943, decision 0936).
#[derive(Clone, Default)]
pub struct Cover {
    /// The caller holds this machine's slots for this run (a landing batch's lease,
    /// `Runner::hold`): a run sent back here from GitHub runs under that hold and does not ask
    /// the broker for them again.
    covered: bool,
    /// A landing batch's own run that its lease does not hold: the batch, and what the suite
    /// needs here. Such a run takes its slots, as the batch, when it comes to run here.
    batch: Option<(Arc<crate::remote::BatchOwner>, Needs)>,
}

/// What a landing batch holds, for the suites it runs: the batch (to rank what its runs take
/// beside the lease) and the slots of the lease it took, if it took one (s943).
#[derive(Clone, Default)]
pub struct BatchCover {
    pub owner: Option<Arc<crate::remote::BatchOwner>>,
    /// What the batch's lease holds; `None` when it holds nothing.
    pub held: Option<Needs>,
}

impl BatchCover {
    /// `needs` is within what the lease holds, so a run of it needs nothing more of this
    /// machine. Never true without a lease.
    pub fn holds(&self, needs: &Needs) -> bool {
        self.held.as_ref().is_some_and(|h| {
            (!needs.exclusive || h.exclusive)
                && needs
                    .amounts
                    .iter()
                    .all(|(k, v)| h.amounts.get(k).is_some_and(|x| x >= v))
        })
    }

    /// The cover of a run of `suite` of this batch.
    fn of(&self, def: &Definition, suite: &str) -> Cover {
        let needs = crate::landing::suites_needs(def, &[suite.to_string()]);
        let covered = self.holds(&needs);
        Cover {
            covered,
            batch: self.owner.clone().map(|o| (o, needs)),
        }
    }
}

/// The tasks of a landing batch, and the name of the definition's Test step. A batch's
/// verification belongs to no task, so what the owner does to its members is read here: it
/// is stopped when every member has a skip debt for the suite (or the whole Test step), or
/// when any member is forced. A batch with some members skipped keeps running: the others
/// still need their tests.
#[derive(Clone, Default)]
pub struct Members {
    pub tasks: Vec<TaskId>,
    pub step: String,
    /// The project's name, to read its freeze.
    pub project: String,
    /// What Land now stopped in this batch (`suite:NAME`, `ci:NAME`), filled in as runs are
    /// stopped. The landing engine turns it into one deferred debt per member when it lands.
    pub stopped: Arc<Mutex<std::collections::BTreeSet<String>>>,
}

impl Members {
    pub fn new(tasks: &[TaskId], def: &Definition) -> Members {
        Members {
            tasks: tasks.to_vec(),
            step: test_step_name(def).unwrap_or_default(),
            project: String::new(),
            stopped: Default::default(),
        }
    }

    pub fn in_project(mut self, project: &str) -> Members {
        self.project = project.to_string();
        self
    }

    /// Why the batch is to stop waiting on its remaining verification because the owner
    /// pressed Land now on a member while it ran, if it is. Unlike force, it keeps the
    /// freeze: a frozen project's batch is stopped only when a member is the fix.
    pub fn land_now(&self, d: &Daemon) -> Option<String> {
        let t = self.tasks.iter().find(|t| {
            d.store.landing_flag(t.0).ok().flatten().as_deref() == Some(LAND_NOW_RUNNING)
        })?;
        let frozen = d
            .store
            .freezes()
            .ok()
            .and_then(|v| v.into_iter().find(|f| f.project == self.project));
        if let Some(f) = frozen {
            if !self.tasks.iter().any(|x| Some(x.0) == f.fix_task) {
                return None;
            }
        }
        Some(format!(
            "land now by the owner (task #{}): the remaining tests run after the merge",
            t.0
        ))
    }

    /// Remember that Land now stopped `scope`.
    pub fn note_stopped(&self, scope: &str) {
        self.stopped
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(scope.to_string());
    }

    /// Why the batch is to be landed without verification, if it is: a member is forced.
    pub fn forced(&self, d: &Daemon) -> Option<String> {
        self.tasks
            .iter()
            .find(|t| d.store.landing_flag(t.0).ok().flatten().as_deref() == Some("force"))
            .map(|t| {
                format!(
                    "forced by the owner (task #{}): landing without verification",
                    t.0
                )
            })
    }

    /// Why the batch's run of `suite` (`""`: every suite, a build) is to stop, if it is.
    pub fn stop(&self, d: &Daemon, suite: &str) -> Option<String> {
        if self.tasks.is_empty() {
            return None;
        }
        if let Some(why) = self.forced(d) {
            return Some(why);
        }
        let debts: Option<Vec<u64>> = self
            .tasks
            .iter()
            .map(|t| skip_for(d, *t, suite, &self.step))
            .collect();
        debts
            .and_then(|v| v.first().copied())
            .map(|id| format!("skipped by the owner (debt #{id})"))
    }

    /// As [`Members::stop`], and Land now stops the run too. The builds of the batch's floor
    /// are not stopped by it (`stop` only): what lands must still build.
    pub fn stop_running(&self, d: &Daemon, suite: &str) -> Option<String> {
        self.stop(d, suite).or_else(|| self.land_now(d))
    }
}

/// The name of the definition's Test step.
pub(crate) fn test_step_name(def: &Definition) -> Option<String> {
    def.steps
        .iter()
        .find(|s| s.kind == mason_definition::StepType::Test)
        .map(|s| s.name.clone())
}

impl Who {
    /// What the executor needs to decide where a run goes: a task's own suite names its task, a
    /// run no task owns (a landing tier, a bisect half) follows the global switch (#702).
    fn place<'a>(&self, suite: &'a Suite) -> Option<(Option<TaskId>, &'a Suite)> {
        Some((self.task.map(|(t, _)| t), suite))
    }

    /// The step run that shows this run's progress, if any.
    fn run(&self) -> Option<StepRunId> {
        self.task.map(|(_, r)| r).or(self.sink)
    }

    fn project_cfg(&self) -> Option<crate::config::ProjectCfg> {
        self.d
            .project_id_by_name(&self.project)
            .and_then(|i| self.d.project(i))
            .cloned()
    }

    /// The parser plugin for this suite, if it names one (`parser = "plugin:<name>"`). A
    /// plugin that is not approved gives a parser that fails, naming what to approve, so the
    /// suite is red for that reason and nothing runs.
    fn plugin_parser(&self, suite: &Suite) -> Option<Box<PluginParse>> {
        let name = suite.plugin_parser.clone()?;
        let p = self
            .d
            .project_id_by_name(&self.project)
            .and_then(|i| self.d.project(i))
            .cloned();
        Some(Box::new(move |text: &str| match &p {
            Some(p) => crate::plugins::parse_with_plugin(p, &name, text),
            None => Err(format!("no project to find the parser plugin `{name}` in")),
        }))
    }

    /// This project's stall thresholds (both off when the project sets none).
    fn thresholds(&self) -> Thresholds {
        self.d
            .project_id_by_name(&self.project)
            .and_then(|i| self.d.project(i))
            .map_or_else(Thresholds::default, |p| {
                Thresholds::new(p.cont_after, p.kill_after)
            })
    }

    fn check(&self, suite: &str) -> Check {
        let (d, task, step, suite, members) = (
            self.d.clone(),
            self.task,
            self.step.clone(),
            suite.to_string(),
            self.members.clone(),
        );
        Arc::new(move || {
            let Some((t, run)) = task else {
                // A batch's run: stopped by what the owner did to its members.
                return if members.stop_running(&d, &suite).is_some() {
                    Stop::Skipped
                } else {
                    Stop::No
                };
            };
            if d.world()
                .map(|w| w.open_run(t) != Some(run))
                .unwrap_or(false)
            {
                return Stop::Gone;
            }
            if skip_for(&d, t, &suite, &step).is_some() {
                Stop::Skipped
            } else {
                Stop::No
            }
        })
    }
}

/// The debt scope of a task made immune to testing: every suite and every step that tests
/// is skipped for it, from develop's checks to landing, and the full run is owed.
pub const ALL_TESTS: &str = "tests:all";

/// The landing flag Land now sets on a task that is already in a running batch.
pub const LAND_NOW_RUNNING: &str = "now_running";

/// An open debt that skips this suite or its whole step (or all of the task's testing).
pub(crate) fn skip_for(d: &Daemon, task: TaskId, suite: &str, step: &str) -> Option<u64> {
    d.store
        .debts(Some(task.0), true)
        .ok()?
        .into_iter()
        .find(|x| {
            x.scope == ALL_TESTS
                || x.scope == format!("suite:{suite}")
                || x.scope == format!("step:{step}")
        })
        .map(|x| x.id)
}

/// The open debt that makes the task immune to testing, if it has one.
pub(crate) fn immune(d: &Daemon, task: TaskId) -> Option<u64> {
    d.store
        .debts(Some(task.0), true)
        .ok()?
        .into_iter()
        .find(|x| x.scope == ALL_TESTS)
        .map(|x| x.id)
}

enum SuiteRun {
    Done {
        outcome: SuiteOutcome,
        results: Option<Results>,
        notes: Vec<String>,
    },
    /// Red, and the task's. `failing`: the tests that failed and are the task's (empty when
    /// the suite did not run, or no names were read). `red` is set for the reds that name no
    /// failing test (a vacuous run, a red run with no names): what the base is compared with (s246).
    Product {
        feedback: String,
        failing: Vec<String>,
        red: Option<(RedKind, String)>,
    },
    /// The task went away while this ran.
    Gone,
}

struct Tree {
    /// The tree hash of the change, for evidence.
    tree: String,
    /// The merge base, where baselines are taken.
    base: String,
}

/// A device leased for one attempt at a suite. Dropping it frees the device.
struct Dev {
    id: String,
    _lease: mason_devices::lease::DeviceLease,
}

/// A device for a gate that runs a suite outside its own attempt loop.
/// `Err` when the suite needs a device and none is free: the gate is skipped,
/// not failed.
fn lease_dev(who: &Who, def: &Definition, suite: &Suite) -> Result<Option<Dev>, ()> {
    let Some((kind, filter)) = def.testing.environment(&suite.env).and_then(|e| {
        crate::targets::kind_of_env(e).map(|k| (k, crate::targets::filter_of_env(e)))
    }) else {
        return Ok(None);
    };
    let label = format!("#{} {}", who.task.map_or(0, |t| t.0 .0), suite.name);
    match who.d.targets.lease(kind, &filter, &label) {
        Some(l) => Ok(Some(Dev {
            id: l.id.clone(),
            _lease: l,
        })),
        None => Err(()),
    }
}

fn dev_id(dev: Option<&Dev>) -> Option<&str> {
    dev.map(|x| x.id.as_str())
}

fn env_down(who: &Who, def: &Definition, suite: &Suite, cwd: &Path, dev: Option<&Dev>) {
    if let Some(e) = def.testing.environment(&suite.env) {
        if let Some(name) = &e.plugin {
            if let Some(p) = who.project_cfg() {
                let _ = crate::plugins::environment_call(&p, name, "teardown", &env_config(e, cwd));
            }
        }
        for p in lifecycle::down(e) {
            let _ = run_shell(
                &lifecycle::with_device(&p.command, dev_id(dev)),
                cwd,
                Duration::from_secs(300),
            );
        }
        // The provider lets the device go last, and only what it started.
        if let Some(dev) = dev {
            let _ = who.d.targets.provider().teardown(&dev.id);
        }
    }
}

/// What an environment plugin is told about the environment.
fn env_config(e: &mason_testing::Environment, cwd: &Path) -> serde_json::Value {
    serde_json::json!({
        "name": e.name,
        "cwd": cwd.to_string_lossy(),
        "setup": e.setup,
        "health": e.health,
        "teardown": e.teardown,
        "uses": e.uses,
    })
}

/// `environment:<name>` plugin: setup, then health until it says ok or `health_timeout` is spent.
fn env_up_plugin(
    who: &Who,
    e: &mason_testing::Environment,
    name: &str,
    cwd: &Path,
) -> Result<(), String> {
    let p = who
        .project_cfg()
        .ok_or_else(|| format!("environment `{}`: no project to find `{name}` in", e.name))?;
    let cfg = env_config(e, cwd);
    crate::plugins::environment_call(&p, name, "setup", &cfg)
        .map_err(|x| format!("environment `{}` failed at setup: {x}", e.name))?;
    let end = Instant::now() + e.health_timeout;
    loop {
        match crate::plugins::environment_call(&p, name, "health", &cfg) {
            Ok(()) => return Ok(()),
            Err(x) if Instant::now() >= end => {
                return Err(format!(
                    "environment `{}` was not healthy after {}s: {x}",
                    e.name,
                    e.health_timeout.as_secs()
                ))
            }
            Err(_) => std::thread::sleep(Duration::from_millis(500)),
        }
    }
}

/// Bring the environment up and wait for it to be healthy. `Err` names what
/// failed: the environment's fault.
fn env_up(
    who: &Who,
    def: &Definition,
    suite: &Suite,
    cwd: &Path,
    dev: Option<&Dev>,
) -> Result<(), String> {
    let Some(e) = def.testing.environment(&suite.env) else {
        return Ok(());
    };
    let provider = who.d.targets.provider();
    if let Some(name) = &e.plugin {
        return env_up_plugin(who, e, name, cwd);
    }
    if let Some(dev) = dev {
        provider.prepare(&dev.id).map_err(|x| {
            format!(
                "environment `{}`: device {} could not be readied: {x}",
                e.name, dev.id
            )
        })?;
    }
    for p in lifecycle::up(e) {
        let (ok, out) = run_shell(
            &lifecycle::with_device(&p.command, dev_id(dev)),
            cwd,
            Duration::from_secs(600),
        );
        if !ok {
            return Err(format!(
                "environment `{}` failed at {}: {}",
                e.name,
                p.name,
                tail(&out, 6).trim()
            ));
        }
    }
    let end = Instant::now() + e.health_timeout;
    if let Some(dev) = dev {
        loop {
            match provider.health(&dev.id) {
                Ok(()) => break,
                Err(x) if Instant::now() >= end => {
                    return Err(format!(
                        "environment `{}`: device {} was not healthy after {}s: {x}",
                        e.name,
                        dev.id,
                        e.health_timeout.as_secs()
                    ))
                }
                Err(_) => std::thread::sleep(Duration::from_millis(500)),
            }
        }
    }
    if let Some(h) = lifecycle::health(e) {
        loop {
            let (ok, out) = run_shell(
                &lifecycle::with_device(&h.command, dev_id(dev)),
                cwd,
                Duration::from_secs(30),
            );
            if ok {
                break;
            }
            if Instant::now() >= end {
                return Err(format!(
                    "environment `{}` was not healthy after {}s: {}",
                    e.name,
                    e.health_timeout.as_secs(),
                    tail(&out, 4).trim()
                ));
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }
    Ok(())
}

/// What fails at the merge base, and every test name there: worked out once
/// per base commit and suite, in a throwaway worktree.
fn baseline(
    who: &Who,
    c_repo: &Path,
    def: &Definition,
    suite: &Suite,
    base: &str,
    lock: &crate::daemon::RepoLock,
    dev: Option<&Dev>,
) -> Option<(Vec<String>, Vec<String>)> {
    baseline_noted(who, c_repo, def, suite, base, lock, dev, &mut None)
}

/// A stable hash of everything that defines how a suite runs, so a stored baseline is
/// never carried across a change to its command or settings (s478).
fn suite_def_hash(def: &Definition, suite: &Suite) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    format!("{suite:?}").hash(&mut h);
    if !suite.env.is_empty() {
        format!("{:?}", def.testing.environment(&suite.env)).hash(&mut h);
    }
    format!("{:016x}", h.finish())
}

/// Whether a change to `paths` leaves `suite` alone, by the definition's impact map. A path
/// the map does not name, or one under `.mason/` (the definition itself), touches it.
fn untouched_by(def: &Definition, suite: &Suite, paths: &[String]) -> bool {
    if paths.iter().any(|p| p.starts_with(".mason/")) {
        return false;
    }
    matches!(
        def.testing.impact.select(paths, std::slice::from_ref(&suite.name)),
        Selection::Some(v) if v.is_empty()
    )
}

/// Split a change's `paths` for `suite`: those it may read, and those the impact map sends only
/// to other suites (`clients/swift/**` goes only to `swift-clients`). A path the map does not
/// name, or one under `.mason/`, stays with the suite. Narrowing then looks at the first list
/// only, so a client-only change does not send a cargo suite to run whole (#616).
pub(crate) fn split_for_suite(
    def: &Definition,
    suite: &Suite,
    paths: &[String],
) -> (Vec<String>, Vec<String>) {
    paths
        .iter()
        .cloned()
        .partition(|p| !untouched_by(def, suite, std::slice::from_ref(p)))
}

/// The dropped paths as a note: "clients/swift/** left out: the impact map sends it only to
/// other suites". Empty when nothing was dropped.
pub(crate) fn left_out_note(dropped: &[String]) -> String {
    if dropped.is_empty() {
        return String::new();
    }
    let mut dirs: Vec<String> = dropped
        .iter()
        .map(|p| match p.split('/').collect::<Vec<_>>().as_slice() {
            [a, b, _, ..] => format!("{a}/{b}/**"),
            _ => p.clone(),
        })
        .collect();
    dirs.sort();
    dirs.dedup();
    format!(
        "{} left out: the impact map sends {} only to other suites",
        dirs.join(", "),
        if dropped.len() == 1 { "it" } else { "them" }
    )
}

/// The paths that differ between two trees.
fn tree_diff(c_repo: &Path, from: &str, to: &str) -> Option<Vec<String>> {
    let out = git::run(c_repo, &["diff", "--name-only", from, to]).ok()?;
    Some(
        out.lines()
            .filter(|l| !l.is_empty())
            .map(String::from)
            .collect(),
    )
}

/// An earlier round's green of `suite` that still stands for `tree`: same task, same
/// definition hash, and a non-empty diff that the impact map sends nowhere near the suite.
/// Returns `(executed, passed, step_run, from_tree)` (#677). Only whole-suite greens are ever
/// stored (a narrowed run proves only the tests it ran), so a narrowed pick is never reused.
fn reusable_task_green(
    store: &mason_store::Store,
    c_repo: &Path,
    def: &Definition,
    suite: &Suite,
    task: u64,
    tree: &str,
) -> Option<(usize, usize, u64, String)> {
    let hash = suite_def_hash(def, suite);
    for (from, step_run, executed, passed) in store.task_greens(task, &suite.name, &hash).ok()? {
        if from == tree {
            continue;
        }
        let Some(paths) = tree_diff(c_repo, &from, tree) else {
            continue;
        };
        if !paths.is_empty() && untouched_by(def, suite, &paths) {
            return Some((executed, passed, step_run, from));
        }
    }
    None
}

/// Record evidence for `tree` from an earlier round's green, when one stands. Never for a
/// landing batch or a debt run, which belong to no task. Returns the note to report.
fn adopt_earlier_green(
    who: &Who,
    c_repo: &Path,
    def: &Definition,
    suite: &Suite,
    tree: &Tree,
) -> Option<String> {
    let (task, run) = who.task?;
    let d = &who.d;
    if matches!(d.store.evidence(&tree.tree, &suite.name), Ok(Some(_))) {
        return None;
    }
    let (executed, passed, from_run, _) =
        reusable_task_green(&d.store, c_repo, def, suite, task.0, &tree.tree)?;
    let hash = suite_def_hash(def, suite);
    let now = now_ms();
    d.store
        .put_evidence(&tree.tree, &suite.name, executed, passed, task.0, now)
        .ok()?;
    let _ = d.store.put_task_green(
        task.0,
        &suite.name,
        &hash,
        &tree.tree,
        from_run,
        executed,
        passed,
        now,
    );
    let _ = run;
    Some(format!(
        "{}: green, reused from an earlier round (step run #{from_run}; nothing since touches it)",
        suite.name
    ))
}

/// The newest stored baseline of this suite at an ancestor of `base`, with the same
/// definition hash, when nothing between the two touches the suite (s478).
fn reusable_baseline(
    store: &mason_store::Store,
    c_repo: &Path,
    def: &Definition,
    suite: &Suite,
    hash: &str,
    base: &str,
) -> Option<(String, Vec<String>, Vec<String>, usize)> {
    let cands = store.ancestor_baselines(&suite.name, hash, 40).ok()?;
    for (cand, failing, names, executed) in cands {
        if cand == base {
            continue;
        }
        if git::run(c_repo, &["merge-base", "--is-ancestor", &cand, base]).is_err() {
            continue;
        }
        let Ok(out) = git::run(c_repo, &["diff", "--name-only", &format!("{cand}..{base}")]) else {
            continue;
        };
        let paths: Vec<String> = out
            .lines()
            .filter(|l| !l.is_empty())
            .map(String::from)
            .collect();
        if untouched_by(def, suite, &paths) {
            return Some((cand, failing, names, executed));
        }
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn baseline_noted(
    who: &Who,
    c_repo: &Path,
    def: &Definition,
    suite: &Suite,
    base: &str,
    lock: &crate::daemon::RepoLock,
    dev: Option<&Dev>,
    note: &mut Option<String>,
) -> Option<(Vec<String>, Vec<String>)> {
    let hash = suite_def_hash(def, suite);
    // A narrowed run's tests are this change's pick, not the suite's: never cached (s386).
    if let Some(b) = (!suite.narrowed)
        .then(|| who.d.store.baseline(base, &suite.name).ok().flatten())
        .flatten()
    {
        return Some(b);
    }
    if !suite.narrowed {
        if let Some((from, failing, names, executed)) =
            reusable_baseline(&who.d.store, c_repo, def, suite, &hash, base)
        {
            let _ = who.d.store.put_baseline_hashed(
                base,
                &suite.name,
                &failing,
                &names,
                executed,
                now_ms(),
                &hash,
            );
            *note = Some(format!(
                "{}: baseline reused from {}, nothing between it and {} touches the suite",
                suite.name,
                &from[..from.len().min(10)],
                &base[..base.len().min(10)]
            ));
            return Some((failing, names));
        }
    }
    let (failing, names, executed) = run_on_base(
        who,
        c_repo,
        def,
        suite,
        base,
        lock,
        dev,
        &suite.command,
        "b",
    )?;
    if !suite.narrowed {
        let _ = who.d.store.put_baseline_hashed(
            base,
            &suite.name,
            &failing,
            &names,
            executed,
            now_ms(),
            &hash,
        );
    }
    Some((failing, names))
}

/// Run `cmd` in a checkout of `base`: what failed, every name seen, and how many tests ran.
/// `None` if the checkout, the run or its time limit did not give a result (a signal, a
/// timeout). `tag` names the run's job id: `b` for the whole suite, `t` for a targeted one.
#[allow(clippy::too_many_arguments)]
fn run_on_base(
    who: &Who,
    c_repo: &Path,
    def: &Definition,
    suite: &Suite,
    base: &str,
    lock: &crate::daemon::RepoLock,
    dev: Option<&Dev>,
    cmd: &str,
    tag: &str,
) -> Option<(Vec<String>, Vec<String>, usize)> {
    let wt = who.log_dir.join(format!(
        "base-{}{}",
        suite.name,
        if tag == "b" { "" } else { "-t" }
    ));
    {
        let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
        git::remove_worktree(c_repo, &wt);
        git::run(
            c_repo,
            &["worktree", "add", "--detach", &wt.to_string_lossy(), base],
        )
        .ok()?;
    }
    if let Err(e) = crate::jobs::setup_checkout(def, &wt) {
        tracing::warn!("baseline of {} at {base}: {e}", suite.name);
        let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
        git::remove_worktree(c_repo, &wt);
        return None;
    }
    let outcome = (|| {
        env_up(who, def, suite, &wt, dev).ok()?;
        if let Some(f) = &suite.results_file {
            remove_results(&wt, f);
        }
        let id = format!("{}-{tag}-{}", who.id_prefix, suite.name);
        // The develop comparison's directory (`base-<suite>`) is never a task's: prepare it (#556).
        if let Some(p) = who.project_cfg() {
            crate::taskcache::prepare_for_build(&who.d, &p, &wt, now_ms());
        }
        let ran = run_hosted(
            &who.d,
            &who.project,
            &id,
            &crate::jobs::pty_wrap(suite.pty, &lifecycle::with_device(cmd, dev_id(dev))),
            &wt,
            suite.timeout.unwrap_or(Duration::from_secs(1800)),
            &who.log_dir,
            who.check(&suite.name),
            who.run(),
            None,
            who.thresholds(),
            who.env.clone(),
            None,
            &who.cover,
        )
        .ok()?;
        if ran.stop != Stop::No {
            return None;
        }
        let j = judge_ran_with(suite, &ran, &wt, who.plugin_parser(suite).as_deref());
        Some((
            j.results.failed_names(),
            j.results.names(),
            j.results.executed(),
        ))
    })();
    env_down(who, def, suite, &wt, dev);
    {
        let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
        git::remove_worktree(c_repo, &wt);
    }
    outcome
}

/// What develop says of the tests that stayed red on the task's tree.
enum AtBase {
    /// The tests that failed there (empty: they all passed).
    Failing(Vec<String>),
    /// No run finished with the tests' names read.
    Unknown,
}

/// Run only `tests` at `base`, with the suite's `rerun` command (`{tests}` filled in). A run
/// that did not finish (a signal, a timeout) or read no names is tried once more; if that
/// one fails the same way the answer is [`AtBase::Unknown`], not "green" (#494).
#[allow(clippy::too_many_arguments)]
fn failing_at_base(
    who: &Who,
    c_repo: &Path,
    def: &Definition,
    suite: &Suite,
    base: &str,
    lock: &crate::daemon::RepoLock,
    dev: Option<&Dev>,
    template: &str,
    tests: &[String],
) -> AtBase {
    let cmd = template.replace(
        "{tests}",
        &tests.iter().map(|x| quote(x)).collect::<Vec<_>>().join(" "),
    );
    for _ in 0..2 {
        let tag = format!("t{}", now_ms());
        if let Some((failing, _, executed)) =
            run_on_base(who, c_repo, def, suite, base, lock, dev, &cmd, &tag)
        {
            if executed > 0 {
                return AtBase::Failing(failing);
            }
        }
    }
    AtBase::Unknown
}

#[allow(clippy::too_many_arguments)]
fn run_suite(
    who: &Who,
    c_repo: &Path,
    repo_lock: &crate::daemon::RepoLock,
    def: &Definition,
    suite: &Suite,
    cwd: &Path,
    tree: &Tree,
    on_wait: &mut dyn FnMut(Option<String>),
) -> SuiteRun {
    let d = &who.d;
    // The test step writes the project's `test` command into its suites before it calls here;
    // a debt run, a ledger rerun and a landing batch did not, and ran `{project_test}` as a
    // command (s271). Writing it in again changes nothing for a suite that has it already.
    let orig_suite = suite;
    let filled;
    let suite = if uses_project_test(suite) {
        let test = d
            .project_id_by_name(&who.project)
            .and_then(|pid| crate::jobs::test_commands(d, def, pid, who.task.map(|(t, _)| t)))
            .map(|t| t.test);
        filled = match with_project_test(suite, test.as_deref()) {
            Ok(s) => s,
            Err(why) => {
                return SuiteRun::Done {
                    outcome: SuiteOutcome::Failed,
                    results: None,
                    notes: vec![format!("{}: {why}", suite.name)],
                }
            }
        };
        &filled
    } else {
        suite
    };
    // What an earlier round of this task proved, when nothing since touches the suite (#677).
    let def_hash = suite_def_hash(def, orig_suite);
    let adopted = adopt_earlier_green(who, c_repo, def, orig_suite, tree);
    // Identical code never tests twice.
    if let Ok(Some((executed, _, task))) = d.store.evidence(&tree.tree, &suite.name) {
        if let Some(run) = who.run() {
            d.progress_sub(run, &suite.name, None, |s| {
                s.unit = "tests".into();
                s.done = Figure::counted(executed as u64);
                s.total = Figure::expected(executed as u64);
            });
        }
        return SuiteRun::Done {
            outcome: SuiteOutcome::Reused,
            results: None,
            notes: vec![adopted.unwrap_or_else(|| {
                format!(
                    "{}: green for this exact tree already ({executed} tests, from task #{task})",
                    suite.name
                )
            })],
        };
    }
    let signatures: &[Signature] = &def.testing.signatures;
    let known = crate::knownred::known_for(&d.store, &suite.name);
    let quarantined: Vec<String> = known.iter().map(|k| k.test.clone()).collect();
    // With a `skip` template the known-red tests are not run at all.
    let skipped_cmd: Option<String> = suite
        .skip
        .as_deref()
        .and_then(|t| crate::knownred::skip_command(t, &quarantined));
    let limit = suite.timeout.unwrap_or(Duration::from_secs(1800));
    let mut attempt = 0u32;
    let mut remote_env_fails = 0u32;
    let mut waiting = false;
    loop {
        // A skip may arrive at any time, even before the suite starts.
        let check = who.check(&suite.name);
        match check() {
            Stop::Skipped => {
                if waiting {
                    on_wait(None);
                }
                return SuiteRun::Done {
                    outcome: SuiteOutcome::Skipped,
                    results: None,
                    notes: vec![format!("{}: skipped by force", suite.name)],
                };
            }
            Stop::Gone => return SuiteRun::Gone,
            Stop::No => {}
        }
        if let Some(f) = &suite.results_file {
            remove_results(cwd, f);
        }
        let environmental = |why: String,
                             advice: String,
                             on_wait: &mut dyn FnMut(Option<String>)| {
            tracing::warn!(suite = %suite.name, %why, "environmental failure: waiting, charged to no task");
            on_wait(Some(format!(
                "environment problem, not the task's fault: {why}{}; retrying",
                if advice.is_empty() {
                    String::new()
                } else {
                    format!(" ({advice})")
                }
            )));
        };
        // A device environment leases one particular device for this attempt.
        let dev: Option<Dev> = match def.testing.environment(&suite.env).and_then(|e| {
            crate::targets::kind_of_env(e).map(|k| (k, crate::targets::filter_of_env(e)))
        }) {
            Some((kind, filter)) => {
                let label = format!("#{} {}", who.task.map_or(0, |t| t.0 .0), suite.name);
                match d.targets.lease(kind, &filter, &label) {
                    Some(l) => Some(Dev {
                        id: l.id.clone(),
                        _lease: l,
                    }),
                    None => {
                        environmental(
                            format!(
                                "no free {} ({}) is available right now",
                                kind.noun(),
                                kind.capability()
                            ),
                            String::new(),
                            on_wait,
                        );
                        waiting = true;
                        attempt += 1;
                        std::thread::sleep(d.cfg.env_retry);
                        continue;
                    }
                }
            }
            None => None,
        };
        if let Err(why) = env_up(who, def, suite, cwd, dev.as_ref()) {
            env_down(who, def, suite, cwd, dev.as_ref());
            environmental(why, String::new(), on_wait);
            waiting = true;
            attempt += 1;
            std::thread::sleep(d.cfg.env_retry);
            continue;
        }
        let id = if attempt == 0 {
            format!("{}-u-{}", who.id_prefix, suite.name)
        } else {
            format!("{}-u{}-{}", who.id_prefix, now_ms(), suite.name)
        };
        // A whole-suite run with no skips can go on after a lost host from what finished (#621).
        let resume = if skipped_cmd.is_none() && suite.results_file.is_none() && !suite.pty {
            crate::resume::template(suite)
        } else {
            None
        };
        let ran = match run_hosted_resuming(
            d,
            &who.project,
            &id,
            &crate::jobs::pty_wrap(
                suite.pty,
                &lifecycle::with_device(
                    skipped_cmd.as_deref().unwrap_or(&suite.command),
                    dev_id(dev.as_ref()),
                ),
            ),
            cwd,
            limit,
            &who.log_dir,
            check,
            who.run(),
            Some(suite),
            who.thresholds(),
            who.env.clone(),
            resume.as_deref(),
            who.place(suite),
            &who.cover,
        ) {
            Ok(r) => r,
            Err(e) if crate::remote::environmental(&e).is_some() => {
                // No runner, or GitHub is down: a wait charged to no task, as a missing device is.
                env_down(who, def, suite, cwd, dev.as_ref());
                environmental(
                    crate::remote::environmental(&e)
                        .unwrap_or_default()
                        .to_string(),
                    String::new(),
                    on_wait,
                );
                waiting = true;
                attempt += 1;
                // A runner that fails the same way again and again is not helped by trying at
                // once: wait twice as long each time.
                remote_env_fails += 1;
                std::thread::sleep(crate::remote::env_backoff(
                    d.cfg.env_retry,
                    remote_env_fails - 1,
                ));
                continue;
            }
            Err(e) => {
                env_down(who, def, suite, cwd, dev.as_ref());
                return SuiteRun::Product {
                    feedback: format!("`{}` could not run: {e}", suite.name),
                    failing: vec![],
                    red: None,
                };
            }
        };
        let ran = Ran {
            narrowed: suite.narrowed || ran.resumed.is_some(),
            ..ran
        };
        record_time(who, &suite.name, "run", &ran);
        match ran.stop {
            Stop::Skipped => {
                env_down(who, def, suite, cwd, dev.as_ref());
                if waiting {
                    on_wait(None);
                }
                return SuiteRun::Done {
                    outcome: SuiteOutcome::Skipped,
                    results: None,
                    notes: vec![format!("{}: skipped by force while it ran", suite.name)],
                };
            }
            Stop::Gone => {
                env_down(who, def, suite, cwd, dev.as_ref());
                return SuiteRun::Gone;
            }
            Stop::No => {}
        }
        let j = judge_ran_with(suite, &ran, cwd, who.plugin_parser(suite).as_deref());
        record_test_times(who, &suite.name, &j.results);
        record_target_times(who, &suite.name, &j.output);
        let out = |extra: &str| {
            format!(
                "{extra}Last output of `{}`:\n{}",
                suite.name,
                tail(&j.output, 40)
            )
        };
        let result = match &j.verdict {
            Verdict::Green if skipped_cmd.is_some() => {
                // Not the whole suite's green: no proof is recorded for the tree, and no
                // `suite_green` for the base. A whole, unnarrowed run at a base is recorded
                // apart, with the tests it skipped (s476).
                if !suite.narrowed {
                    let _ = d.store.put_suite_skip_green(
                        &who.project,
                        &suite.name,
                        &tree.base,
                        j.results.executed(),
                        &quarantined,
                        now_ms(),
                    );
                }
                SuiteRun::Done {
                    outcome: SuiteOutcome::Proven,
                    results: Some(j.results.clone()),
                    notes: vec![format!(
                        "{}: green, {} tests ran; skipped, known red: {}",
                        suite.name,
                        j.results.executed(),
                        quarantined.join(", ")
                    )],
                }
            }
            Verdict::Green => {
                // What a green run ran passed: the proof that these names are tests (s253).
                let _ = d
                    .store
                    .note_passing(&suite.name, &j.results.names(), now_ms());
                // Each target this run built and ran is green on its input key (s707).
                if let Some(plan) = &who.reuse {
                    plan.record_green(
                        &d.store,
                        &j.output,
                        &tree.tree,
                        who.task.map_or(0, |t| t.0 .0),
                        now_ms(),
                    );
                }
                // A narrowed run proves only the tests it ran: no evidence for the tree, and no
                // record that the suite is green at its base (s386).
                if !suite.narrowed {
                    let _ = d.store.put_suite_green(
                        &who.project,
                        &suite.name,
                        &tree.base,
                        j.results.executed(),
                        now_ms(),
                    );
                    let _ = d.store.put_evidence(
                        &tree.tree,
                        &suite.name,
                        j.results.executed(),
                        j.results.passed(),
                        who.task.map_or(0, |t| t.0 .0),
                        now_ms(),
                    );
                    if let Some((t, r)) = who.task {
                        let _ = d.store.put_task_green(
                            t.0,
                            &suite.name,
                            &def_hash,
                            &tree.tree,
                            r.0,
                            j.results.executed(),
                            j.results.passed(),
                            now_ms(),
                        );
                    }
                }
                SuiteRun::Done {
                    outcome: SuiteOutcome::Proven,
                    results: Some(j.results.clone()),
                    notes: vec![format!(
                        "{}: green, {} tests ran",
                        suite.name,
                        j.results.executed()
                    )],
                }
            }
            Verdict::Vacuous { executed, needed } => {
                // The machine's fault takes precedence even here.
                if let RedClass::Environmental { signature, advice } = classify(&Inputs {
                    failing: &[],
                    output: &j.output,
                    signatures,
                    rerun_failing: None,
                    baseline_failing: None,
                    quarantined: &[],
                }) {
                    env_down(who, def, suite, cwd, dev.as_ref());
                    environmental(format!("matches `{signature}`"), advice, on_wait);
                    waiting = true;
                    attempt += 1;
                    std::thread::sleep(d.cfg.env_retry);
                    continue;
                }
                SuiteRun::Product {
                    feedback: out(&format!(
                        "`{}` proved nothing: {executed} test(s) ran and at least {needed} must. A run that executes no tests is not a pass; make the suite run the tests (or fix what stopped it).\n",
                        suite.name
                    )),
                    red: Some((RedKind::Vacuous, String::new())),
                    failing: vec![],
                }
            }
            Verdict::Red { failures, why } => {
                // A red target is never reused on these inputs, even if a rerun is green (s707).
                if let Some(plan) = &who.reuse {
                    plan.record_red(
                        &d.store,
                        &j.output,
                        &tree.tree,
                        who.task.map_or(0, |t| t.0 .0),
                        now_ms(),
                    );
                }
                // A landing batch's run stops at the boundary for a waiting update (s705, s739):
                // the immediate rerun is one more run, so it is left to the next batch.
                let yielding = who.task.is_none() && d.landing.draining();
                let rerun = if failures.is_empty() || yielding {
                    None
                } else {
                    rerun_failing(who, suite, cwd, failures, attempt, dev.as_ref())
                };
                // Baseline only when something still fails after a rerun.
                let persistent: Vec<String> = match &rerun {
                    Some(again) => failures
                        .iter()
                        .filter(|f| again.contains(f))
                        .cloned()
                        .collect(),
                    None => failures.clone(),
                };
                // A known-red failure needs no base run: the ledger already excuses it.
                let unexcused = persistent.iter().any(|t| !quarantined.contains(t));
                let mut reuse_note: Option<String> = None;
                let mut unknown_at_base: Vec<String> = vec![];
                // A whole-suite baseline already known for this base is used as is.
                let cached = !suite.narrowed
                    && d.store
                        .baseline(&tree.base, &suite.name)
                        .ok()
                        .flatten()
                        .is_some();
                let base = if persistent.is_empty() || !unexcused || !who.trust_base {
                    None
                } else if let (Some(tpl), false) = (suite.rerun.as_deref(), cached) {
                    // Only the tests that failed, not the suite (#494).
                    let ask: Vec<String> = persistent
                        .iter()
                        .filter(|t| !quarantined.contains(t))
                        .cloned()
                        .collect();
                    match failing_at_base(
                        who,
                        c_repo,
                        def,
                        suite,
                        &tree.base,
                        repo_lock,
                        dev.as_ref(),
                        tpl,
                        &ask,
                    ) {
                        AtBase::Failing(f) => {
                            let own = who.task.map_or(vec![], |(t, _)| {
                                crate::basecheck::own_tests(&d.store, &suite.name, t.0)
                            });
                            Some(f.into_iter().filter(|t| !own.contains(t)).collect())
                        }
                        AtBase::Unknown => {
                            unknown_at_base = ask;
                            None
                        }
                    }
                } else {
                    baseline_noted(
                        who,
                        c_repo,
                        def,
                        suite,
                        &tree.base,
                        repo_lock,
                        dev.as_ref(),
                        &mut reuse_note,
                    )
                    .map(|b| {
                        // The fix task's own step is not excused for its tests.
                        let own = who.task.map_or(vec![], |(t, _)| {
                            crate::basecheck::own_tests(&d.store, &suite.name, t.0)
                        });
                        b.0.into_iter()
                            .filter(|t| !own.contains(t))
                            .collect::<Vec<String>>()
                    })
                };
                let class = classify(&Inputs {
                    failing: failures,
                    output: &j.output,
                    signatures,
                    rerun_failing: rerun.as_deref(),
                    baseline_failing: base.as_deref(),
                    quarantined: &quarantined,
                });
                // What failed at the base goes into the ledger, with one fix task each.
                let known_red: Vec<String> = match &class {
                    RedClass::Baseline { tests } => tests.clone(),
                    RedClass::Product { excused, .. } => excused
                        .iter()
                        .filter(|t| base.as_ref().is_some_and(|b| b.contains(t)))
                        .cloned()
                        .collect(),
                    _ => vec![],
                };
                let fixes = record_known_red(who, suite, &tree.base, &known_red, &j.output);
                match class {
                    RedClass::Environmental { signature, advice } => {
                        env_down(who, def, suite, cwd, dev.as_ref());
                        environmental(format!("matches `{signature}`"), advice, on_wait);
                        waiting = true;
                        attempt += 1;
                        std::thread::sleep(d.cfg.env_retry);
                        continue;
                    }
                    RedClass::Flaky { tests } => {
                        for t in &tests {
                            let _ = d.store.record_flaky(
                                &suite.name,
                                t,
                                who.task.map_or(0, |t| t.0 .0),
                                now_ms(),
                            );
                        }
                        // Three tasks in a week make it a quarantine and a task (s253).
                        if let Some(pid) = d.project_id_by_name(&who.project) {
                            crate::autoquarantine::consider(d, pid, &suite.name, &tests);
                        }
                        SuiteRun::Done {
                            outcome: SuiteOutcome::Flaky,
                            results: None,
                            notes: vec![format!(
                                "{}: flaky, passed on an immediate rerun: {}",
                                suite.name,
                                tests.join(", ")
                            )],
                        }
                    }
                    RedClass::Baseline { tests } => SuiteRun::Done {
                        outcome: SuiteOutcome::Excused,
                        results: None,
                        notes: std::iter::once(format!(
                            "{}: excused, also failing at the merge base ({}): {}{}",
                            suite.name,
                            &tree.base[..tree.base.len().min(10)],
                            tests.join(", "),
                            fixes
                        ))
                        .chain(reuse_note.clone())
                        .collect(),
                    },
                    RedClass::Quarantined { tests } => SuiteRun::Done {
                        outcome: SuiteOutcome::Excused,
                        results: None,
                        notes: vec![format!(
                            "{}: excused, quarantined: {}",
                            suite.name,
                            tests.join(", ")
                        )],
                    },
                    RedClass::Product {
                        tests,
                        excused,
                        flaky,
                    } => {
                        // develop's answer is unknown (the run on it did not finish twice):
                        // the round is not the task's to answer for (#494).
                        if tests.iter().any(|t| unknown_at_base.contains(t)) {
                            env_down(who, def, suite, cwd, dev.as_ref());
                            if waiting {
                                on_wait(None);
                            }
                            return SuiteRun::Done {
                                outcome: SuiteOutcome::Excused,
                                results: None,
                                notes: vec![format!(
                                    "{}: red on `{}`, but the run of those tests on develop ({}) did not finish twice, so this round is not counted against the task",
                                    suite.name,
                                    tests.join("`, `"),
                                    &tree.base[..tree.base.len().min(10)]
                                )],
                            };
                        }
                        let mut extra = format!("`{}` is red ({why}).", suite.name);
                        if why == RAN_OUT_OF_TIME && tests.is_empty() {
                            let still = still_running(&j.output);
                            if !still.is_empty() {
                                extra.push_str(&format!("\n{STILL_RUNNING}{}.", still.join(", ")));
                            }
                        }
                        if !tests.is_empty() {
                            extra.push_str(&format!(" Failing: {}.", tests.join(", ")));
                        }
                        if !excused.is_empty() {
                            extra.push_str(&format!(
                                " Not yours (failing at the base too, or quarantined): {}.{}",
                                excused.join(", "),
                                fixes
                            ));
                        }
                        if !flaky.is_empty() {
                            extra.push_str(&format!(
                                " Flaked and passed on rerun: {}.",
                                flaky.join(", ")
                            ));
                        }
                        if let Some(n) = &reuse_note {
                            extra.push_str(&format!(" {n}."));
                        }
                        extra.push('\n');
                        // A red that names no test is compared with the base by its first error.
                        let red = failures.is_empty().then(|| {
                            let e = basered::first_error(&j.output);
                            (
                                RedKind::Nameless,
                                if e.is_empty() { why.clone() } else { e },
                            )
                        });
                        SuiteRun::Product {
                            feedback: out(&extra),
                            red,
                            failing: tests.clone(),
                        }
                    }
                }
            }
        };
        env_down(who, def, suite, cwd, dev.as_ref());
        if waiting {
            on_wait(None);
        }
        // Say what a lost host's run left reusable (#621).
        let said = [ran.resumed.clone(), ran.ran_on.clone()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join("; ");
        let said = (!said.is_empty()).then_some(said);
        // A run no task owns (a landing tier, a bisect half) says where it ran, for `mason landing`.
        if who.task.is_none() {
            crate::place::note_landing_run(
                &who.project,
                &id,
                &suite.name,
                ran.ran_on.as_deref().unwrap_or("ran on this Mac"),
            );
        }
        let result = match (result, &said) {
            (
                SuiteRun::Done {
                    outcome,
                    results,
                    mut notes,
                },
                Some(r),
            ) => {
                notes.push(format!("{}: {r}", suite.name));
                SuiteRun::Done {
                    outcome,
                    results,
                    notes,
                }
            }
            (
                SuiteRun::Product {
                    feedback,
                    failing,
                    red,
                },
                Some(r),
            ) => SuiteRun::Product {
                feedback: format!("({}: {r})\n{feedback}", suite.name),
                failing,
                red,
            },
            (other, _) => other,
        };
        return result;
    }
}

/// Add the tests seen red at the base to the ledger and file one fix task each (the first
/// time only). Returns a note naming the fixes, or nothing.
fn record_known_red(
    who: &Who,
    suite: &Suite,
    base: &str,
    tests: &[String],
    output: &str,
) -> String {
    if tests.is_empty() {
        return String::new();
    }
    let Some(pid) = who.d.project_id_by_name(&who.project) else {
        return String::new();
    };
    let tail = tail(output, 30);
    let seen = crate::basecheck::Seen {
        project: pid,
        project_name: &who.project,
        task: who.task.map_or(0, |(t, _)| t.0),
        step: &suite.name,
        base,
        cmd: &suite.command,
        log_tail: &tail,
        follows: None,
    };
    let fixes = crate::basecheck::record_red(&who.d, &suite.name, &seen, tests);
    let named: Vec<String> = fixes
        .iter()
        .filter_map(|(t, f)| f.as_ref().map(|f| format!("{t}: {}", f.text())))
        .collect();
    if named.is_empty() {
        String::new()
    } else {
        format!(" Fix: {}.", named.join("; "))
    }
}

/// Lift ledger entries for tests this run executed and passed. Only a run on the base
/// branch calls this; a run that did not execute a test never lifts it.
fn lift_proven(d: &Daemon, suite: &str, results: &Results) {
    let passed: Vec<String> = results
        .cases
        .iter()
        .filter(|c| c.status == mason_testing::Status::Pass)
        .map(|c| c.name.clone())
        .collect();
    if passed.is_empty() {
        return;
    }
    if let Ok(l) = d.store.ledger_lift(suite, &passed) {
        for t in l {
            tracing::info!(suite, test = %t, "known-red entry lifted: the test passed");
        }
    }
}

/// Run the failing tests again, straight away. Returns what still fails, or
/// `None` if the rerun could not tell (then nothing is called flaky).
fn rerun_failing(
    who: &Who,
    suite: &Suite,
    cwd: &Path,
    failures: &[String],
    attempt: u32,
    dev: Option<&Dev>,
) -> Option<Vec<String>> {
    let id = if attempt == 0 {
        format!("{}-r-{}", who.id_prefix, suite.name)
    } else {
        format!("{}-r{}-{}", who.id_prefix, now_ms(), suite.name)
    };
    run_tests_alone(who, suite, cwd, failures, &id, false, dev)
}

/// The suite's `rerun` template with `{tests}` filled in; one test at a time (`serial`) adds
/// libtest's `--test-threads=1` after a `--`, so a red that came from load shows as green.
fn rerun_command(suite: &Suite, tests: &[String], serial: bool) -> String {
    let Some(t) = &suite.rerun else {
        return suite.command.clone();
    };
    let mut cmd = t.replace(
        "{tests}",
        &tests.iter().map(|f| quote(f)).collect::<Vec<_>>().join(" "),
    );
    if serial && cmd.contains(" -- ") {
        cmd.push_str(" --test-threads=1");
    }
    cmd
}

/// Run `tests` of a suite alone with its `rerun` template (the whole suite when it has none)
/// under log `id`. What still fails, or `None` when the run could not tell.
fn run_tests_alone(
    who: &Who,
    suite: &Suite,
    cwd: &Path,
    tests: &[String],
    id: &str,
    serial: bool,
    dev: Option<&Dev>,
) -> Option<Vec<String>> {
    let cmd = rerun_command(suite, tests, serial);
    let cmd = crate::jobs::pty_wrap(suite.pty, &lifecycle::with_device(&cmd, dev_id(dev)));
    if let Some(f) = &suite.results_file {
        remove_results(cwd, f);
    }
    let ran = run_hosted(
        &who.d,
        &who.project,
        id,
        &cmd,
        cwd,
        suite.timeout.unwrap_or(Duration::from_secs(1800)),
        &who.log_dir,
        who.check(&suite.name),
        who.run(),
        None,
        who.thresholds(),
        who.env.clone(),
        who.place(suite),
        &who.cover,
    )
    .ok()?;
    if ran.stop != Stop::No {
        return None;
    }
    // A test that hangs is still not green (s702): it stays on the list.
    if ran.timed_out {
        return Some(tests.to_vec());
    }
    let j = judge_ran_with(suite, &ran, cwd, who.plugin_parser(suite).as_deref());
    if j.results.executed() == 0 {
        // It ran nothing, so it proved nothing: not flaky.
        return None;
    }
    Some(j.results.failed_names())
}

/// Whether every red suite in `failing` can be run on its failing tests alone: it names some
/// and the suite defines a `rerun` template.
pub fn can_run_alone(
    def: &Definition,
    failed: &std::collections::BTreeSet<String>,
    failing: &std::collections::BTreeMap<String, Vec<String>>,
) -> bool {
    !failed.is_empty()
        && failed.iter().all(|n| {
            failing.get(n).is_some_and(|t| !t.is_empty())
                && def.testing.suite(n).is_some_and(|s| s.rerun.is_some())
        })
}

/// Run only the failing tests of each suite in `wt` (#682), one at a time when `serial`;
/// with `build`, after the suite's own `build` (a bisect subset is a fresh tree). Green when
/// none of them still fails. A build that fails, or a run that cannot tell, is red for the
/// suite, with no names. This proves less than a whole run: nothing lands on it alone.
#[allow(clippy::too_many_arguments)]
pub fn run_alone(
    d: &Arc<Daemon>,
    p: &crate::config::ProjectCfg,
    def: &Definition,
    failing: &std::collections::BTreeMap<String, Vec<String>>,
    wt: &Path,
    log_dir: &Path,
    id_prefix: &str,
    sink: Option<StepRunId>,
    build: bool,
    serial: bool,
    cover: &BatchCover,
) -> OverRun {
    let who = Who {
        d: d.clone(),
        task: None,
        step: String::new(),
        project: p.id.clone(),
        log_dir: log_dir.to_path_buf(),
        id_prefix: id_prefix.to_string(),
        sink,
        env: vec![],
        trust_base: true,
        members: Members::default(),
        reuse: None,
        cover: Cover::default(),
    };
    let _ = std::fs::create_dir_all(log_dir);
    let project_test = d
        .project_id_by_name(&p.id)
        .and_then(|pid| crate::jobs::test_commands(d, def, pid, None))
        .map(|t| t.test);
    let mut out = OverRun {
        green: true,
        failed: Default::default(),
        notes: vec![],
        failing: Default::default(),
        unfinished: Default::default(),
    };
    for (name, tests) in failing {
        let mut who = who.clone();
        who.cover = cover.of(def, name);
        let who = &who;
        let red = |out: &mut OverRun, note: String| {
            out.green = false;
            out.failed.insert(name.clone());
            out.notes.push(note);
        };
        let Some(suite) = def.testing.suite(name) else {
            red(&mut out, format!("{name}: not a suite of the definition"));
            continue;
        };
        let suite = match with_project_test(suite, project_test.as_deref()) {
            Ok(s) => s,
            Err(why) => {
                red(&mut out, format!("{name}: {why}"));
                continue;
            }
        };
        if build {
            if let Some(b) = suite.build.as_deref() {
                match run_build(who, &suite, b, 0, wt) {
                    Built::Ok => {}
                    Built::Gone => {
                        red(&mut out, format!("{name}: stopped"));
                        continue;
                    }
                    Built::Red(why) => {
                        red(&mut out, format!("{name}: {}", tail(&why, 12)));
                        continue;
                    }
                }
            }
        }
        let id = format!("{id_prefix}-n-{name}");
        match run_tests_alone(who, &suite, wt, tests, &id, serial, None) {
            Some(still) if still.is_empty() => out.notes.push(format!(
                "{name}: {} failing test(s) green on their own",
                tests.len()
            )),
            Some(still) => {
                red(
                    &mut out,
                    format!("{name}: still red on its own: {}", still.join(", ")),
                );
                out.failing.insert(name.clone(), still);
            }
            None => red(&mut out, format!("{name}: the rerun could not tell")),
        }
    }
    out
}

// ---- the step --------------------------------------------------------------

enum Built {
    Ok,
    Red(String),
    Gone,
}

/// What waiting for a suite's slot came to.
enum Leased {
    Got,
    /// The suite (or its step) was skipped by force before or while it waited: it holds
    /// nothing, and its own run reports the skip at once.
    Skipped,
    Gone,
}

/// Wait for `needs` for one suite and hold them as the step's lease, unless the suite
/// is skipped first: a skip ends the wait, so a skipped suite never takes a slot. On
/// 2026-11-13 #288's test step was skipped while it waited for a compile slot, and it
/// waited on until a slot came, built, and only then noticed.
fn lease_suite(
    c: &Ctx,
    needs: Needs,
    check: &Check,
    expected_ms: Option<u64>,
    label: Option<&str>,
) -> Leased {
    match check() {
        Stop::Skipped => return Leased::Skipped,
        Stop::Gone => return Leased::Gone,
        Stop::No => {}
    }
    if !needs.exclusive && needs.amounts.is_empty() {
        return Leased::Got;
    }
    let wanted = || check() == Stop::No;
    match crate::jobs::acquire_needs_labeled(c, needs, &wanted, false, expected_ms, label) {
        Ok(l) => {
            *c.lease.lock().unwrap_or_else(|e| e.into_inner()) = Some(l);
            Leased::Got
        }
        Err(_) if check() == Stop::Skipped => Leased::Skipped,
        Err(_) => Leased::Gone,
    }
}

/// Wait for `needs` and hold them as the step's lease. `false`: the task went away.
fn lease(c: &Ctx, needs: Needs) -> bool {
    if !needs.exclusive && needs.amounts.is_empty() {
        return true;
    }
    match crate::jobs::acquire_needs(c, needs) {
        Ok(l) => {
            *c.lease.lock().unwrap_or_else(|e| e.into_inner()) = Some(l);
            true
        }
        Err(_) => false,
    }
}

/// The step's `MASON_*` variables, with `MASON_HELD` naming what is leased for the
/// command about to run (not just the step's own `uses`).
/// What the test step does with one suite once narrowing is applied.
enum StepNarrow {
    Whole,
    Run(Box<Suite>, String, Option<Vec<String>>),
    /// The suite is not run; the note says why (it runs whole later).
    NotRun(String),
}

/// The suite's `whole_later` claim, when it holds for this project now: `batch` needs a landing
/// tier that lists the suite, `ci` a `[ci]` section. A claim that does not hold changes nothing.
fn whole_later_holds(c: &Ctx, suite: &Suite) -> Option<mason_testing::WholeLater> {
    let w = suite.whole_later?;
    let claim = [(suite.name.clone(), w.name().to_string())];
    let cfg = crate::landing::load_config(&c.d, c.project_id).ok()?;
    let ci = c.project.services.ci.is_some();
    mason_landing::config::whole_later_problems(&cfg, &claim, ci)
        .is_empty()
        .then_some(w)
}

fn narrow_step(c: &Ctx, suite: &Suite) -> StepNarrow {
    if suite.narrow_packages {
        return match narrow_packages_for_step(c, suite) {
            Some((s, n, g)) => StepNarrow::Run(Box::new(s), n, g),
            None => StepNarrow::Whole,
        };
    }
    let later = whole_later_holds(c, suite);
    narrow_units_for_step(c, suite, later).unwrap_or(StepNarrow::Whole)
}

/// The narrowed copy of `suite` for this task's test step (s386), with a note naming what it
/// runs, or `None` to run it whole: no `narrow_*` keys, the change unreadable, or
/// [`mason_testing::narrow::select`] picks nothing or most of it. With `whole_later`, see
/// [`mason_testing::narrow::select_later`].
fn narrow_units_for_step(
    c: &Ctx,
    suite: &Suite,
    later: Option<mason_testing::WholeLater>,
) -> Option<StepNarrow> {
    let n = suite.narrow.as_ref()?;
    let base = git::run(&c.wt, &["merge-base", &c.project.base_ref(), "HEAD"]).ok()?;
    let base = base.trim();
    let paths = worktree_paths(&c.wt, &c.project.base_ref())?;
    let listed = git::run(&c.wt, &["ls-files"]).ok()?;
    let files: std::collections::BTreeMap<String, String> = listed
        .lines()
        .filter(|p| mason_testing::glob::matches(&n.tests, p))
        .filter_map(|p| Some((p.to_string(), std::fs::read_to_string(c.wt.join(p)).ok()?)))
        .collect();
    let mut added: std::collections::BTreeMap<String, Vec<u32>> = Default::default();
    for p in paths.iter().filter(|p| files.contains_key(*p)) {
        let diff = git::run(&c.wt, &["diff", "-U0", base, "--", p]).unwrap_or_default();
        added.insert(p.clone(), added_lines(&diff));
    }
    let change = mason_testing::narrow::Change {
        paths: &paths,
        added: &added,
    };
    let total = mason_testing::narrow::units(n, &files).len();
    let (picked, case) = match later {
        None => (mason_testing::narrow::select(n, &files, &change)?, None),
        Some(w) => match mason_testing::narrow::select_later(n, &files, &change) {
            mason_testing::narrow::Later::Whole => return None,
            mason_testing::narrow::Later::Run(u, case) => (u, Some((w, case))),
            mason_testing::narrow::Later::NotRun => {
                let why = whole_later_why(n, &change);
                return Some(StepNarrow::NotRun(format!(
                    "{}: not run: {why}; runs whole at {}",
                    suite.name,
                    w.name()
                )));
            }
        },
    };
    let mut s = suite.clone();
    s.command = mason_testing::narrow::command(n, &picked);
    s.min_tests = 1;
    s.narrowed = true;
    let names: Vec<&str> = picked.iter().take(8).map(|u| u.name.as_str()).collect();
    let more = picked.len().saturating_sub(names.len());
    let list = format!(
        "{}{}",
        names.join(", "),
        if more > 0 {
            format!(" and {more} more")
        } else {
            String::new()
        }
    );
    let note = match case {
        None => format!(
            "{}: narrowed to {} of {} test units ({list}); the landing batch runs it whole",
            suite.name,
            picked.len(),
            total
        ),
        Some((w, case)) => format!(
            "{}: narrowed to {} of {} test units ({list}){}; runs whole at {}",
            suite.name,
            picked.len(),
            total,
            case.map_or(String::new(), |c| format!(" because {}", c.describe())),
            w.name()
        ),
    };
    Some(StepNarrow::Run(Box::new(s), note, None))
}

/// Why nothing runs for a suite whose pick is empty and which has no smoke units.
fn whole_later_why(
    n: &mason_testing::Narrow,
    change: &mason_testing::narrow::Change,
) -> &'static str {
    let never = change
        .paths
        .iter()
        .any(|p| n.never.iter().any(|g| mason_testing::glob::matches(g, p)));
    if never {
        "a changed path matches narrow_never, but none of the suite's tests is picked and narrow_smoke is empty"
    } else {
        "nothing in this change reaches its tests"
    }
}

/// The package-mode narrowing of `suite` for a task's test step (s396): its `dev_command`
/// (and `dev_build`) with `{packages}` for the crates the change touches and their dependents,
/// as `run_check` does. `None` (runs whole) when the suite has no `dev_command`, or no package
/// list can be found. Like a test-unit narrowing, it records no evidence.
fn narrow_packages_for_step(
    c: &Ctx,
    suite: &Suite,
) -> Option<(Suite, String, Option<Vec<String>>)> {
    let dc = suite.dev_command.as_ref()?;
    let all = worktree_paths(&c.wt, &c.project.base_ref())?;
    let (paths, dropped) = split_for_suite(&c.def, suite, &all);
    // A `{tests_for_change}` command narrows to test targets and adds the smoke set (decision
    // 0539); a `{packages}` one to whole packages.
    let named = [
        Some(suite.command.as_str()),
        suite.build.as_deref(),
        suite.dev_command.as_deref(),
        suite.dev_build.as_deref(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join("\n");
    let n = crate::scope::narrow_for(&c.wt, &paths, crate::scope::wants_tests(dc), true, &named);
    let n = match n {
        Ok(n) => n,
        Err(why) => {
            // A path that reaches every target (a root file) or no cargo: the suite runs whole.
            eprintln!(
                "suite {}: runs whole because {why} ({} changed paths)",
                suite.name,
                paths.len()
            );
            return None;
        }
    };
    let mut s = suite.clone();
    s.command = crate::scope::expand(dc, &n, false);
    s.dev_shard_command = dev_shard_for(suite, &n);
    if let Some(db) = &suite.dev_build {
        s.build = Some(crate::scope::expand(db, &n, true));
    }
    s.min_tests = 1;
    s.narrowed = true;
    let left = left_out_note(&dropped);
    let note = format!(
        "{}: narrowed to {}{}; the landing batch runs it whole",
        suite.name,
        n.describe(),
        if left.is_empty() {
            String::new()
        } else {
            format!(" ({left})")
        }
    );
    let groups = match &n {
        crate::scope::Narrow::Targets(t) => Some(t.clone()),
        crate::scope::Narrow::Packages(_) => None,
    };
    Some((s, note, groups))
}

/// A batch suite narrowed to the union diff `base..HEAD` in `wt` (s804): its `dev_command` (and
/// `dev_build`) over the affected tests plus the smoke set, the pick a task's test step makes.
/// `Err` says why it runs whole: no `dev_command`, an empty union, or a path that reaches every
/// target (`Cargo.lock`, `Cargo.toml`, `build.rs`, `tests/support/**`, ...). The copy is marked
/// narrowed, so it records no evidence.
pub fn narrow_for_batch(
    def: &Definition,
    suite: &Suite,
    wt: &Path,
    base: &str,
) -> Result<(Suite, String), String> {
    let dc = suite
        .dev_command
        .as_ref()
        .ok_or_else(|| "it has no dev_command".to_string())?;
    let diff = git::run(wt, &["diff", "--name-only", base, "HEAD"]).map_err(|e| e.to_string())?;
    let all: Vec<String> = diff
        .lines()
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect();
    if all.is_empty() {
        return Err("the union changes nothing".into());
    }
    let (paths, dropped) = split_for_suite(def, suite, &all);
    let named = [
        Some(suite.command.as_str()),
        suite.build.as_deref(),
        suite.dev_command.as_deref(),
        suite.dev_build.as_deref(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join("\n");
    let n = crate::scope::narrow_for(wt, &paths, crate::scope::wants_tests(dc), true, &named)?;
    let mut s = suite.clone();
    s.command = crate::scope::expand(dc, &n, false);
    s.dev_shard_command = dev_shard_for(suite, &n);
    if let Some(db) = &suite.dev_build {
        s.build = Some(crate::scope::expand(db, &n, true));
    }
    s.min_tests = 1;
    s.narrowed = true;
    let left = left_out_note(&dropped);
    let note = format!(
        "{}: narrowed to {} over the batch's {} changed file(s){}; the scheduled full test runs it whole",
        suite.name,
        n.describe(),
        all.len(),
        if left.is_empty() {
            String::new()
        } else {
            format!(" ({left})")
        }
    );
    Ok((s, note))
}

/// The new-file line numbers a `git diff -U0` adds or changes (`@@ -a,b +c,d @@`).
fn added_lines(diff: &str) -> Vec<u32> {
    let mut out = vec![];
    for l in diff.lines().filter(|l| l.starts_with("@@")) {
        let Some(plus) = l.split_whitespace().find(|w| w.starts_with('+')) else {
            continue;
        };
        let mut it = plus[1..].split(',');
        let start: u32 = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
        let count: u32 = it.next().and_then(|v| v.parse().ok()).unwrap_or(1);
        out.extend(start..start + count);
    }
    out
}

/// The `CARGO_TARGET_DIR=path` a suite's commands set, when it is a plain path.
fn target_dir_hint(suite: &Suite) -> Option<PathBuf> {
    [Some(suite.command.as_str()), suite.build.as_deref()]
        .into_iter()
        .flatten()
        .flat_map(|t| t.split_whitespace())
        .find_map(|w| w.strip_prefix("CARGO_TARGET_DIR="))
        .map(|v| v.trim_matches(|c| c == '"' || c == '\''))
        .filter(|v| !v.is_empty() && !v.contains('$'))
        .map(PathBuf::from)
}

/// The plan for leaving out of a narrowed run the targets that are green on the same inputs
/// (s707), after the suite's build. `None` when the suite cannot be narrowed by target, the
/// workspace cannot be read, or the suite has known-red tests (a quarantined test is not
/// green, so nothing is reused or recorded around it).
fn reuse_plan(who: &Who, c: &Ctx, suite: &Suite, groups: &[String]) -> Option<crate::reuse::Plan> {
    let dc = suite.dev_command.as_deref()?;
    if !crate::scope::wants_tests(dc)
        || !crate::knownred::known_for(&who.d.store, &suite.name).is_empty()
    {
        return None;
    }
    let env: Vec<(String, String)> = std::env::vars().collect();
    let hint = target_dir_hint(suite);
    let ws = crate::reuse::Workspace::open(&c.wt, hint.as_deref(), &env)?;
    let tag = format!(
        "{}\n{dc}\n{}\n{}",
        suite.name,
        suite.dev_build.as_deref().unwrap_or(""),
        suite.build.as_deref().unwrap_or("")
    );
    let store = &who.d.store;
    Some(crate::reuse::plan(
        &ws,
        &who.project,
        &suite.name,
        groups,
        &tag,
        &|label, key| {
            store
                .target_green(&who.project, &suite.name, label, key)
                .ok()
                .flatten()
        },
    ))
}

/// Is `suite` already green for this exact tree? Then `run_suite` reuses that result without
/// running anything, so it needs no lease (s380).
fn proven(d: &Daemon, tree: &Tree, suite: &str) -> bool {
    matches!(d.store.evidence(&tree.tree, suite), Ok(Some(_)))
}

/// A suite whose run needs `compile` and nothing else, with no build of its own: a build
/// that can run beside others of its kind (s345).
fn compile_only(def: &Definition, step: &Step, suite: &Suite) -> bool {
    let n = run_needs(def, step, suite);
    suite.build.is_none()
        && !n.exclusive
        && !n.amounts.is_empty()
        && n.amounts.keys().all(|k| k == "compile")
}

/// Run `suites` side by side, each in its own thread on its own lease, as [`lease_suite`]
/// and the step's loop would one at a time. The broker decides how many run at once.
fn run_side_by_side(
    c: &Ctx,
    who: &Who,
    lock: &crate::daemon::RepoLock,
    suites: &[Suite],
    tree: &Tree,
) -> std::collections::HashMap<String, SuiteRun> {
    std::thread::scope(|sc| {
        let runs: Vec<_> = suites
            .iter()
            .map(|suite| {
                sc.spawn(move || {
                    let needs = run_needs(&c.def, &c.step, suite);
                    let mut who = who.clone();
                    who.env = held_env(c, &needs);
                    let check = who.check(&suite.name);
                    // A skipped suite holds nothing: `run_suite` sees the skip before it runs.
                    // Nor does one already green for this tree: `run_suite` reuses it (s380).
                    let lease = match check() {
                        Stop::Gone => return (suite.name.clone(), SuiteRun::Gone),
                        Stop::Skipped => None,
                        Stop::No if proven(&c.d, tree, &suite.name) => None,
                        Stop::No => {
                            let wanted = || check() == Stop::No;
                            let expected = estimate(&who, &suite.name, "run");
                            match crate::jobs::acquire_needs_expecting(
                                c, needs, &wanted, true, expected,
                            ) {
                                Ok(l) => Some(l),
                                Err(_) if check() == Stop::Skipped => None,
                                Err(_) => return (suite.name.clone(), SuiteRun::Gone),
                            }
                        }
                    };
                    let ran = run_suite(
                        &who,
                        &c.project.path,
                        lock,
                        &c.def,
                        suite,
                        &c.wt,
                        tree,
                        &mut |_| {},
                    );
                    if lease.is_some() {
                        drop(lease);
                        c.d.admit();
                    }
                    (suite.name.clone(), ran)
                })
            })
            .collect();
        runs.into_iter().filter_map(|h| h.join().ok()).collect()
    })
}

fn held_env(c: &Ctx, held: &Needs) -> Vec<(String, String)> {
    let mut env = crate::jobs::step_env_pairs(c);
    for (k, v) in env.iter_mut() {
        if k == "MASON_HELD" {
            *v = crate::jobs::held_needs_text(held);
        }
    }
    env
}

/// A suite's build, in a worktree, under a step host. Green is exit status 0.
fn run_build(who: &Who, suite: &Suite, build: &str, n: usize, cwd: &Path) -> Built {
    let id = format!("{}-b{n}-{}", who.id_prefix, suite.name);
    let limit = suite.timeout.unwrap_or(Duration::from_secs(1800));
    // Object sets of older crate hashes pile up in a build directory (#476). Every directory a
    // build runs in is prepared: a task's, a landing slot's, `wt`'s (#556).
    if let Some(p) = who.project_cfg() {
        crate::taskcache::prepare_for_build(&who.d, &p, cwd, now_ms());
        if let Some((task, _)) = who.task {
            crate::taskcache::prune_task_objects(&who.d, &p, task.0, now_ms());
        }
    }
    let ran = match run_hosted(
        &who.d,
        &who.project,
        &id,
        build,
        cwd,
        limit,
        &who.log_dir,
        who.check(&suite.name),
        who.run(),
        None,
        who.thresholds(),
        who.env.clone(),
        None,
        &who.cover,
    ) {
        Ok(r) => r,
        Err(e) => return Built::Red(format!("Building `{}` could not run: {e}", suite.name)),
    };
    record_time(who, &suite.name, "build", &ran);
    match ran.stop {
        Stop::Gone => return Built::Gone,
        // A skip is the suite's own run to notice; nothing was built, nothing is blamed.
        Stop::Skipped => return Built::Ok,
        Stop::No => {}
    }
    if ran.timed_out {
        return Built::Red(format!(
            "Building `{}` did not finish within {} s and was stopped. Last output:\n{}",
            suite.name,
            limit.as_secs(),
            tail(&ran.output, 40)
        ));
    }
    if ran.code != 0 || ran.signal != 0 {
        return Built::Red(format!(
            "Building `{}` failed (`{build}`). Last output:\n{}",
            suite.name,
            tail(&ran.output, 40)
        ));
    }
    Built::Ok
}

mod base_red;

/// The test step. A suite that is red in a way the base has too (s246) makes the task wait; once
/// the base is green and folded in, the step runs again from the start.
pub fn run(c: &Ctx) {
    let again = std::cell::Cell::new(false);
    let mut pass = 0u32;
    loop {
        run_once(c, &again, pass);
        if !again.replace(false) {
            return;
        }
        pass += 1;
    }
}

/// `pass` counts the runs of the step in this job: each one names its commands apart, so a host
/// that finished the first pass's command is not mistaken for this pass's.
fn run_once(c: &Ctx, again: &std::cell::Cell<bool>, pass: u32) {
    // As for a test command: test what would land, with the base folded in.
    match crate::jobs::prepare(c) {
        Ok(None) => {}
        Ok(Some(files)) => {
            let base = c.project.base_ref();
            return c.conflict(
                format!(
                    "the branch conflicts with {base} in: {}",
                    files.replace('\n', ", ")
                ),
                crate::jobs::conflict_note(&base, &files),
            );
        }
        Err(why) => return c.fail(why),
    }
    let d = &c.d;
    let log_dir = c.task_dir().join(format!("suites-r{}", c.run.0));
    let mut who = Who {
        d: d.clone(),
        task: Some((c.task, c.run)),
        step: c.step.name.clone(),
        project: c.project.id.clone(),
        log_dir: log_dir.clone(),
        id_prefix: if pass == 0 {
            format!("t{}r{}", c.task.0, c.run.0)
        } else {
            format!("t{}r{}p{pass}", c.task.0, c.run.0)
        },
        sink: None,
        env: vec![],
        trust_base: true,
        members: Members::default(),
        reuse: None,
        cover: Cover::default(),
    };
    let head = |rev: &str| git::run(&c.wt, &["rev-parse", rev]).map(|s| s.trim().to_string());
    let (Ok(tree_hash), Ok(head_sha)) = (head("HEAD^{tree}"), head("HEAD")) else {
        return c.fail("cannot read the change's tree");
    };
    let base_ref = c.project.base_ref();
    let base = git::run(&c.wt, &["merge-base", &base_ref, &head_sha])
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| base_ref.clone());
    let tree = Tree {
        tree: tree_hash,
        base: base.clone(),
    };
    let lock = d.repo_lock(c.project_id);

    let sels = plan(&c.project, &c.def, &c.step, &c.branch);
    // One sub-count per selected suite, each with the total it is expected to reach.
    let subs: Vec<SubProgress> = sels
        .iter()
        .filter(|s| s.selected)
        .map(|s| SubProgress {
            name: s.suite.name.clone(),
            unit: "tests".into(),
            done: Figure::counted(0),
            total: expected_total(d, &c.project.id, &s.suite, &base),
            ..SubProgress::default()
        })
        .collect();
    d.progress_report(c.run, |p, now| {
        // After a restart the stored record already has the finished suites' counts.
        for s in subs {
            if !p.subs.iter().any(|o| o.name == s.name) {
                p.subs.push(s);
            }
        }
        crate::progress::rollup(p);
        p.updated_at_ms = now;
        Change::Urgent
    });
    let mut outcomes: Vec<(String, SuiteOutcome)> = vec![];
    let mut notes: Vec<String> = vec![];
    // Why, for every suite that did not simply pass: the closing note says it.
    let mut details: Vec<String> = vec![];
    let mut head_results: Vec<(Suite, Results)> = vec![];
    let mut waited = false;
    let mut on_wait = |why: Option<String>| match why {
        Some(w) if !waited => {
            waited = c.emit(EventKind::WaitStarted {
                task: c.task,
                on: w,
                class: Some("environment".into()),
            });
        }
        Some(_) => {}
        None => {
            if waited {
                c.emit(EventKind::WaitEnded { task: c.task });
                waited = false;
            }
        }
    };

    // A suite that builds first queues phase by phase: its build holds one `compile` slot and
    // gives it back, then its run holds the rest. The step then holds nothing between suites.
    // So does a step with a suite that needs the whole machine (s340): only that suite's run
    // holds `exclusive`; and one with two or more suites that only build (s345), which run
    // side by side below. Base-red checks still go with builds, as before.
    let builds = sels.iter().any(|s| s.selected && s.suite.build.is_some());
    let chosen: Vec<Suite> = sels
        .iter()
        .filter(|s| s.selected)
        .map(|s| s.suite.clone())
        .collect();
    let per_suite = queues_per_suite(&c.def, &c.step, &chosen);
    if !per_suite {
        // The step holds one lease for all its suites (see `jobs::acquire`).
        let mut needs = needs_of(&c.step.uses, None);
        let more = needs_for(&c.project, &c.def, &c.step, &c.branch);
        needs.exclusive |= more.exclusive;
        for (k, v) in more.amounts {
            let have = needs.amounts.entry(k).or_insert(0);
            *have = (*have).max(v);
        }
        who.env = held_env(c, &needs);
    }
    let project_test =
        crate::jobs::test_commands(d, &c.def, c.project_id, Some(c.task)).map(|t| t.test);
    let mut built: Vec<String> = vec![];
    // A restarted daemon finds the wait it was in: no second run of the suite to learn the same.
    if builds {
        if let Ok(Some(w)) = d.store.base_red(c.task.0) {
            let suite = c
                .def
                .testing
                .suite(&w.suite)
                .and_then(|s| with_project_test(s, project_test.as_deref()).ok());
            match (suite, RedKind::parse(&w.kind)) {
                (Some(suite), Some(kind)) => {
                    let fe = w.first_error.clone();
                    match base_red::gate(c, &who, &lock, &suite, &tree.base, kind, &fe, Some(w)) {
                        base_red::Gate::Cleared => {
                            again.set(true);
                            return;
                        }
                        base_red::Gate::Gone => return,
                        base_red::Gate::NotBase | base_red::Gate::Skipped => {}
                    }
                }
                _ => {
                    let _ = d.store.clear_base_red(c.task.0);
                }
            }
        }
    }
    // Suites whose run needs nothing but `compile` (star's `build-*`) run side by side first,
    // each on its own lease (s345): as many at once as the broker grants `compile` slots. The
    // loop below then takes their results in order, so reds, debts and notes are as before.
    let mut side: std::collections::HashMap<String, SuiteRun> = Default::default();
    if per_suite {
        let builds_only: Vec<Suite> = sels
            .iter()
            .filter(|s| s.selected)
            .filter_map(|s| with_project_test(&s.suite, project_test.as_deref()).ok())
            .filter(|s| compile_only(&c.def, &c.step, s))
            .collect();
        if builds_only.len() > 1 {
            side = run_side_by_side(c, &who, &lock, &builds_only, &tree);
        }
    }
    for sel in &sels {
        let name = sel.suite.name.clone();
        if !sel.selected {
            notes.push(format!("{name}: not run, {}", sel.why));
            outcomes.push((name, SuiteOutcome::NotSelected));
            continue;
        }
        let suite = match with_project_test(&sel.suite, project_test.as_deref()) {
            Ok(s) => s,
            Err(why) => return c.fail(why),
        };
        // Only the tests this change needs, when the suite says how (s386). The landing batch
        // runs it whole.
        let (suite, groups) = match narrow_step(c, &suite) {
            StepNarrow::Run(narrowed, note, groups) => {
                // Said in the closing note too: a narrowed green is not the whole suite's.
                details.push(note.clone());
                notes.push(note);
                // The stored total is the whole suite's size: wrong for a narrowed pick.
                d.progress_sub(c.run, &name, None, forget_whole_total);
                (*narrowed, groups)
            }
            StepNarrow::NotRun(note) => {
                // It runs whole at landing: the step has nothing to prove, and says so.
                details.push(note.clone());
                notes.push(note);
                d.progress_sub(c.run, &name, None, forget_whole_total);
                outcomes.push((name, SuiteOutcome::NotSelected));
                continue;
            }
            StepNarrow::Whole => (suite, None),
        };
        // A narrowed run of short targets takes the small lane; a whole run never does (s649).
        let small = groups
            .as_deref()
            .and_then(|g| small_run(&who, c, &suite, g));
        let ran_beside = side.remove(&name);
        let mut suite = suite;
        let mut reused_whole: Option<usize> = None;
        if per_suite && ran_beside.is_none() {
            c.release_lease();
            let reused = matches!(d.store.evidence(&tree.tree, &suite.name), Ok(Some(_)));
            if let Some(b) = suite
                .build
                .as_ref()
                .filter(|b| !reused && !built.contains(b))
                // On GitHub the build is the first part of the suite's own command.
                .filter(|_| {
                    !crate::place::place_of(d, &c.project.id, Some(c.task.0), &suite, "suite")
                        .is_github()
                })
            {
                let needs = needs_of(&crate::jobs::build_uses(), None);
                who.env = held_env(c, &needs);
                match lease_suite(
                    c,
                    needs,
                    &who.check(&name),
                    estimate(&who, &name, "build"),
                    None,
                ) {
                    Leased::Gone => return,
                    // Nothing is built; the suite's run below reports the skip.
                    Leased::Skipped => {}
                    Leased::Got => {
                        let ran = run_build(&who, &suite, b, built.len(), &c.wt);
                        c.release_lease();
                        match ran {
                            Built::Gone => return,
                            Built::Red(feedback) => {
                                let err = basered::first_error(base_red::output_of(&feedback));
                                match base_red::gate(
                                    c,
                                    &who,
                                    &lock,
                                    &suite,
                                    &tree.base,
                                    RedKind::Build,
                                    &err,
                                    None,
                                ) {
                                    base_red::Gate::Cleared => {
                                        again.set(true);
                                        return;
                                    }
                                    base_red::Gate::Gone => return,
                                    base_red::Gate::Skipped => {
                                        let _ = record_skip_debt(d, c, &name);
                                        notes.push(format!("{name}: skipped by force"));
                                        outcomes.push((name, SuiteOutcome::Skipped));
                                        continue;
                                    }
                                    base_red::Gate::NotBase => {}
                                }
                                outcomes.push((name.clone(), SuiteOutcome::Failed));
                                let (v, note) = closing(&outcomes);
                                let _ = d.store.set_closing(c.task.0, v.name(), &note, now_ms());
                                let reason = format!("the build of suite `{name}` is red");
                                return c.conclude(Outcome::Red, reason, Some(feedback));
                            }
                            Built::Ok => built.push(b.clone()),
                        }
                    }
                }
            }
            // A target green on exactly these inputs is left out of the run (s707).
            who.reuse = None;
            if let (Some(g), true) = (groups.as_deref(), reused_whole.is_none()) {
                if let Some(plan) = reuse_plan(&who, c, &suite, g) {
                    if let Some(note) = plan.note() {
                        details.push(note.clone());
                        notes.push(note);
                    }
                    if plan.all_reused() {
                        reused_whole = Some(plan.reused.len());
                    } else if !plan.reused.is_empty() {
                        if let Some(dc) = suite.dev_command.as_deref() {
                            suite.command = crate::scope::expand(
                                dc,
                                &crate::scope::Narrow::Targets(plan.groups.clone()),
                                false,
                            );
                        }
                    }
                    who.reuse = Some(Arc::new(plan));
                }
            }
            let mut needs = run_needs(&c.def, &c.step, &suite);
            let mut label = None;
            if let Some(ms) = small {
                let before = needs.amounts.clone();
                needs.amounts = crate::jobs::small_lane(&c.def, needs.amounts);
                if needs.amounts != before {
                    let l = crate::targettimes::label(ms);
                    notes.push(format!(
                        "{name}: {l}: queued for {}",
                        crate::jobs::TEST_SMALL
                    ));
                    label = Some(l);
                }
            }
            who.env = held_env(c, &needs);
            // A skipped suite holds nothing: `run_suite` sees the skip before it runs. Nor does
            // one already green for this exact tree, which `run_suite` reuses without running
            // (s380): it must not wait in the queue to learn that.
            if !proven(d, &tree, &name) && reused_whole.is_none() {
                if let Leased::Gone = lease_suite(
                    c,
                    needs,
                    &who.check(&name),
                    estimate(&who, &name, "run"),
                    label.as_deref(),
                ) {
                    return;
                }
            }
        }
        let ran = match ran_beside {
            Some(r) => r,
            None if reused_whole.is_some() => SuiteRun::Done {
                outcome: SuiteOutcome::Reused,
                results: None,
                notes: vec![],
            },
            None => run_suite(
                &who,
                &c.project.path,
                &lock,
                &c.def,
                &suite,
                &c.wt,
                &tree,
                &mut on_wait,
            ),
        };
        who.reuse = None;
        if per_suite {
            c.release_lease();
        }
        match ran {
            SuiteRun::Gone => return,
            SuiteRun::Done {
                outcome,
                results,
                notes: n,
            } => {
                if outcome == SuiteOutcome::Skipped {
                    let _ = record_skip_debt(d, c, &name);
                }
                if !matches!(outcome, SuiteOutcome::Proven | SuiteOutcome::Reused) {
                    details.extend(n.clone());
                } else {
                    // What a lost host's run left reusable is said in the closing note (#621).
                    details.extend(
                        n.iter()
                            .filter(|x| x.contains(": lost host after "))
                            .cloned(),
                    );
                }
                notes.extend(n);
                if let Some(r) = results {
                    head_results.push((sel.suite.clone(), r));
                }
                outcomes.push((name, outcome));
            }
            SuiteRun::Product {
                feedback,
                failing,
                red,
            } => {
                if let (true, Some((kind, err))) = (builds, red) {
                    match base_red::gate(c, &who, &lock, &suite, &tree.base, kind, &err, None) {
                        base_red::Gate::Cleared => {
                            again.set(true);
                            return;
                        }
                        base_red::Gate::Gone => return,
                        base_red::Gate::Skipped => {
                            let _ = record_skip_debt(d, c, &name);
                            notes.push(format!("{name}: skipped by force"));
                            outcomes.push((name, SuiteOutcome::Skipped));
                            continue;
                        }
                        base_red::Gate::NotBase => {}
                    }
                }
                let mut reason = format!("suite `{name}` is red");
                let mut feedback = feedback;
                // What failed is kept for the last round, counted across tasks, and on the
                // last round run at the merge base (s282).
                if !failing.is_empty() {
                    crate::exhausted::record_round(c, &failing);
                    let repeated = crate::exhausted::record_persistent(c, &name, &failing);
                    let cmd_for = |t: &[String]| match &suite.rerun {
                        Some(tpl) => tpl.replace(
                            "{tests}",
                            &t.iter().map(|x| quote(x)).collect::<Vec<_>>().join(" "),
                        ),
                        None => suite.command.clone(),
                    };
                    // The suite's own lease and device are gone by now: a suite with an
                    // environment runs at the base on one too, brought up before the base
                    // runs and torn down after, as its own attempts are.
                    let at_base = |tests: &[String], check: crate::exhausted::BaseCheck| {
                        let dev = lease_dev(&who, &c.def, &suite);
                        let up = match &dev {
                            Ok(dv) => env_up(&who, &c.def, &suite, &c.wt, dv.as_ref()),
                            Err(()) => Err("no device is free for it".to_string()),
                        };
                        let ex = match up {
                            Ok(()) => {
                                let id = dev
                                    .as_ref()
                                    .ok()
                                    .and_then(|x| dev_id(x.as_ref()))
                                    .map(str::to_string);
                                let on_dev = |t: &[String]| {
                                    lifecycle::with_device(&cmd_for(t), id.as_deref())
                                };
                                check(&on_dev)
                            }
                            Err(why) => crate::exhausted::Exhausted {
                                why: Some(format!(
                                    "could not check at the base: `{}` ({why})",
                                    tests.join("`, `")
                                )),
                                ..Default::default()
                            },
                        };
                        if let Ok(dv) = &dev {
                            env_down(&who, &c.def, &suite, &c.wt, dv.as_ref());
                        }
                        ex
                    };
                    if !crate::exhausted::is_last_round(c) && !repeated.is_empty() {
                        // Failing in many tasks is not proof: known red only if red at the
                        // base too (s416).
                        at_base(&repeated, &|on_dev| {
                            crate::exhausted::repeated_at_base(
                                c,
                                &name,
                                suite.build.as_deref(),
                                on_dev,
                                &repeated,
                            )
                        });
                    }
                    if crate::exhausted::is_last_round(c) {
                        let ex = at_base(&failing, &|on_dev| {
                            crate::exhausted::on_last_round(
                                c,
                                &name,
                                suite.build.as_deref(),
                                on_dev,
                                &failing,
                            )
                        });
                        let left = failing.iter().any(|t| !ex.excused.contains(t));
                        if !ex.excused.is_empty() && !left && ex.why.is_none() {
                            // Red at the base too: in the ledger now, and the step goes on.
                            let mut n = vec![format!(
                                "{name}: excused on the last round, red at the merge base too"
                            )];
                            n.extend(ex.lines);
                            details.extend(n.clone());
                            notes.extend(n);
                            outcomes.push((name, SuiteOutcome::Excused));
                            continue;
                        }
                        if let Some(w) = ex.why {
                            reason = format!("{reason}; {w}");
                            feedback = format!("{feedback}\nChecked at the base: {w}\n");
                        }
                    }
                }
                outcomes.push((name.clone(), SuiteOutcome::Failed));
                let (v, note) = closing(&outcomes);
                let _ = d.store.set_closing(c.task.0, v.name(), &note, now_ms());
                return c.conclude(Outcome::Red, reason, Some(feedback));
            }
        }
    }

    // The optional gates run suites of their own: hold what the whole step needs for them.
    if per_suite && !c.step.gates.is_empty() {
        let all = needs_for(&c.project, &c.def, &c.step, &c.branch);
        who.env = held_env(c, &all);
        if !lease(c, all) {
            return;
        }
    }
    // The optional gates, over what the suites found.
    if let Some(feedback) = gates_failed(c, &who, &base, &head_results, &mut notes) {
        let (v, note) = closing(&outcomes);
        let _ = d.store.set_closing(
            c.task.0,
            v.name(),
            &format!("{note}; {}", feedback.lines().next().unwrap_or("")),
            now_ms(),
        );
        return c.conclude(Outcome::Red, "a gate failed".into(), Some(feedback));
    }

    let (verdict, note) = closing(&outcomes);
    let note = if details.is_empty() {
        note
    } else {
        format!("{note} [{}]", details.join("; "))
    };
    let _ = d
        .store
        .set_closing(c.task.0, verdict.name(), &note, now_ms());
    let _ = std::fs::write(c.task_dir().join("test-notes.txt"), notes.join("\n"));
    let _ = std::fs::remove_file(c.task_dir().join("feedback.txt"));
    let _ = Closing::Verified; // (the verdict is recorded; it never blocks a green step)
    c.green();
}

fn record_skip_debt(d: &Arc<Daemon>, c: &Ctx, suite: &str) -> Option<()> {
    // The debt was recorded when the skip was requested; nothing to add.
    let _ = (d, c, suite);
    Some(())
}

/// The gates the step names. Returns feedback if one fails.
fn gates_failed(
    c: &Ctx,
    who: &Who,
    base: &str,
    results: &[(Suite, Results)],
    notes: &mut Vec<String>,
) -> Option<String> {
    let step = &c.step;
    let test_paths: Vec<String> = c
        .def
        .testing
        .suites
        .iter()
        .flat_map(|s| s.test_paths.clone())
        .collect();
    if step.gates.iter().any(|g| g == "assertion_scan") {
        let diff = git::run(&c.wt, &["diff", &format!("{base}..HEAD")]).unwrap_or_default();
        let found = gates::weakening(&diff, &test_paths);
        let message =
            git::run(&c.wt, &["log", "--format=%B", &format!("{base}..HEAD")]).unwrap_or_default();
        if !found.is_empty() && !gates::explains_weakening(&message) {
            let list: Vec<String> = found
                .iter()
                .map(|w| format!("  {}: {}", w.file, w.what))
                .collect();
            return Some(format!(
                "The change weakens assertions and does not say so:\n{}\nIf this is right, say why in the commit message, naming the assertions (it must mention \"assertion\" and give a reason). Otherwise put them back.",
                list.join("\n")
            ));
        }
        notes.push(if found.is_empty() {
            "assertion scan: nothing weakened".into()
        } else {
            "assertion scan: weakened, and explained in the commit message".into()
        });
    }
    if step.gates.iter().any(|g| g == "acceptance") {
        let criteria: Vec<String> = std::fs::read_to_string(c.task_dir().join("plan.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .and_then(|v| {
                v["acceptance_criteria"].as_array().map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect()
                })
            })
            .unwrap_or_default();
        let mapping = std::fs::read_to_string(c.wt.join(".mason/acceptance.json"))
            .map_err(|_| "there is no .mason/acceptance.json".to_string())
            .and_then(|t| gates::parse_acceptance(&t));
        let all = Results::new(results.iter().flat_map(|(_, r)| r.cases.clone()).collect());
        match mapping {
            Err(e) => {
                return Some(format!(
                    "The acceptance gate needs a table of evidence: {e}. Write `.mason/acceptance.json` as {{\"criterion\": [\"test name\", ...]}} for each acceptance criterion in the plan:\n  {}",
                    criteria.join("\n  ")
                ))
            }
            Ok(m) => {
                let gaps = gates::acceptance_table(&criteria, &m, &all);
                if !gaps.is_empty() {
                    let list: Vec<String> = gaps
                        .iter()
                        .map(|g| match g {
                            gates::Gap::Unmapped(c) => format!("  no test is named for: {c}"),
                            gates::Gap::Missing { criterion, test } => format!("  `{test}` (for: {criterion}) did not run"),
                            gates::Gap::NotPassing { criterion, test } => format!("  `{test}` (for: {criterion}) did not pass"),
                        })
                        .collect();
                    return Some(format!("Acceptance criteria without evidence:\n{}", list.join("\n")));
                }
                notes.push("acceptance: every criterion has a passing test".into());
            }
        }
    }
    if step.gates.iter().any(|g| g == "negative_control") {
        for (suite, head) in results {
            let Ok(dev) = lease_dev(who, &c.def, suite) else {
                notes.push(format!(
                    "negative control: not run for `{}`, no free device",
                    suite.name
                ));
                continue;
            };
            let Some((_, base_names)) = baseline(
                who,
                &c.project.path,
                &c.def,
                suite,
                base,
                &c.d.repo_lock(c.project_id),
                dev.as_ref(),
            ) else {
                continue;
            };
            let new = gates::new_tests(head, &base_names);
            if new.is_empty() {
                continue;
            }
            let at_base = run_new_tests_at_base(c, who, suite, base, dev.as_ref());
            let bad = match at_base {
                Some(r) => gates::negative_control(&new, &r),
                None => new.clone(),
            };
            if !bad.is_empty() {
                return Some(format!(
                    "Negative control: these new tests do not fail against the old code, so they prove nothing about the change: {}",
                    bad.join(", ")
                ));
            }
            notes.push(format!(
                "negative control: {} new test(s) in `{}` fail without the change",
                new.len(),
                suite.name
            ));
        }
    }
    None
}

/// Run the suite at the base with the change's test files laid over it.
fn run_new_tests_at_base(
    c: &Ctx,
    who: &Who,
    suite: &Suite,
    base: &str,
    dev: Option<&Dev>,
) -> Option<Results> {
    let wt = who.log_dir.join(format!("negctl-{}", suite.name));
    let lock = c.d.repo_lock(c.project_id);
    {
        let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
        git::remove_worktree(&c.project.path, &wt);
        git::run(
            &c.project.path,
            &["worktree", "add", "--detach", &wt.to_string_lossy(), base],
        )
        .ok()?;
    }
    if let Err(e) = crate::jobs::setup_checkout(&c.def, &wt) {
        tracing::warn!("negative control of {} at {base}: {e}", suite.name);
        let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
        git::remove_worktree(&c.project.path, &wt);
        return None;
    }
    let result = (|| {
        let changed = git::run(&c.wt, &["diff", "--name-only", &format!("{base}..HEAD")]).ok()?;
        for p in changed
            .lines()
            .filter(|p| gates::is_test_path(p, &suite.test_paths))
        {
            let from = c.wt.join(p);
            let to = wt.join(p);
            if let Some(parent) = to.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if from.exists() {
                let _ = std::fs::copy(&from, &to);
            }
        }
        env_up(who, &c.def, suite, &wt, dev).ok()?;
        if let Some(f) = &suite.results_file {
            remove_results(&wt, f);
        }
        let ran = run_hosted(
            &c.d,
            &who.project,
            &format!("{}-n-{}", who.id_prefix, suite.name),
            &crate::jobs::pty_wrap(
                suite.pty,
                &lifecycle::with_device(&suite.command, dev_id(dev)),
            ),
            &wt,
            suite.timeout.unwrap_or(Duration::from_secs(1800)),
            &who.log_dir,
            who.check(&suite.name),
            who.run(),
            None,
            who.thresholds(),
            who.env.clone(),
            None,
            &who.cover,
        )
        .ok()?;
        Some(judge_ran_with(suite, &ran, &wt, who.plugin_parser(suite).as_deref()).results)
    })();
    env_down(who, &c.def, suite, &wt, dev);
    {
        let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
        git::remove_worktree(&c.project.path, &wt);
    }
    result
}

// ---- debts -----------------------------------------------------------------

/// Skip a suite or a whole step by force, now or ahead of time. The task's
/// running suite is stopped within a moment, green lanes keep their results,
/// the closing verdict is downgraded, and a debt is recorded. `tests:all` makes the
/// task immune to testing: every check, suite and test tier is skipped for it until it lands.
pub fn skip(
    d: &Arc<Daemon>,
    task: TaskId,
    scope: &str,
    reason: &str,
    owner: &str,
    deferred: bool,
) -> Result<u64, String> {
    let w = d.world().map_err(|e| e.to_string())?;
    let t = w.tasks.get(&task).ok_or("no such task")?;
    let project = d.project(t.project).ok_or("no such project")?.id.clone();
    if scope == ALL_TESTS {
        if matches!(t.state, State::Landed | State::Canceled) {
            return Err("the task is done; there is no testing left to skip".into());
        }
        if let Some(id) = immune(d, task) {
            return Err(format!(
                "the task is already immune to testing (debt #{id})"
            ));
        }
        return d
            .store
            .add_debt(
                task.0,
                &project,
                ALL_TESTS,
                reason,
                owner,
                deferred,
                now_ms(),
            )
            .map_err(|e| e.to_string());
    }
    let (kind, name) = scope
        .split_once(':')
        .ok_or("scope is `suite:NAME`, `step:NAME` or `tests:all`")?;
    let def = d
        .pinned_def(&t.def)
        .unwrap_or_else(|| d.current_def(t.project));
    // A client that has no step name for the row (a task waiting in `base-red` shows none)
    // leaves it empty: that means the task's test step.
    let test_scope;
    let (scope, name) = if kind == "step" && name.is_empty() {
        let test = test_step_name(&def).ok_or("this project has no test step")?;
        test_scope = format!("step:{test}");
        (test_scope.as_str(), test)
    } else {
        (scope, name.to_string())
    };
    let name = name.as_str();
    match kind {
        "suite" if def.testing.suite(name).is_none() => {
            return Err(format!("there is no suite `{name}`"))
        }
        "step" if def.step(name).is_none() => return Err(format!("there is no step `{name}`")),
        "suite" | "step" => {}
        _ => return Err("scope is `suite:NAME` or `step:NAME`".into()),
    }
    let scope = landing_scope(&def, t.state, scope);
    d.store
        .add_debt(task.0, &project, &scope, reason, owner, deferred, now_ms())
        .map_err(|e| e.to_string())
}

/// The scope a skip is recorded against. A task in landing is shown with its landing step, and
/// the clients send that step's name, but what landing can honour is a skip of the Test step
/// (`tests_skipped`, and a batch's verification, read `step:<Test step>`). So a `step:` scope
/// that names a step other than the Test step is, for a task in landing, the Test step's.
/// Every other scope stays as it was.
pub fn landing_scope(def: &Definition, state: State, scope: &str) -> String {
    let Some(("step", name)) = scope.split_once(':') else {
        return scope.to_string();
    };
    let named_is_test = def
        .step(name)
        .is_some_and(|s| s.kind == mason_definition::StepType::Test);
    match test_step_name(def) {
        Some(test) if state == State::Landing && !named_is_test => format!("step:{test}"),
        _ => scope.to_string(),
    }
}

/// The suites a debt's scope names in `def`. Immunity (`tests:all`) owes the Test step's
/// suites, or every suite when there is no Test step.
///
/// A force or an early Land now (`landing:force`, `landing:land_now`) skipped the landing tier
/// and owes its suites: `tier` (the batch tier's suite names; empty when the project has none,
/// then the Test step's suites).
fn debt_suites(def: &Definition, tier: &[String], scope: &str) -> Vec<Suite> {
    if scope == ALL_TESTS {
        return match test_step_name(def) {
            Some(test) => debt_suites(def, tier, &format!("step:{test}")),
            None => def.testing.suites.clone(),
        };
    }
    let (kind, name) = scope.split_once(':').unwrap_or(("", ""));
    match kind {
        "landing" if matches!(name, "force" | "land_now") => {
            if tier.is_empty() {
                match test_step_name(def) {
                    Some(test) => debt_suites(def, tier, &format!("step:{test}")),
                    None => vec![],
                }
            } else {
                tier.iter()
                    .filter_map(|n| def.testing.suite(n).cloned())
                    .collect()
            }
        }
        "suite" => def.testing.suite(name).cloned().into_iter().collect(),
        "step" => def
            .step(name)
            .map(|s| {
                s.suites
                    .iter()
                    .filter_map(|n| def.testing.suite(n).cloned())
                    .collect()
            })
            .unwrap_or_default(),
        _ => vec![],
    }
}

/// The suite names of the project's landing `batch` tier (empty when it has none).
fn landing_tier(d: &Daemon, pid: ProjectId) -> Vec<String> {
    crate::landing::load_config(d, pid)
        .ok()
        .and_then(|c| c.batch_suites().map(<[String]>::to_vec))
        .unwrap_or_default()
}

/// The task's last commit as the second parent of the oldest merge in `tip`'s history whose
/// subject starts `Merge branch '<branch>'`; `None` when no such merge exists. For a task whose
/// branch has been deleted, so its debt can still be proven landed and paid.
fn merged_branch_commit(path: &std::path::Path, branch: &str, tip: &str) -> Option<String> {
    let subject = format!("Merge branch '{branch}'");
    let out = git::run(
        path,
        &[
            "log",
            "--merges",
            "--fixed-strings",
            "--format=%H%x09%s",
            &format!("--grep={subject}"),
            tip,
        ],
    )
    .ok()?;
    let merge = out
        .lines()
        .filter_map(|l| l.split_once('\t'))
        // The closing quote keeps `t96` from matching `t966`.
        .filter(|(_, s)| s.starts_with(&subject))
        .map(|(h, _)| h.trim().to_string())
        .next_back()?;
    let second = git::run(path, &["rev-parse", &format!("{merge}^2")]).ok()?;
    Some(second.trim().to_string())
}

/// The commit a debt's task landed, when the base branch `tip` contains it. Recorded on the
/// debt the first time it is seen; `None` when the task's branch is gone or never landed, so a
/// debt without proof of landing is never closed.
fn debt_landed(
    d: &Arc<Daemon>,
    p: &crate::config::ProjectCfg,
    row: &mason_store::DebtRow,
    tip: &str,
) -> Option<String> {
    let landed = if row.landed.is_empty() {
        let branch = p.branch(row.task);
        match git::run(
            &p.path,
            &["rev-parse", "--verify", &format!("{branch}^{{commit}}")],
        ) {
            Ok(c) => c.trim().to_string(),
            // The branch is deleted once its task lands; the merge on the base names it still.
            Err(_) => merged_branch_commit(&p.path, &branch, tip)?,
        }
    } else {
        row.landed.clone()
    };
    if !git::is_ancestor(&p.path, &landed, tip) {
        return None;
    }
    if row.landed.is_empty() {
        let _ = d.store.set_debt_landed(row.id, &landed);
    }
    Some(landed)
}

/// The part of a test's name that appears in source: the last `::` or `.` segment.
fn test_leaf(test: &str) -> &str {
    let t = test.rsplit("::").next().unwrap_or(test);
    t.rsplit('.').next().unwrap_or(t).trim()
}

/// Whether the debt's own task added or changed one of these tests: a line its diff against
/// the base adds or removes names the test. Errs toward yes (an unreadable diff counts).
fn task_touched(
    p: &crate::config::ProjectCfg,
    landed: &str,
    tip: &str,
    skipped: &[String],
) -> Option<String> {
    // The first commit on the base's first-parent line that holds the task's commit: a merge
    // (its first parent is the base before the task) or the task's own commit when fast-forwarded.
    let range = format!("{landed}..{tip}");
    let entry = if landed == tip {
        landed.to_string()
    } else {
        match git::run(
            &p.path,
            &["rev-list", "--first-parent", "--ancestry-path", &range],
        ) {
            Ok(l) => match l.lines().last().map(str::trim) {
                // Only a merge that took the task's commit as its second parent is the entry;
                // otherwise the task's commit is itself on the line (fast-forwarded).
                Some(c)
                    if git::run(&p.path, &["rev-parse", &format!("{c}^2")])
                        .is_ok_and(|x| x.trim() == landed) =>
                {
                    c.to_string()
                }
                _ => landed.to_string(),
            },
            Err(_) => return skipped.first().cloned(),
        }
    };
    let Ok(diff) = git::run(&p.path, &["diff", "-U0", &format!("{entry}^1"), &entry]) else {
        return skipped.first().cloned();
    };
    let changed: Vec<&str> = diff
        .lines()
        .filter(|l| {
            (l.starts_with('+') && !l.starts_with("+++"))
                || (l.starts_with('-') && !l.starts_with("---"))
        })
        .collect();
    skipped
        .iter()
        .find(|t| {
            let leaf = test_leaf(t);
            !leaf.is_empty() && changed.iter().any(|l| l.contains(leaf))
        })
        .cloned()
}

/// How one suite stands for a debt whose task landed as `landed`: the proof line when the suite
/// is green at a base holding that commit, either whole or with known-red tests skipped
/// (named in the line), and none when the task itself touched a skipped test.
fn suite_proof(
    d: &Arc<Daemon>,
    p: &crate::config::ProjectCfg,
    row: &mason_store::DebtRow,
    suite: &str,
    landed: &str,
    tip: &str,
) -> Option<String> {
    let short = |b: &str| b[..b.len().min(10)].to_string();
    let greens = d
        .store
        .suite_green_bases(&row.project, suite)
        .unwrap_or_default();
    if let Some((base, _)) = greens
        .iter()
        .find(|(base, _)| git::is_ancestor(&p.path, landed, base))
    {
        return Some(format!(
            "{suite} green on {} at {} since",
            p.base,
            short(base)
        ));
    }
    let skips = d
        .store
        .suite_skip_green_bases(&row.project, suite)
        .unwrap_or_default();
    for (base, skipped) in skips {
        if !git::is_ancestor(&p.path, landed, &base) {
            continue;
        }
        if task_touched(p, landed, tip, &skipped).is_some() {
            continue;
        }
        return Some(format!(
            "{suite} green on {} at {} since, with known-red tests skipped: {}",
            p.base,
            short(&base),
            skipped.join(", ")
        ));
    }
    None
}

/// Whether `name` is a suite the definition no longer has. A definition with no suites at all
/// is taken as unloaded, not as proof that the suite is gone.
fn suite_gone(def: &Definition, name: &str) -> bool {
    !def.testing.suites.is_empty() && def.testing.suite(name).is_none()
}

/// Close every open debt whose suites (all of its step's, for a step) have been recorded
/// green at a base that contains the commit its task landed. A green with known-red tests
/// skipped counts too, unless the debt's own task touched a skipped test; the skipped tests
/// are written in `how`. The proof is written on the debt. Returns how many were closed.
pub fn settle_debts(d: &Arc<Daemon>) -> usize {
    let Ok(rows) = d.store.debts(None, true) else {
        return 0;
    };
    let mut closed = 0;
    for row in rows {
        let Some(pid) = d.project_id_by_name(&row.project) else {
            continue;
        };
        let Some(p) = d.project(pid).cloned() else {
            continue;
        };
        let def = d.current_def(pid);
        if let Some(name) = row.scope.strip_prefix("suite:") {
            if suite_gone(&def, name) {
                let how = format!(
                    "suite `{name}` is no longer in the project's definition, so nothing can pay this debt"
                );
                if d.store.pay_debt(row.id, now_ms(), &how).unwrap_or(false) {
                    closed += 1;
                }
                continue;
            }
        }
        let suites = debt_suites(&def, &landing_tier(d, pid), &row.scope);
        if suites.is_empty() {
            continue;
        }
        let Ok(tip) = git::run(&p.path, &["rev-parse", &p.base_ref()]) else {
            continue;
        };
        let tip = tip.trim().to_string();
        let Some(landed) = debt_landed(d, &p, &row, &tip) else {
            continue;
        };
        let mut proofs = vec![];
        for s in &suites {
            match suite_proof(d, &p, &row, &s.name, &landed, &tip) {
                Some(x) => proofs.push(x),
                None => break,
            }
        }
        if proofs.len() == suites.len() {
            let how = format!(
                "{} (base holds the task's commit {})",
                proofs.join("; "),
                &landed[..landed.len().min(10)]
            );
            if d.store.pay_debt(row.id, now_ms(), &how).unwrap_or(false) {
                closed += 1;
            }
        }
    }
    closed
}

/// The error of a payment that stopped waiting because its daemon is shutting down or its debt
/// was closed by another path. The debt stays as it was; it is not marked tried.
pub const PAY_CANCELLED: &str = "cancelled: the debt was closed or the daemon is shutting down";

/// Whether a payment of `debt` should keep waiting: the daemon runs and the debt is still open.
fn debt_wait_goes_on(d: &Daemon, debt: u64) -> bool {
    if d.is_shutting_down() {
        return false;
    }
    match d.store.debts(None, false) {
        Ok(rows) => rows.iter().any(|x| x.id == debt && x.status == "open"),
        Err(_) => true,
    }
}

/// Hold what the definition's test step `uses` (`compile` and `test-suite` for MASON) while a
/// debt runs, ranked in the debts band (behind all task work), so a payment queues like a
/// test step and never runs beside a full set of them, nor ahead of any task (#835). Nothing to hold gives `Ok(None)`.
fn hold_for_debt(
    d: &Arc<Daemon>,
    def: &Definition,
    project: &str,
    task: TaskId,
    debt: u64,
    at: u64,
) -> Result<Option<crate::sched::Lease>, String> {
    let Some(step) = def
        .steps
        .iter()
        .find(|s| s.kind == mason_definition::StepType::Test)
    else {
        return Ok(None);
    };
    let needs = needs_of(&step.uses, None);
    if !needs.exclusive && needs.amounts.is_empty() {
        return Ok(None);
    }
    // A payment that runs on GitHub holds runner pool units, not this machine's slots (#761).
    let mut _waiting = None;
    let needs = match crate::remote::pool_needs(d, project, &needs, Some(task)) {
        Some(n) => {
            _waiting = Some(crate::remote_runs::waiting(
                d,
                task.0,
                "debt payment",
                &format!("debt #{}", task.0),
            ));
            n
        }
        None => needs,
    };
    if needs.amounts.keys().any(|n| n.starts_with("device:")) {
        d.targets.declare_all(d);
    }
    for name in needs.amounts.keys() {
        if !name.starts_with("llm:") && !name.starts_with("device:") {
            d.broker
                .declare(name, def.resources.get(name).copied().unwrap_or(1));
        }
    }
    // The debts band: behind all task work, the landing band and every step, oldest first.
    let rank = mason_scheduler::Rank::new(false, 0, 0, at, debt, false).debt();
    // Beside: the task may still hold a step's own request (a debt paid by hand).
    d.broker
        .acquire_beside(
            task,
            &format!("debt #{debt} (behind task work)"),
            rank,
            needs,
            &|| debt_wait_goes_on(d, debt),
            &mut |_| {},
        )
        .map(Some)
        .map_err(|_| PAY_CANCELLED.to_string())
}

/// The path-and-module words of red tests: every `::`, `.` or `/` segment but the last of each
/// name, with `-` read as `_`, three characters or more.
fn red_modules(tests: &[String]) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for t in tests {
        let segs: Vec<&str> = t.split(['/', '.']).flat_map(|x| x.split("::")).collect();
        for seg in segs.iter().take(segs.len().saturating_sub(1)) {
            let seg = seg.trim().replace('-', "_");
            if seg.len() >= 3 && !out.contains(&seg) {
                out.push(seg);
            }
        }
    }
    out
}

/// Whether a file in `files` (one path per line) has a path component, extension off, named
/// like one of the `modules`.
fn touches_modules(files: &str, modules: &[String]) -> bool {
    files.lines().any(|f| {
        f.split(['/', '.'])
            .map(|c| c.replace('-', "_"))
            .any(|c| modules.contains(&c))
    })
}

/// Whether the suite's last red record keeps it from running for debts at `tip`: yes at the
/// base it was red at; at a later one only until a landing touched a module of the red tests or
/// `debt_red_backoff` has passed (#774).
fn red_blocks(
    d: &Arc<Daemon>,
    p: &crate::config::ProjectCfg,
    suite: &str,
    tip: &str,
    now: u64,
) -> bool {
    let Ok(Some((base, tests, at))) = d.store.suite_red(&p.id, suite) else {
        return false;
    };
    if now.saturating_sub(at) >= d.cfg.debt_red_backoff.as_millis() as u64 {
        return false;
    }
    if base == tip {
        return true;
    }
    let modules = red_modules(&tests);
    if modules.is_empty() {
        return true;
    }
    match git::run(&p.path, &["diff", "--name-only", &base, tip]) {
        Ok(files) => !touches_modules(&files, &modules),
        // A base that cannot be compared with is no reason to hold the suite back.
        Err(_) => false,
    }
}

/// Mark every open debt of the project that needs `suite` as tried: the suite was red.
fn mark_tried_covering(d: &Arc<Daemon>, project: &str, suite: &str) {
    let Some(pid) = d.project_id_by_name(project) else {
        return;
    };
    let (def, tier) = (d.current_def(pid), landing_tier(d, pid));
    for row in d.store.debts(None, true).unwrap_or_default() {
        if row.project == project
            && !row.scope.starts_with("ci:")
            && debt_suites(&def, &tier, &row.scope)
                .iter()
                .any(|s| s.name == suite)
        {
            let _ = d.store.debt_tried(row.id, now_ms());
        }
    }
}

/// Whether the project's runs go to GitHub and the `gha` pool has no idle unit: a debt run
/// then waits for the next pass instead of queueing behind every task.
fn gha_full(d: &Arc<Daemon>, project: &str) -> bool {
    matches!(
        crate::remote::executor_global(d, project),
        crate::remote::Executor::GitHub { .. }
    ) && d
        .broker
        .limit("gha")
        .is_some_and(|l| d.broker.used("gha") >= l)
}

/// One thing the idle pass may run: a debt to run it for, and which suite of its scope (`None`
/// for a CI debt, which runs as a whole).
struct Pick {
    row: mason_store::DebtRow,
    suite: Option<String>,
    tip: String,
}

/// The next suite some open debt needs and that may run now, or `None`. A suite is needed when
/// a landed debt's scope names it and no proof of it is recorded; it may run when it has not run
/// in this pass at this tip and its red record (if any) does not hold it back.
fn next_pick(d: &Arc<Daemon>, ran: &[(String, String, String)]) -> Option<Result<Pick, ()>> {
    let rows = d.store.debts(None, true).ok()?;
    let now = now_ms();
    for row in rows {
        let Some(pid) = d.project_id_by_name(&row.project) else {
            continue;
        };
        let Some(p) = d.project(pid).cloned() else {
            continue;
        };
        let Ok(tip) = git::run(&p.path, &["rev-parse", &p.base_ref()]) else {
            continue;
        };
        let tip = tip.trim().to_string();
        let Some(landed) = debt_landed(d, &p, &row, &tip) else {
            continue;
        };
        if row.scope.starts_with("ci:") {
            let key = (row.project.clone(), tip.clone(), row.scope.clone());
            if row.tried_at == 0 && !ran.contains(&key) {
                return Some(Ok(Pick {
                    row,
                    suite: None,
                    tip,
                }));
            }
            continue;
        }
        let backoff = d.cfg.debt_red_backoff.as_millis() as u64;
        for s in debt_suites(&d.current_def(pid), &landing_tier(d, pid), &row.scope) {
            if suite_proof(d, &p, &row, &s.name, &landed, &tip).is_some() {
                continue;
            }
            let key = (row.project.clone(), tip.clone(), s.name.clone());
            if ran.contains(&key) {
                continue;
            }
            let allowed = match d.store.suite_red(&p.id, &s.name) {
                Ok(Some(_)) => !red_blocks(d, &p, &s.name, &tip, now),
                // No red on record: a debt that was tried waits out the backoff.
                _ => row.tried_at == 0 || now.saturating_sub(row.tried_at) >= backoff,
            };
            if !allowed {
                continue;
            }
            if gha_full(d, &row.project) {
                return Some(Err(()));
            }
            return Some(Ok(Pick {
                suite: Some(s.name.clone()),
                row,
                tip,
            }));
        }
    }
    None
}

/// Run the suites that open debts need, one at a time, as work below every task: only while the
/// compile queue is empty and a slot is free (and, with remote on, the `gha` pool has an idle
/// unit). The pass picks suites, not debts (#774): each suite runs at most once per base tip
/// however many debts of whatever scope need it, [`settle_debts`] then closes every debt the
/// recorded green covers, and a suite that was red is not run again until a landing touched a
/// module of its red tests or `debt_red_backoff` passed. Only debts whose task landed count.
pub fn run_idle_debts(d: &Arc<Daemon>) {
    if d.idle_run.swap(true, Ordering::SeqCst) {
        return;
    }
    struct Reset(Arc<Daemon>);
    impl Drop for Reset {
        fn drop(&mut self) {
            self.0.idle_run.store(false, Ordering::SeqCst);
        }
    }
    let _reset = Reset(d.clone());
    // (project, tip, suite or CI scope) already run in this pass.
    let mut ran: Vec<(String, String, String)> = vec![];
    loop {
        settle_debts(d);
        let c = d.broker.compilation();
        // A limit of 0 means no `compile` pool is declared: nothing to wait for.
        if !c.pending.is_empty() || (c.limit > 0 && c.used >= c.limit) {
            return;
        }
        // Debt payments go to GitHub only when nothing else waits for a runner (#766).
        if d.broker.gha_waiting() > 0 {
            return;
        }
        let Some(Ok(pick)) = next_pick(d, &ran) else {
            return;
        };
        let (row, tip) = (pick.row, pick.tip);
        let unit = pick.suite.clone().unwrap_or_else(|| row.scope.clone());
        ran.push((row.project.clone(), tip, unit.clone()));
        match pay_with(d, row.id, pick.suite.as_deref()) {
            Ok(m) => tracing::info!(debt = row.id, "idle run: {m}"),
            Err(e) if e == PAY_CANCELLED => {
                tracing::info!(debt = row.id, "idle run: {e}");
                if d.is_shutting_down() {
                    return;
                }
            }
            Err(e) => {
                if pick.suite.is_some() {
                    mark_tried_covering(d, &row.project, &unit);
                } else {
                    let _ = d.store.debt_tried(row.id, now_ms());
                }
                tracing::warn!(debt = row.id, "idle run: {e}");
            }
        }
    }
}

/// `*` matches any run of characters; everything else matches itself.
fn glob_match(pat: &str, text: &str) -> bool {
    let parts: Vec<&str> = pat.split('*').collect();
    if parts.len() == 1 {
        return pat == text;
    }
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    if !text.starts_with(first) || text.len() < first.len() + last.len() || !text.ends_with(last) {
        return false;
    }
    let mut at = first.len();
    let end = text.len() - last.len();
    for mid in &parts[1..parts.len() - 1] {
        match text[at..end].find(mid) {
            Some(i) => at += i + mid.len(),
            None => return false,
        }
    }
    true
}

/// Record suites green from the base branch's own CI runs (#774). A suite with `ci_jobs` is
/// green at a finished run's commit when every listed job pattern matches at least one job of
/// that run and every job it matches succeeded. Looks at the newest ten finished runs, skips a
/// commit already recorded, and settles the debts the new greens cover. Returns how many greens
/// it recorded.
pub fn record_ci_greens(
    d: &Arc<Daemon>,
    pid: ProjectId,
    src: &dyn crate::ci_watch::Source,
) -> usize {
    let Some(p) = d.project(pid).cloned() else {
        return 0;
    };
    let def = d.current_def(pid);
    let mapped: Vec<&Suite> = def
        .testing
        .suites
        .iter()
        .filter(|s| !s.ci_jobs.is_empty())
        .collect();
    if mapped.is_empty() {
        return 0;
    }
    let Ok(runs) = src.runs(&p.base) else {
        return 0;
    };
    let mut recorded = 0;
    for run in runs.iter().filter(|r| r.conclusion.is_some()).take(10) {
        let mut jobs = None;
        for s in &mapped {
            let known = d
                .store
                .suite_green_bases(&p.id, &s.name)
                .unwrap_or_default();
            if known.iter().any(|(b, _)| *b == run.sha) {
                continue;
            }
            if jobs.is_none() {
                match src.jobs(run.id) {
                    Ok(j) => jobs = Some(j),
                    Err(_) => break,
                }
            }
            let jobs = jobs.as_deref().unwrap_or_default();
            let covered = s.ci_jobs.iter().all(|pat| {
                let hit: Vec<_> = jobs.iter().filter(|j| glob_match(pat, &j.name)).collect();
                !hit.is_empty()
                    && hit
                        .iter()
                        .all(|j| j.conclusion.as_deref() == Some("success"))
            });
            if covered {
                let executed = d
                    .store
                    .suite_green(&p.id, &s.name, &run.sha)
                    .ok()
                    .flatten()
                    .unwrap_or(0);
                if d.store
                    .put_suite_green(&p.id, &s.name, &run.sha, executed, now_ms())
                    .is_ok()
                {
                    recorded += 1;
                }
            }
        }
    }
    if recorded > 0 {
        settle_debts(d);
    }
    recorded
}

/// Look at the open debts once an hour (`debt_interval`), the first time soon after start.
pub fn spawn_debts(d: &Arc<Daemon>) {
    let every = d.cfg.debt_interval;
    // A zero interval turns the idle pass off (a test that pays by hand).
    if every.is_zero() {
        return;
    }
    let mut first = true;
    crate::bg::spawn_loop(d, "debts", move |weak| {
        std::thread::sleep(if first {
            every.min(Duration::from_secs(60))
        } else {
            every
        });
        first = false;
        let Some(d) = weak.upgrade() else {
            return false;
        };
        run_idle_debts(&d);
        true
    });
}

/// Pay a debt by running the skipped suite on the base branch's tip. Green
/// pays it; red leaves it open and says why.
pub fn pay(d: &Arc<Daemon>, debt: u64) -> Result<String, String> {
    pay_with(d, debt, None)
}

/// [`pay`], or with `only` set, run just that one suite of the debt's scope on the tip: what the
/// idle pass does, so a suite runs once however many debts need it. A green is recorded for the
/// base and closes the debts it covers through [`settle_debts`]; a red is recorded in `suite_red`.
fn pay_with(d: &Arc<Daemon>, debt: u64, only: Option<&str>) -> Result<String, String> {
    let row = d
        .store
        .debts(None, false)
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|x| x.id == debt)
        .ok_or("no such debt")?;
    if row.status != "open" {
        return Ok("already paid".into());
    }
    let pid = d
        .project_id_by_name(&row.project)
        .ok_or("its project is gone")?;
    let p = d.project(pid).ok_or("its project is gone")?.clone();
    let def = d.current_def(pid);
    if row.scope.starts_with("ci:") {
        return pay_ci(d, &row, &p);
    }
    let mut suites = debt_suites(&def, &landing_tier(d, pid), &row.scope);
    if let Some(name) = only {
        suites.retain(|s| s.name == name);
    }
    if suites.is_empty() {
        return Err("the debt's suite no longer exists in the definition".into());
    }
    // One payment at a time in this daemon. The wait ends when the daemon shuts down or the
    // debt is closed meanwhile; there is no fixed timeout.
    let _one = loop {
        match d.paying.try_lock() {
            Ok(g) => break g,
            Err(std::sync::TryLockError::Poisoned(e)) => break e.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => {
                if d.is_shutting_down() {
                    return Err(PAY_CANCELLED.into());
                }
                // Another payment may have settled this one while it waited.
                if d.store
                    .debts(None, false)
                    .map_err(|e| e.to_string())?
                    .iter()
                    .any(|x| x.id == debt && x.status != "open")
                {
                    return Err(PAY_CANCELLED.into());
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    };
    // Another payment may have settled this one while it waited.
    if d.store
        .debts(None, false)
        .map_err(|e| e.to_string())?
        .iter()
        .any(|x| x.id == debt && x.status != "open")
    {
        return Ok("already paid".into());
    }
    let _lease = hold_for_debt(d, &def, &row.project, TaskId(row.task), debt, row.at)?;
    if p.remote.is_some() {
        let _ = d.fetch_remote(pid);
    }
    let tip = git::run(&p.path, &["rev-parse", &p.base_ref()]).map_err(|e| e.to_string())?;
    let tip = tip.trim().to_string();
    let dir = d.cfg.task_dir(row.task).join(format!("debt-{debt}"));
    let wt = dir.join("wt");
    let lock = d.repo_lock(pid);
    {
        let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
        git::remove_worktree(&p.path, &wt);
        git::run(
            &p.path,
            &["worktree", "add", "--detach", &wt.to_string_lossy(), &tip],
        )
        .map_err(|e| e.to_string())?;
    }
    if let Err(e) = crate::jobs::setup_checkout(&def, &wt) {
        let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
        git::remove_worktree(&p.path, &wt);
        return Err(e);
    }
    let who = Who {
        d: d.clone(),
        task: None,
        step: String::new(),
        project: p.id.clone(),
        log_dir: dir.clone(),
        id_prefix: format!("debt{debt}"),
        sink: None,
        env: vec![],
        trust_base: true,
        members: Members::default(),
        reuse: None,
        cover: Cover::default(),
    };
    let tree_hash = git::run(&wt, &["rev-parse", "HEAD^{tree}"])
        .map(|s| s.trim().to_string())
        .map_err(|e| e.to_string())?;
    let tree = Tree {
        tree: tree_hash,
        base: tip.clone(),
    };
    let mut verdicts = vec![];
    let mut all_green = true;
    // The tests a red run named, for the backoff (#774).
    let mut red_tests: Vec<String> = vec![];
    let mut red_suite: Option<String> = None;
    for s in &suites {
        let r = run_suite(&who, &p.path, &lock, &def, s, &wt, &tree, &mut |_| {});
        match r {
            SuiteRun::Done {
                outcome: SuiteOutcome::Proven | SuiteOutcome::Reused,
                results,
                notes,
            } => {
                // A deferred debt runs on the base branch's tip: what passed there is proven.
                if let Some(r) = &results {
                    lift_proven(d, &s.name, r);
                }
                verdicts.push(notes.join("; "))
            }
            SuiteRun::Done { notes, .. } => {
                all_green = false;
                red_suite.get_or_insert(s.name.clone());
                verdicts.push(notes.join("; "));
            }
            SuiteRun::Product {
                feedback, failing, ..
            } => {
                all_green = false;
                red_suite.get_or_insert(s.name.clone());
                red_tests.extend(failing);
                verdicts.push(feedback.lines().next().unwrap_or("red").to_string());
            }
            SuiteRun::Gone => all_green = false,
        }
    }
    {
        let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
        git::remove_worktree(&p.path, &wt);
    }
    if let Some(name) = &red_suite {
        let _ = d
            .store
            .put_suite_red(&row.project, name, &tip, &red_tests, now_ms());
    }
    if all_green {
        for s in &suites {
            let _ = d.store.clear_suite_red(&row.project, &s.name);
        }
    }
    let _ = who.project;
    // Tests a green run skipped as known red at this tip. A debt whose own task touched one of
    // them is not paid by a run that never ran it (s476).
    let mut skipped_at_tip: Vec<String> = vec![];
    if all_green {
        for s in &suites {
            let skips = d
                .store
                .suite_skip_green_bases(&row.project, &s.name)
                .unwrap_or_default();
            if let Some((_, sk)) = skips.into_iter().find(|(b, _)| *b == tip) {
                skipped_at_tip.extend(sk);
            }
        }
        if let Some(landed) = debt_landed(d, &p, &row, &tip) {
            if let Some(t) = task_touched(&p, &landed, &tip, &skipped_at_tip) {
                let _ = d.store.debt_tried(debt, now_ms());
                return Err(format!(
                    "still owed: the task touched `{t}`, which the run skipped as known red"
                ));
            }
        }
    }
    if all_green && only.is_some() {
        // The green is recorded for the tip; every debt it covers closes in `settle_debts`.
        return Ok(format!("suite green at {}", &tip[..tip.len().min(10)]));
    }
    if all_green {
        let mut how = format!(
            "green when run on {} at {}",
            p.base_ref(),
            &tip[..tip.len().min(10)]
        );
        if !skipped_at_tip.is_empty() {
            skipped_at_tip.sort();
            skipped_at_tip.dedup();
            how.push_str(&format!(
                ", with known-red tests skipped: {}",
                skipped_at_tip.join(", ")
            ));
        }
        d.store
            .pay_debt(debt, now_ms(), &how)
            .map_err(|e| e.to_string())?;
        Ok(format!("paid: {}", verdicts.join("; ")))
    } else {
        let _ = d.store.debt_tried(debt, now_ms());
        Err(format!("still owed: {}", verdicts.join("; ")))
    }
}

// ---- the scheduled full test: every listed suite, whole, on the base tip (#783) ----------------

/// How one suite of a scheduled full test turned out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WholeState {
    /// Green, and recorded green at the tip.
    Green,
    /// Green at the tip already (a recorded green), so not run again.
    AlreadyGreen,
    /// Red, then green on the immediate rerun: a flake.
    Flaky,
    /// Failed only on quarantined or known-red tests: set aside, no green recorded.
    Excused,
    /// Red (after the rerun), or the suite could not be judged.
    Red,
}

/// One suite's part of a [`Whole`] run.
#[derive(Clone, Debug)]
pub struct WholeSuite {
    pub name: String,
    pub state: WholeState,
    /// What the run said, for the status and for a ticket.
    pub note: String,
    /// The tests that failed (red only).
    pub failing: Vec<String>,
}

/// What a scheduled full test did: the tip it ran on and each suite's part.
#[derive(Clone, Debug)]
pub struct Whole {
    pub tip: String,
    pub suites: Vec<WholeSuite>,
}

impl Whole {
    pub fn red(&self) -> bool {
        self.suites.iter().any(|s| s.state == WholeState::Red)
    }
}

/// The suites a `suites` schedule runs when it names none: every suite bound to the Test step
/// and to the landing tiers, or every suite when the definition has no Test step.
pub fn default_full_suites(d: &Daemon, pid: ProjectId, def: &Definition) -> Vec<String> {
    let mut names: Vec<String> = match test_step_name(def) {
        Some(test) => debt_suites(def, &[], &format!("step:{test}"))
            .into_iter()
            .map(|s| s.name)
            .collect(),
        None => def.testing.suites.iter().map(|s| s.name.clone()).collect(),
    };
    for n in landing_tier(d, pid) {
        if !names.contains(&n) {
            names.push(n);
        }
    }
    names
}

/// Hold what the Test step `uses` as work below every task: the lowest rank there is, so a
/// task's step never waits behind it. With remote on the units are `gha` pool units (#761).
fn hold_below_all(
    d: &Arc<Daemon>,
    def: &Definition,
    project: &str,
    label: &str,
    alive: &dyn Fn() -> bool,
) -> Result<Option<crate::sched::Lease>, String> {
    let Some(step) = def
        .steps
        .iter()
        .find(|s| s.kind == mason_definition::StepType::Test)
    else {
        return Ok(None);
    };
    let needs = needs_of(&step.uses, None);
    if !needs.exclusive && needs.amounts.is_empty() {
        return Ok(None);
    }
    let needs = crate::remote::pool_needs(d, project, &needs, None).unwrap_or(needs);
    if needs.amounts.keys().any(|n| n.starts_with("device:")) {
        d.targets.declare_all(d);
    }
    for name in needs.amounts.keys() {
        if !name.starts_with("llm:") && !name.starts_with("device:") {
            d.broker
                .declare(name, def.resources.get(name).copied().unwrap_or(1));
        }
    }
    let rank = mason_scheduler::Rank::new(false, u32::MAX, u8::MAX, now_ms(), 0, false);
    d.broker
        .acquire_beside(TaskId(0), label, rank, needs, alive, &mut |_| {})
        .map(Some)
        .map_err(|_| PAY_CANCELLED.to_string())
}

/// Run `names` (suites of the definition) whole on the base branch's tip, one after another, on
/// a clean checkout. Each goes through [`run_suite`], so the quarantine, the immediate rerun of
/// a red test, the judge and the executor (GitHub shards when remote is on, this Mac when it is
/// off) are the ones a debt payment uses. A suite that was green is recorded green at the tip,
/// which closes every debt it covers ([`settle_debts`]); a red is recorded in `suite_red`.
/// `at` is the commit to run on (the base tip the caller read); `force` runs a suite even when
/// a green at it is on record. Waits behind every task for
/// its slots and behind a running payment; `alive` says whether to go on waiting.
pub fn run_whole(
    d: &Arc<Daemon>,
    pid: ProjectId,
    names: &[String],
    at: &str,
    force: bool,
    alive: &dyn Fn() -> bool,
) -> Result<Whole, String> {
    let p = d.project(pid).ok_or("its project is gone")?.clone();
    let def = d.current_def(pid);
    let names: Vec<String> = if names.is_empty() {
        default_full_suites(d, pid, &def)
    } else {
        names.to_vec()
    };
    if names.is_empty() {
        return Err("the definition has no suites to run".into());
    }
    let mut suites = vec![];
    for n in &names {
        match def.testing.suite(n) {
            Some(s) => suites.push(s.clone()),
            None => return Err(format!("the definition has no suite `{n}`")),
        }
    }
    // One payment or full test at a time in this daemon.
    let _one = loop {
        match d.paying.try_lock() {
            Ok(g) => break g,
            Err(std::sync::TryLockError::Poisoned(e)) => break e.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => {
                if d.is_shutting_down() || !alive() {
                    return Err(PAY_CANCELLED.into());
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    };
    let _lease = hold_below_all(d, &def, &p.id, "scheduled full test", alive)?;
    // The commit the caller decided on; nothing is fetched here, so the record and the run agree.
    let tip = git::run(&p.path, &["rev-parse", &format!("{at}^{{commit}}")])
        .map_err(|e| e.to_string())?
        .trim()
        .to_string();
    let mut out = Whole {
        tip: tip.clone(),
        suites: vec![],
    };
    let mut todo = vec![];
    for s in suites {
        let known = d
            .store
            .suite_green_bases(&p.id, &s.name)
            .unwrap_or_default();
        if !force && known.iter().any(|(b, _)| *b == tip) {
            out.suites.push(WholeSuite {
                name: s.name.clone(),
                state: WholeState::AlreadyGreen,
                note: format!("{}: already green at {}", s.name, &tip[..tip.len().min(10)]),
                failing: vec![],
            });
        } else {
            todo.push(s);
        }
    }
    if todo.is_empty() {
        return Ok(out);
    }
    let dir = d.cfg.task_dir(0).join(format!("full-{}", p.id));
    let wt = dir.join("wt");
    let lock = d.repo_lock(pid);
    {
        let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
        git::remove_worktree(&p.path, &wt);
        git::run(
            &p.path,
            &["worktree", "add", "--detach", &wt.to_string_lossy(), &tip],
        )
        .map_err(|e| e.to_string())?;
    }
    if let Err(e) = crate::jobs::setup_checkout(&def, &wt) {
        let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
        git::remove_worktree(&p.path, &wt);
        return Err(e);
    }
    let who = Who {
        d: d.clone(),
        task: None,
        step: String::new(),
        project: p.id.clone(),
        log_dir: dir.clone(),
        id_prefix: format!("full-{}", p.id),
        sink: None,
        env: vec![],
        // The tip is the tree under test, so no red is excused as "also red at the base".
        trust_base: false,
        members: Members::default(),
        reuse: None,
        cover: Cover::default(),
    };
    let tree_hash = git::run(&wt, &["rev-parse", "HEAD^{tree}"])
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let tree = Tree {
        tree: tree_hash,
        base: tip.clone(),
    };
    for s in &todo {
        let r = run_suite(&who, &p.path, &lock, &def, s, &wt, &tree, &mut |_| {});
        let part = |state, note: String, failing: Vec<String>| WholeSuite {
            name: s.name.clone(),
            state,
            note,
            failing,
        };
        let part = match r {
            SuiteRun::Done {
                outcome: SuiteOutcome::Proven | SuiteOutcome::Reused,
                results,
                notes,
            } => {
                if let Some(r) = &results {
                    lift_proven(d, &s.name, r);
                }
                let _ = d.store.clear_suite_red(&p.id, &s.name);
                part(WholeState::Green, notes.join("; "), vec![])
            }
            SuiteRun::Done {
                outcome: SuiteOutcome::Flaky,
                notes,
                ..
            } => part(WholeState::Flaky, notes.join("; "), vec![]),
            SuiteRun::Done {
                outcome: SuiteOutcome::Excused | SuiteOutcome::NotSelected | SuiteOutcome::Skipped,
                notes,
                ..
            } => part(WholeState::Excused, notes.join("; "), vec![]),
            SuiteRun::Done { notes, .. } => part(WholeState::Red, notes.join("; "), vec![]),
            SuiteRun::Product {
                feedback, failing, ..
            } => part(WholeState::Red, feedback, failing),
            SuiteRun::Gone => part(WholeState::Red, format!("{}: went away", s.name), vec![]),
        };
        if part.state == WholeState::Red {
            let _ = d
                .store
                .put_suite_red(&p.id, &s.name, &tip, &part.failing, now_ms());
        }
        out.suites.push(part);
    }
    {
        let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
        git::remove_worktree(&p.path, &wt);
    }
    // Every green recorded at the tip closes the debts it covers.
    settle_debts(d);
    Ok(out)
}

/// The name a CI debt's scope carries: the workflow when the CI system names one, else `ci`.
pub fn ci_scope(p: &crate::config::ProjectCfg) -> String {
    let name = match p.services.ci.as_ref().map(|c| &c.kind) {
        Some(crate::config::CiKind::GithubActions {
            workflow: Some(w), ..
        }) => w.clone(),
        _ => "ci".to_string(),
    };
    format!("ci:{name}")
}

/// Pay a CI debt (`ci:<name>`, left by Land now): push the base branch's tip under a
/// candidate name, wait for CI on it, and delete the name again. Green pays the debt; red
/// leaves it owed.
fn pay_ci(
    d: &Arc<Daemon>,
    row: &mason_store::DebtRow,
    p: &crate::config::ProjectCfg,
) -> Result<String, String> {
    let (ci, cfg) = crate::services::ci(p)?.ok_or("the project has no [ci] section any more")?;
    let remote = p
        .remote
        .clone()
        .ok_or("the project has no remote to run CI on")?;
    let pid = d.project_id_by_name(&p.id).ok_or("its project is gone")?;
    let _ = d.fetch_remote(pid);
    let tip = git::run(&p.path, &["rev-parse", &p.base_ref()]).map_err(|e| e.to_string())?;
    let tip = tip.trim().to_string();
    let name = format!("mason/ci/{}-debt{}", p.id, row.id);
    git::run(
        &p.path,
        &[
            "push",
            "--porcelain",
            &remote,
            &format!("{tip}:refs/heads/{name}"),
        ],
    )
    .map_err(|e| format!("could not push {name}: {e}"))?;
    let started = Instant::now();
    let state = (|| -> Result<mason_adapter_api::service::CiState, String> {
        let run = ci.trigger(&name, &tip).map_err(|e| e.to_string())?;
        loop {
            match ci.status(&run).map_err(|e| e.to_string())? {
                mason_adapter_api::service::CiState::Pending => {
                    if started.elapsed().as_secs() > cfg.timeout_s {
                        return Err(format!("CI still not finished after {} s", cfg.timeout_s));
                    }
                    std::thread::sleep(Duration::from_secs(cfg.poll_s.max(1)));
                }
                done => return Ok(done),
            }
        }
    })();
    let _ = git::run(
        &p.path,
        &["push", &remote, "--delete", &format!("refs/heads/{name}")],
    );
    match state {
        Ok(mason_adapter_api::service::CiState::Green) => {
            let how = format!(
                "CI green when run on {} at {}",
                p.base_ref(),
                &tip[..tip.len().min(10)]
            );
            d.store
                .pay_debt(row.id, now_ms(), &how)
                .map_err(|e| e.to_string())?;
            Ok(format!("paid: {how}"))
        }
        Ok(mason_adapter_api::service::CiState::Red { summary, .. }) => {
            let _ = d.store.debt_tried(row.id, now_ms());
            Err(format!("still owed: CI is red on the base: {summary}"))
        }
        Ok(_) => Err("CI did not answer".into()),
        Err(e) => {
            let _ = d.store.debt_tried(row.id, now_ms());
            Err(format!("still owed: {e}"))
        }
    }
}

/// One deferred debt for each of `tasks` and each scope in `scopes` that it does not owe yet:
/// what Land now stopped, to be run on the base after the merge.
pub fn defer_stopped(d: &Daemon, project: &str, tasks: &[u64], scopes: &[String]) {
    let owner = d.cfg.owner_names.first().cloned().unwrap_or_default();
    for t in tasks {
        let open = d.store.debts(Some(*t), true).unwrap_or_default();
        for sc in scopes {
            if open.iter().any(|x| &x.scope == sc) {
                continue;
            }
            let _ = d.store.add_debt(
                *t,
                project,
                sc,
                "Land now: skipped to land past slow testing; runs after the merge",
                &owner,
                true,
                now_ms(),
            );
        }
    }
}

/// After a task lands, run what it skipped and asked to have run later.
pub fn after_landing(d: &Arc<Daemon>, task: TaskId) {
    // The ledger's tests this task was filed for run once at the new base.
    if d.store.ledger().is_ok_and(|l| {
        l.iter()
            .any(|r| r.fix_task == Some(task.0) && r.source != "owner")
    }) {
        let d2 = d.clone();
        let _ = std::thread::Builder::new()
            .name(format!("ledger-{}", task.0))
            .spawn(move || match ledger_run(&d2, task) {
                Ok(m) => tracing::info!(task = task.0, "{m}"),
                Err(e) => tracing::warn!(task = task.0, "ledger run: {e}"),
            });
    }
    // The commit this task landed, kept on each of its open debts: the task's branch may be
    // gone by the time a later green could close them.
    if let Ok(rows) = d.store.debts(Some(task.0), true) {
        for r in rows.iter().filter(|r| r.landed.is_empty()) {
            let p = d
                .project_id_by_name(&r.project)
                .and_then(|id| d.project(id).cloned());
            if let Some(c) = p.and_then(|p| {
                git::run(
                    &p.path,
                    &[
                        "rev-parse",
                        "--verify",
                        &format!("{}^{{commit}}", p.branch(task.0)),
                    ],
                )
                .ok()
            }) {
                let _ = d.store.set_debt_landed(r.id, c.trim());
            }
        }
    }
    // A landing may be what a suite's green was waiting for: close what is now proven.
    let d3 = d.clone();
    let _ = std::thread::Builder::new()
        .name(format!("debts-settle-{}", task.0))
        .spawn(move || {
            let n = settle_debts(&d3);
            if n > 0 {
                tracing::info!(closed = n, "debts closed by a later green");
            }
        });
    let Ok(rows) = d.store.debts(Some(task.0), true) else {
        return;
    };
    // Every open debt of the landed task is paid, not only the deferred ones: a skipped
    // check is run once at the new base, so a skip never stays owed for good (s426).
    // One thread pays them in turn, and `pay` itself runs one at a time behind the test
    // step's slots: a batch of test-skipped tasks must not start a full run per debt at once.
    let d = d.clone();
    let _ = std::thread::Builder::new()
        .name(format!("debts-{}", task.0))
        .spawn(move || {
            for r in rows {
                match pay(&d, r.id) {
                    Ok(m) => tracing::info!(debt = r.id, "{m}"),
                    Err(e) => tracing::warn!(debt = r.id, "deferred run: {e}"),
                }
                settle_debts(&d);
            }
        });
}

/// Run the ledger's tests for fix `task` once on the base branch's tip. A test that passes
/// is lifted. One still red reopens its entry with a note beside the fix task and gets a
/// follow-up task that links it. A test the run did not execute stays as it is.
pub fn ledger_run(d: &Arc<Daemon>, task: TaskId) -> Result<String, String> {
    let rows: Vec<_> = d
        .store
        .ledger()
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|r| r.fix_task == Some(task.0) && r.source != "owner")
        .collect();
    if rows.is_empty() {
        return Ok("nothing in the ledger for this task".into());
    }
    let pid = d
        .world()
        .map_err(|e| e.to_string())?
        .tasks
        .get(&task)
        .map(|t| t.project)
        .ok_or("no such task")?;
    let p = d.project(pid).ok_or("its project is gone")?.clone();
    let def = d.current_def(pid);
    if p.remote.is_some() {
        let _ = d.fetch_remote(pid);
    }
    let tip = git::run(&p.path, &["rev-parse", &p.base_ref()])
        .map_err(|e| e.to_string())?
        .trim()
        .to_string();
    let dir = d.cfg.task_dir(task.0).join("ledger");
    let wt = dir.join("wt");
    let lock = d.repo_lock(pid);
    {
        let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
        git::remove_worktree(&p.path, &wt);
        git::run(
            &p.path,
            &["worktree", "add", "--detach", &wt.to_string_lossy(), &tip],
        )
        .map_err(|e| e.to_string())?;
    }
    if let Err(e) = crate::jobs::setup_checkout(&def, &wt) {
        let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
        git::remove_worktree(&p.path, &wt);
        return Err(e);
    }
    let who = Who {
        d: d.clone(),
        task: None,
        step: String::new(),
        project: p.id.clone(),
        log_dir: dir.clone(),
        id_prefix: format!("ledger{}", task.0),
        sink: None,
        env: vec![],
        trust_base: true,
        members: Members::default(),
        reuse: None,
        cover: Cover::default(),
    };
    let tree = Tree {
        tree: git::run(&wt, &["rev-parse", "HEAD^{tree}"])
            .map(|s| s.trim().to_string())
            .map_err(|e| e.to_string())?,
        base: tip.clone(),
    };
    let mut report = vec![];
    let mut suites: Vec<String> = rows.iter().map(|r| r.suite.clone()).collect();
    suites.dedup();
    for name in suites {
        let tests: Vec<String> = rows
            .iter()
            .filter(|r| r.suite == name)
            .map(|r| r.test.clone())
            .collect();
        let Some(suite) = def.testing.suite(&name) else {
            report.push(format!(
                "{name}: not a suite of the definition; left in place"
            ));
            continue;
        };
        // A separate name, so the evidence and the base cache of the whole suite are not
        // taken by a run of a few tests.
        let mut s = suite.clone();
        s.name = format!("{name}~ledger");
        s.min_tests = 1;
        if let Some(t) = &suite.rerun {
            s.command = t.replace(
                "{tests}",
                &tests.iter().map(|f| quote(f)).collect::<Vec<_>>().join(" "),
            );
        }
        let r = run_suite(&who, &p.path, &lock, &def, &s, &wt, &tree, &mut |_| {});
        let red = match r {
            SuiteRun::Done {
                outcome: SuiteOutcome::Proven,
                results: Some(res),
                ..
            } => {
                lift_proven(d, &name, &res);
                report.push(format!("{name}: green"));
                continue;
            }
            SuiteRun::Done { notes, .. } => notes.join("; "),
            SuiteRun::Product { feedback, .. } => feedback,
            SuiteRun::Gone => {
                report.push(format!("{name}: stopped"));
                continue;
            }
        };
        let seen_tail = tail(&red, 30);
        for t in tests.iter().filter(|t| red.contains(t.as_str())) {
            let note = format!(
                "task #{} landed but this test is still red at {}",
                task.0,
                &tip[..tip.len().min(10)]
            );
            let _ = d.store.ledger_reopen(&name, t, &note, now_ms());
            let _ = std::fs::write(
                d.cfg.task_dir(task.0).join(format!(
                    "ledger-reopened-{}.txt",
                    t.replace(['/', ':'], "_")
                )),
                &note,
            );
            let seen = crate::basecheck::Seen {
                project: pid,
                project_name: &p.id,
                task: 0,
                step: &name,
                base: &tip,
                cmd: &suite.command,
                log_tail: &seen_tail,
                follows: Some(task.0),
            };
            crate::basecheck::follow_up(d, &name, &seen, t);
        }
        report.push(format!("{name}: red"));
    }
    {
        let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
        git::remove_worktree(&p.path, &wt);
    }
    Ok(report.join("; "))
}

/// The `why` of a run that was stopped at its time limit (s702).
pub const RAN_OUT_OF_TIME: &str = "it ran out of time";

/// Starts the line of a timed-out run's feedback that names the tests still running at the kill.
const STILL_RUNNING: &str = "Still running at the kill: ";

/// The tests libtest reported as "has been running for over N seconds" that never printed a
/// result: what a run killed at its limit was still doing (s702).
pub fn still_running(output: &str) -> Vec<String> {
    let mut slow: Vec<String> = vec![];
    let mut done: std::collections::BTreeSet<&str> = Default::default();
    for l in output.lines() {
        let Some(rest) = l.trim().strip_prefix("test ") else {
            continue;
        };
        if let Some((name, _)) = rest.split_once(" has been running for over ") {
            if !slow.iter().any(|n| n == name) {
                slow.push(name.to_string());
            }
        } else if let Some((name, _)) = rest.split_once(" ... ") {
            done.insert(name);
        }
    }
    slow.retain(|n| !done.contains(n.as_str()));
    slow
}

/// The names a timed-out run's feedback lists as still running, when the feedback is a timeout's
/// (`None` otherwise).
fn timeout_names(feedback: &str) -> Option<Vec<String>> {
    feedback
        .lines()
        .any(|l| l.starts_with('`') && l.contains(&format!("({RAN_OUT_OF_TIME})")))
        .then(|| {
            feedback
                .lines()
                .find_map(|l| l.strip_prefix(STILL_RUNNING))
                .map(|r| {
                    r.trim_end_matches('.')
                        .split(", ")
                        .filter(|n| !n.is_empty())
                        .map(String::from)
                        .collect()
                })
                .unwrap_or_default()
        })
}

/// What running suites over a batch's integration branch said.
#[derive(Default)]
pub struct OverRun {
    pub green: bool,
    /// The suites that were red.
    pub failed: std::collections::BTreeSet<String>,
    /// One line per suite, and the feedback of a red.
    pub notes: Vec<String>,
    /// The failing tests of each red suite that named some (a red that read none, a build
    /// failure or a timeout has no entry): what a rerun or a bisect can run alone (#682).
    pub failing: std::collections::BTreeMap<String, Vec<String>>,
    /// The suites that were stopped at their time limit with no failing test named, each with
    /// the tests still running at the kill (possibly none read): a timeout is not a red to
    /// bisect (s702).
    pub unfinished: std::collections::BTreeMap<String, Vec<String>>,
}

static LAST_FULL: std::sync::Mutex<Vec<(String, u64)>> = std::sync::Mutex::new(Vec::new());

/// Is a full run (every target, reuse off) due for this project? True at the first batch after
/// the daemon starts and again once `hours` have passed since the last one; 0 hours: always.
fn full_run_due(project: &str, hours: u64) -> bool {
    let g = LAST_FULL.lock().unwrap_or_else(|e| e.into_inner());
    match g.iter().find(|(p, _)| p == project) {
        None => true,
        Some((_, at)) => now_ms().saturating_sub(*at) >= hours.saturating_mul(3_600_000),
    }
}

fn note_full_run(project: &str) {
    let mut g = LAST_FULL.lock().unwrap_or_else(|e| e.into_inner());
    g.retain(|(p, _)| p != project);
    g.push((project.to_string(), now_ms()));
}

/// A batch's reuse (s707): build every package's targets, then leave out of the run those green
/// on the same inputs (`full`: none, but each is recorded). `None` runs the suite as it is.
fn batch_reuse(
    who: &Who,
    p: &crate::config::ProjectCfg,
    def: &Definition,
    suite: &Suite,
    wt: &Path,
    full: bool,
) -> Option<(Suite, crate::reuse::Plan)> {
    let d = &who.d;
    let test = d
        .project_id_by_name(&p.id)
        .and_then(|pid| crate::jobs::test_commands(d, def, pid, None))
        .map(|t| t.test);
    let s = with_project_test(suite, test.as_deref()).ok()?;
    let dc = s
        .dev_command
        .as_deref()
        .filter(|c| crate::scope::wants_tests(c))?;
    let env: Vec<(String, String)> = std::env::vars().collect();
    let ws = crate::reuse::Workspace::open(wt, target_dir_hint(&s).as_deref(), &env)?;
    let groups: Vec<String> = ws
        .pkg_names()
        .into_iter()
        .map(|n| format!("-p {n}"))
        .collect();
    if groups.is_empty() || !crate::knownred::known_for(&d.store, &s.name).is_empty() {
        return None;
    }
    let all = crate::scope::Narrow::Targets(groups.clone());
    if let Some(db) = s.dev_build.as_deref().or(s.build.as_deref()) {
        let b = crate::scope::expand(db, &all, true);
        if !matches!(run_build(who, &s, &b, 0, wt), Built::Ok) {
            return None;
        }
    }
    let tag = format!(
        "{}\n{dc}\n{}\n{}",
        s.name,
        s.dev_build.as_deref().unwrap_or(""),
        s.build.as_deref().unwrap_or("")
    );
    let plan = crate::reuse::plan(&ws, &p.id, &s.name, &groups, &tag, &|label, key| {
        if full {
            return None;
        }
        d.store
            .target_green(&p.id, &s.name, label, key)
            .ok()
            .flatten()
    });
    let mut run = s.clone();
    if !plan.reused.is_empty() {
        run.command = crate::scope::expand(
            dc,
            &crate::scope::Narrow::Targets(plan.groups.clone()),
            false,
        );
        run.min_tests = 1;
        run.narrowed = true;
    }
    Some((run, plan))
}

/// Run the named suites in `wt` (a batch's integration worktree, at the tree
/// to be landed) once, as the landing engine does over a batch's union diff.
/// They belong to no task, so a skip is read from `members`: the run stops when every member
/// has skipped the suite (or the Test step), and is green with a note naming the debt; a
/// forced member stops it too. Evidence is by tree, so a
/// tree that was green is not run twice, whoever asks. `base` is where the
/// batch started (baselines are taken there). A suite whose only reds are excused (also red
/// at `base`, or known red) is green for the batch, with a note naming them; `base_excuses`
/// false (post-land, where `base` is the tree itself) lets only known-red tests excuse.
/// `cover`: what the batch's lease holds of this machine. A suite whose slots it holds, and that
/// falls back from GitHub, runs under that hold (s911); one it does not hold takes its slots as
/// the batch when it runs here (s943).
#[allow(clippy::too_many_arguments)]
pub fn run_over(
    d: &Arc<Daemon>,
    p: &crate::config::ProjectCfg,
    def: &Definition,
    suites: &[String],
    wt: &Path,
    base: &str,
    log_dir: &Path,
    id_prefix: &str,
    sink: Option<StepRunId>,
    base_excuses: bool,
    members: &Members,
    reuse: bool,
    cover: &BatchCover,
) -> OverRun {
    let lock = d.repo_lock(d.project_id_by_name(&p.id).unwrap_or(ProjectId(0)));
    let tree_hash = git::run(wt, &["rev-parse", "HEAD^{tree}"])
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let tree = Tree {
        tree: tree_hash,
        base: base.to_string(),
    };
    let who = Who {
        d: d.clone(),
        task: None,
        step: String::new(),
        project: p.id.clone(),
        log_dir: log_dir.to_path_buf(),
        id_prefix: id_prefix.to_string(),
        sink,
        env: vec![],
        trust_base: base_excuses,
        members: members.clone(),
        reuse: None,
        cover: Cover::default(),
    };
    let _ = std::fs::create_dir_all(log_dir);
    let mut out = OverRun {
        green: true,
        failed: Default::default(),
        notes: vec![],
        failing: Default::default(),
        unfinished: Default::default(),
    };
    for name in suites {
        let Some(suite) = def.testing.suite(name) else {
            out.green = false;
            out.failed.insert(name.clone());
            out.notes.push(format!(
                "{name}: the landing tier names a suite the definition does not have"
            ));
            continue;
        };
        // Targets green on the same inputs are left out, except on the periodic full run that
        // checks the keys (s707).
        let mut who = who.clone();
        who.cover = cover.of(def, name);
        let mut left_out: Option<Suite> = None;
        let full = reuse && p.reuse_green && full_run_due(&p.id, p.reuse_full_hours);
        if reuse && p.reuse_green {
            if let Some((s, plan)) = batch_reuse(&who, p, def, suite, wt, full) {
                if let Some(note) = plan.note() {
                    out.notes.push(note);
                }
                if plan.all_reused() {
                    continue;
                }
                if !plan.reused.is_empty() {
                    left_out = Some(s);
                }
                who.reuse = Some(Arc::new(plan));
            }
        }
        let suite = left_out.as_ref().unwrap_or(suite);
        let res = run_suite(&who, &p.path, &lock, def, suite, wt, &tree, &mut |_| {});
        if full && !matches!(res, SuiteRun::Gone) {
            note_full_run(&p.id);
        }
        // A red on the full run where reuse had said green points at the key computation: every
        // recorded key of the suite is dropped and the note says so.
        if full
            && matches!(
                res,
                SuiteRun::Product { .. }
                    | SuiteRun::Done {
                        outcome: SuiteOutcome::Failed,
                        ..
                    }
            )
        {
            if let Ok(n) = d.store.clear_target_runs(&p.id, name) {
                if n > 0 {
                    out.notes.push(format!(
                        "{name}: red on the full run with reuse off: if a reuse recorded one of its targets green, the input key missed an input (file a bug against crates/daemon/src/reuse.rs); {n} recorded keys dropped"
                    ));
                }
            }
        }
        match res {
            SuiteRun::Done {
                outcome:
                    SuiteOutcome::Proven
                    | SuiteOutcome::Reused
                    | SuiteOutcome::Flaky
                    | SuiteOutcome::Excused
                    | SuiteOutcome::NotSelected,
                notes,
                ..
            } => out.notes.push(notes.join("; ")),
            SuiteRun::Done {
                outcome: SuiteOutcome::Skipped,
                notes,
                ..
            } => out.notes.push(match members.stop_running(d, name) {
                Some(why) => {
                    if members.land_now(d).is_some() {
                        members.note_stopped(&format!("suite:{name}"));
                    }
                    format!("{name}: {why}")
                }
                None => notes.join("; "),
            }),
            SuiteRun::Done { notes, .. } => {
                out.green = false;
                out.failed.insert(name.clone());
                out.notes.push(notes.join("; "));
            }
            SuiteRun::Product {
                feedback, failing, ..
            } => {
                out.green = false;
                out.failed.insert(name.clone());
                if failing.is_empty() {
                    if let Some(names) = timeout_names(&feedback) {
                        out.unfinished.insert(name.clone(), names);
                    }
                }
                if !failing.is_empty() {
                    out.failing.insert(name.clone(), failing);
                }
                out.notes.push(format!("{name}: {}", tail(&feedback, 12)));
            }
            SuiteRun::Gone => {
                out.green = false;
                out.failed.insert(name.clone());
                out.notes.push(format!("{name}: stopped"));
            }
        }
    }
    out
}

#[cfg(test)]
mod lossy_tests {
    use super::*;
    use mason_testing::ParserKind;

    fn ran_from(bytes: &[u8], code: i32) -> (Ran, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("log");
        std::fs::write(&log, bytes).unwrap();
        let ran = Ran {
            code,
            signal: 0,
            output: crate::jobs::read_lossy(&log).unwrap_or_default(),
            stop: Stop::No,
            timed_out: false,
            stalled: None,
            ran_ms: None,
            narrowed: false,
            resumed: None,
            ran_on: None,
        };
        (ran, dir)
    }

    #[test]
    fn a_stray_byte_does_not_hide_the_test_lines() {
        let suite = Suite::new("t", "x", ParserKind::Tap);
        let (ran, dir) = ran_from(b"junk \xff here\nok 1 - a\nok 2 - b\n", 0);
        assert!(ran.output.contains("ok 1 - a"));
        let j = judge_ran(&suite, &ran, dir.path());
        assert!(matches!(j.verdict, Verdict::Green), "{:?}", j.verdict);
    }

    #[test]
    fn a_stray_byte_does_not_turn_a_failure_green() {
        let suite = Suite::new("t", "x", ParserKind::Tap);
        let (ran, dir) = ran_from(b"\xff\nok 1 - a\nnot ok 2 - b\n", 1);
        let j = judge_ran(&suite, &ran, dir.path());
        assert!(matches!(j.verdict, Verdict::Red { .. }));
        // Test lines hidden behind the byte do not make a pass either.
        let (ran, dir) = ran_from(b"\xff\nnothing here\n", 0);
        let j = judge_ran(&suite, &ran, dir.path());
        assert!(!matches!(j.verdict, Verdict::Green));
    }

    #[test]
    fn a_stalled_run_is_red_with_its_reason_even_if_the_output_looks_green() {
        let suite = Suite::new("t", "x", ParserKind::Tap);
        let (mut ran, dir) = ran_from(b"ok 1 - a\nok 2 - b\n", 0);
        ran.stalled = Some("stalled: no output or CPU for 30 min".into());
        let j = judge_ran(&suite, &ran, dir.path());
        match j.verdict {
            Verdict::Red { why, .. } => assert_eq!(why, "stalled: no output or CPU for 30 min"),
            v => panic!("{v:?}"),
        }
    }
}

#[cfg(test)]
mod remote_tests {
    use super::*;
    use mason_testing::ParserKind;

    /// What a local step host gives for a command that printed `output` and exited `code`.
    fn local(output: &str, code: i32) -> Ran {
        Ran {
            code,
            signal: 0,
            output: output.to_string(),
            stop: Stop::No,
            timed_out: false,
            stalled: None,
            ran_ms: Some(12_000),
            narrowed: false,
            resumed: None,
            ran_on: None,
        }
    }

    fn finished(output: &str, code: i32) -> crate::remote::Finished {
        crate::remote::Finished {
            code,
            signal: 0,
            output: output.to_string(),
            timed_out: false,
            stop: None,
            ran_ms: Some(12_000),
            phase: None,
            ran_on: "ran on GitHub (ubuntu-latest), run u, picked up after 6 s".into(),
            parts: vec![],
            missing: vec![],
        }
    }

    fn remote(output: &str, code: i32) -> Ran {
        remote_ran(finished(output, code), None)
    }

    fn part(shard: u32, listed: u32, code: i32, output: &str) -> crate::shards::Part {
        crate::shards::Part {
            shard,
            listed,
            code,
            signal: 0,
            output: output.to_string(),
            run_s: 1,
            build_s: 1,
        }
    }

    /// A suite with cargo's lines and a minimum, as `cargo-workspace` has.
    fn cargo_suite(min_tests: u32) -> Suite {
        let mut s = Suite::new("cargo", "x", ParserKind::Regex);
        s.pass_regex = Some(r"^test (.+) \.\.\. ok$".into());
        s.fail_regex = Some(r"^test (.+) \.\.\. FAILED$".into());
        s.min_tests = min_tests;
        s
    }

    fn sharded(parts: Vec<crate::shards::Part>, suite: &Suite) -> Ran {
        let mut f = finished("", 0);
        f.parts = parts;
        remote_ran(f, Some(suite))
    }

    #[test]
    fn three_shards_are_judged_as_the_one_combined_output() {
        let suite = cargo_suite(4);
        let dir = tempfile::tempdir().unwrap();
        let outs = [
            "test a ... ok\ntest b ... ok\n",
            "test c ... ok\n",
            "test d ... ok\n",
        ];
        let merged = sharded(
            outs.iter()
                .enumerate()
                .map(|(i, o)| part(i as u32 + 1, if i == 0 { 2 } else { 1 }, 0, o))
                .collect(),
            &suite,
        );
        let whole = remote(&outs.concat(), 0);
        assert!(same(&merged, &whole));
        let (jm, jw) = (
            judge_ran(&suite, &merged, dir.path()),
            judge_ran(&suite, &whole, dir.path()),
        );
        assert_eq!(jm.verdict, jw.verdict);
        assert_eq!(jm.verdict, Verdict::Green);
        assert_eq!(jm.results.executed(), 4);
        // `min_tests` applies to the merge: three tests of four are too few.
        let few = judge_ran(&cargo_suite(5), &merged, dir.path());
        assert!(matches!(
            few.verdict,
            Verdict::Vacuous { .. } | Verdict::Red { .. }
        ));
    }

    #[test]
    fn a_red_shard_names_its_failing_tests_and_an_empty_shard_is_red_whatever_the_count() {
        let suite = cargo_suite(2);
        let dir = tempfile::tempdir().unwrap();
        let red = sharded(
            vec![
                part(1, 1, 0, "test a ... ok\n"),
                part(2, 2, 101, "test b ... FAILED\ntest c ... ok\n"),
            ],
            &suite,
        );
        let j = judge_ran(&suite, &red, dir.path());
        assert!(matches!(j.verdict, Verdict::Red { .. }));
        assert_eq!(j.results.failed_names(), vec!["b".to_string()]);
        // Shard 3 ran none of its 140 tests, yet the other two pass `min_tests`.
        let empty = sharded(
            vec![
                part(1, 1, 0, "test a ... ok\ntest b ... ok\n"),
                part(2, 1, 0, "test c ... ok\n"),
                part(3, 140, 0, "error: no runner\n"),
            ],
            &suite,
        );
        let j = judge_ran(&suite, &empty, dir.path());
        assert!(matches!(j.verdict, Verdict::Red { .. }), "{:?}", j.verdict);
        assert!(j.output.contains("shard 3 ran 0 of 140 tests"));
    }

    fn same(a: &Ran, b: &Ran) -> bool {
        (
            a.code,
            a.signal,
            &a.output,
            a.stop,
            a.timed_out,
            &a.stalled,
            a.ran_ms,
            a.narrowed,
        ) == (
            b.code,
            b.signal,
            &b.output,
            b.stop,
            b.timed_out,
            &b.stalled,
            b.ran_ms,
            b.narrowed,
        )
    }

    #[test]
    fn a_remote_run_is_the_same_ran_and_is_judged_the_same_as_a_local_one() {
        let suite = Suite::new("t", "x", ParserKind::Tap);
        let dir = tempfile::tempdir().unwrap();
        let cases = [
            ("ok 1 - a\nok 2 - b\n", 0),
            ("ok 1 - a\nnot ok 2 - b\n", 1),
            ("nothing ran here\n", 0),
        ];
        let mut kinds = vec![];
        for (out, code) in cases {
            let (l, r) = (local(out, code), remote(out, code));
            assert!(same(&l, &r));
            assert!(r.ran_on.as_deref().is_some_and(|w| w.contains("GitHub")));
            let (jl, jr) = (
                judge_ran(&suite, &l, dir.path()),
                judge_ran(&suite, &r, dir.path()),
            );
            assert_eq!(jl.verdict, jr.verdict);
            assert_eq!(jl.results.failed_names(), jr.results.failed_names());
            assert_eq!(jl.results.executed(), jr.results.executed());
            kinds.push(match jr.verdict {
                Verdict::Green => "green",
                Verdict::Red { .. } => "red",
                Verdict::Vacuous { .. } => "vacuous",
            });
            if code == 1 {
                assert_eq!(jr.results.failed_names().len(), 1);
            }
        }
        assert_eq!(kinds, ["green", "red", "vacuous"]);
    }
}

#[cfg(test)]
mod results_file_tests {
    use super::*;

    #[test]
    fn a_wildcard_results_file_reads_every_matching_file_and_a_plain_one_reads_itself() {
        // On 2026-11-14 star's test-desktop (`star-desktop/build/test-results/test/*.xml`)
        // had never been green: the pattern was opened as a file name.
        let dir = tempfile::tempdir().unwrap();
        let res = dir.path().join("build/test-results/test");
        std::fs::create_dir_all(&res).unwrap();
        std::fs::write(res.join("TEST-b.xml"), "<testcase name=\"b\"/>").unwrap();
        std::fs::write(res.join("TEST-a.xml"), "<testcase name=\"a\"/>").unwrap();
        std::fs::write(res.join("output.bin"), "not results").unwrap();
        let pattern = "build/test-results/test/*.xml";
        let paths = results_paths(dir.path(), pattern);
        assert_eq!(paths, vec![res.join("TEST-a.xml"), res.join("TEST-b.xml")]);
        let text = read_results(dir.path(), pattern).unwrap();
        assert!(
            text.contains("name=\"a\"") && text.contains("name=\"b\""),
            "{text}"
        );
        assert!(!text.contains("not results"));
        // A plain path is the file itself, read as before.
        assert_eq!(
            read_results(dir.path(), "build/test-results/test/TEST-a.xml").unwrap(),
            "<testcase name=\"a\"/>\n"
        );
        // Removing before a run takes every match and leaves the rest.
        remove_results(dir.path(), pattern);
        assert!(results_paths(dir.path(), pattern).is_empty());
        assert!(res.join("output.bin").exists());
        // No match reads as "not written", as a missing plain file does.
        assert!(read_results(dir.path(), pattern).is_err());
        assert!(read_results(dir.path(), "missing.xml").is_err());
    }
}

#[cfg(test)]
mod reuse_tests {
    use super::*;
    use mason_testing::{ImpactMap, ParserKind};

    fn def_with_impact() -> Definition {
        let mut def = Definition::empty();
        def.testing.impact = ImpactMap {
            entries: vec![("crates/**".into(), vec!["cargo".into()])],
            overrides: vec![],
        };
        def
    }

    #[test]
    fn paths_the_impact_map_sends_elsewhere_are_left_out_of_a_suite() {
        let mut def = Definition::empty();
        def.testing.impact = ImpactMap {
            entries: vec![
                ("crates/**".into(), vec!["cargo".into()]),
                ("docs/**".into(), vec!["cargo".into()]),
                (
                    "scripts/exit/**".into(),
                    vec!["cargo".into(), "exit".into()],
                ),
                ("clients/swift/**".into(), vec!["swift".into()]),
            ],
            overrides: vec![],
        };
        let suite = Suite::new("cargo", "cargo test", ParserKind::Tap);
        let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let (kept, dropped) = split_for_suite(
            &def,
            &suite,
            &v(&[
                "clients/swift/A.swift",
                "docs/stages/s1.md",
                "scripts/exit/s1.sh",
                "Makefile",
                ".mason/smoke-tests.txt",
                "crates/store/src/lib.rs",
            ]),
        );
        assert_eq!(
            kept,
            v(&[
                "docs/stages/s1.md",
                "scripts/exit/s1.sh",
                "Makefile",
                ".mason/smoke-tests.txt",
                "crates/store/src/lib.rs"
            ])
        );
        assert_eq!(dropped, v(&["clients/swift/A.swift"]));
        assert_eq!(
            left_out_note(&dropped),
            "clients/swift/** left out: the impact map sends it only to other suites"
        );
        assert_eq!(left_out_note(&[]), "");
    }

    fn git_in(dir: &Path, args: &[&str]) -> String {
        let out = mason_landing::git::command()
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(out.status.success(), "{args:?}: {out:?}");
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    fn commit(dir: &Path, file: &str) -> String {
        let p = dir.join(file);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, file).unwrap();
        git_in(dir, &["add", "-A"]);
        git_in(dir, &["commit", "-qm", file]);
        git_in(dir, &["rev-parse", "HEAD"])
    }

    fn reuse(
        def: &Definition,
        suite: &Suite,
        dir: &Path,
        store: &mason_store::Store,
        base: &str,
    ) -> Option<String> {
        let hash = suite_def_hash(def, suite);
        reusable_baseline(store, dir, def, suite, &hash, base).map(|r| r.0)
    }

    #[test]
    fn a_readme_commit_reuses_the_baseline_and_a_crate_change_or_definition_change_does_not() {
        let dir = tempfile::tempdir().unwrap();
        git_in(dir.path(), &["init", "-q"]);
        let a = commit(dir.path(), "crates/x/src/lib.rs");
        let db = tempfile::tempdir().unwrap();
        let store = mason_store::Store::open(db.path().join("s.db")).unwrap();
        let def = def_with_impact();
        let suite = Suite::new("cargo", "cargo test", ParserKind::Tap);
        let hash = suite_def_hash(&def, &suite);
        store
            .put_baseline_hashed(
                &a,
                "cargo",
                &["t1".into()],
                &["t1".into(), "t2".into()],
                2,
                1,
                &hash,
            )
            .unwrap();

        // Same base: handled by the exact lookup, not here.
        assert_eq!(reuse(&def, &suite, dir.path(), &store, &a), None);
        // A README-only commit reuses it.
        let b = commit(dir.path(), "README.md");
        // README.md is unmapped, so the impact map says it touches everything...
        assert_eq!(reuse(&def, &suite, dir.path(), &store, &b), None);
        // ...unless the map names it as touching no suite.
        let mut def2 = def.clone();
        def2.testing.impact.entries.push(("*.md".into(), vec![]));
        assert_eq!(
            reuse(&def2, &suite, dir.path(), &store, &b),
            Some(a.clone())
        );
        // A change under crates/ does not.
        let c = commit(dir.path(), "crates/x/src/more.rs");
        assert_eq!(reuse(&def2, &suite, dir.path(), &store, &c), None);
        // A change to the suite's definition does not.
        let mut changed = suite.clone();
        changed.command = "cargo test --all".into();
        assert_eq!(reuse(&def2, &changed, dir.path(), &store, &b), None);
        // A change under .mason/ does not.
        let d = commit(dir.path(), ".mason/testing.toml");
        let mut def3 = def2.clone();
        def3.testing
            .impact
            .entries
            .push((".mason/**".into(), vec![]));
        assert_eq!(reuse(&def3, &suite, dir.path(), &store, &d), None);
    }

    fn tree_of(dir: &Path) -> String {
        git_in(dir, &["rev-parse", "HEAD^{tree}"])
    }

    fn two_suite_def() -> (Definition, Suite, Suite) {
        let mut def = Definition::empty();
        def.testing.impact = ImpactMap {
            entries: vec![
                ("a/**".into(), vec!["A".into()]),
                ("b/**".into(), vec!["B".into()]),
            ],
            overrides: Vec::new(),
        };
        (
            def,
            Suite::new("A", "run a", ParserKind::Tap),
            Suite::new("B", "run b", ParserKind::Tap),
        )
    }

    #[test]
    fn a_later_round_reuses_a_green_only_for_suites_the_change_does_not_touch() {
        let dir = tempfile::tempdir().unwrap();
        git_in(dir.path(), &["init", "-q"]);
        commit(dir.path(), "a/x");
        commit(dir.path(), "b/x");
        let t1 = tree_of(dir.path());
        let db = tempfile::tempdir().unwrap();
        let store = mason_store::Store::open(db.path().join("s.db")).unwrap();
        let (def, a, b) = two_suite_def();
        for s in [&a, &b] {
            let h = suite_def_hash(&def, s);
            store
                .put_task_green(7, &s.name, &h, &t1, 3, 5, 5, 1)
                .unwrap();
        }
        // Round 2 changes only a/.
        commit(dir.path(), "a/y");
        let t2 = tree_of(dir.path());
        let got = |s: &Suite, d: &Definition, task: u64, tree: &str| {
            reusable_task_green(&store, dir.path(), d, s, task, tree).map(|r| (r.2, r.3))
        };
        assert_eq!(got(&b, &def, 7, &t2), Some((3, t1.clone())));
        assert_eq!(got(&a, &def, 7, &t2), None);
        // Another task's green is not this task's.
        assert_eq!(got(&b, &def, 8, &t2), None);
        // The same tree is the exact-evidence path, not this one.
        assert_eq!(got(&b, &def, 7, &t1), None);
        // A changed definition forces a rerun.
        let mut changed = b.clone();
        changed.command = "run b --more".into();
        assert_eq!(got(&changed, &def, 7, &t2), None);
        // A narrowed pick hashes differently, so an earlier whole run is not reused for it.
        let mut narrowed = b.clone();
        narrowed.narrowed = true;
        assert_eq!(got(&narrowed, &def, 7, &t2), None);
        // An unmapped path forces a rerun.
        commit(dir.path(), "other/z");
        let t3 = tree_of(dir.path());
        assert_eq!(got(&b, &def, 7, &t3), None);
    }

    #[test]
    fn an_unhashed_baseline_is_never_reused() {
        let dir = tempfile::tempdir().unwrap();
        let store = mason_store::Store::open(dir.path().join("s.db")).unwrap();
        store.put_baseline("abc", "cargo", &[], &[], 0, 1).unwrap();
        assert!(store
            .ancestor_baselines("cargo", "", 10)
            .unwrap()
            .is_empty());
    }
}

#[cfg(test)]
mod land_now_debt_tests {
    use super::*;
    use mason_testing::ParserKind;

    #[test]
    fn a_forced_or_land_now_debt_owes_the_suites_of_the_tier_it_skipped() {
        let mut def = Definition::empty();
        for n in ["unit", "integration", "smoke"] {
            def.testing.suites.push(Suite::new(n, "x", ParserKind::Tap));
        }
        let tier = vec!["integration".to_string(), "smoke".to_string()];
        for scope in ["landing:force", "landing:land_now"] {
            let names: Vec<String> = debt_suites(&def, &tier, scope)
                .into_iter()
                .map(|s| s.name)
                .collect();
            assert_eq!(names, ["integration", "smoke"], "{scope}");
        }
        // A tier that names a suite the definition lost owes the rest, not an error.
        let gone = vec!["integration".to_string(), "removed".to_string()];
        assert_eq!(debt_suites(&def, &gone, "landing:force").len(), 1);
        // Other scopes are as before.
        assert_eq!(debt_suites(&def, &tier, "suite:unit").len(), 1);
        assert!(debt_suites(&def, &tier, "landing:other").is_empty());
    }

    #[test]
    fn a_ci_scope_names_the_workflow_or_ci() {
        // `ci:` debts are paid by running CI on the base, not by suites.
        let def = Definition::empty();
        assert!(debt_suites(&def, &[], "ci:build").is_empty());
    }
}

#[cfg(test)]
mod alone_tests {
    use super::*;
    use mason_testing::ParserKind;
    use std::collections::{BTreeMap, BTreeSet};

    fn def(rerun: Option<&str>) -> Definition {
        let mut def = Definition::empty();
        let mut s = Suite::new("cw", "cargo test", ParserKind::Tap);
        s.rerun = rerun.map(String::from);
        def.testing.suites.push(s);
        def
    }

    fn red(names: &[&str]) -> (BTreeSet<String>, BTreeMap<String, Vec<String>>) {
        let failed: BTreeSet<String> = ["cw".to_string()].into();
        let mut m = BTreeMap::new();
        if !names.is_empty() {
            m.insert(
                "cw".to_string(),
                names.iter().map(|n| n.to_string()).collect(),
            );
        }
        (failed, m)
    }

    #[test]
    fn a_red_runs_alone_only_when_every_red_suite_has_names_and_a_rerun_template() {
        let (f, n) = red(&["a::b"]);
        assert!(can_run_alone(&def(Some("t -- {tests}")), &f, &n));
        // No readable names: the whole suite, as before.
        let (f, none) = red(&[]);
        assert!(!can_run_alone(&def(Some("t -- {tests}")), &f, &none));
        // No rerun template: the whole suite.
        let (f, n) = red(&["a::b"]);
        assert!(!can_run_alone(&def(None), &f, &n));
        // A red the names do not cover (a lint, a floor) is not searched on tests alone.
        let mut f2 = f.clone();
        f2.insert("lint".into());
        assert!(!can_run_alone(&def(Some("t -- {tests}")), &f2, &n));
    }

    #[test]
    fn the_rerun_command_names_the_tests_and_runs_them_one_at_a_time_when_asked() {
        let d = def(Some("cargo test -- --exact {tests}"));
        let s = d.testing.suite("cw").unwrap();
        let t = vec!["a::b".to_string(), "c::d".to_string()];
        assert_eq!(
            rerun_command(s, &t, false),
            "cargo test -- --exact 'a::b' 'c::d'"
        );
        assert_eq!(
            rerun_command(s, &t, true),
            "cargo test -- --exact 'a::b' 'c::d' --test-threads=1"
        );
        // Without a `--` there is nowhere to put a libtest flag.
        let d = def(Some("run {tests}"));
        assert_eq!(
            rerun_command(d.testing.suite("cw").unwrap(), &t, true),
            "run 'a::b' 'c::d'"
        );
    }
}

#[cfg(test)]
mod timeout_tests {
    use super::*;

    #[test]
    fn the_tests_still_running_at_the_kill_are_the_slow_ones_with_no_result() {
        let out = "test a ... ok\ntest b has been running for over 60 seconds\ntest c has been running for over 60 seconds\ntest b has been running for over 120 seconds\ntest c ... ok <61.2s>\n";
        assert_eq!(still_running(out), vec!["b".to_string()]);
        assert!(still_running("1..1\nok 1 - x\n").is_empty());
    }

    #[test]
    fn a_timeout_feedback_gives_its_names_and_another_red_gives_none() {
        let f = format!(
            "`s` is red ({RAN_OUT_OF_TIME}).\n{STILL_RUNNING}a::b, c.\nLast output of `s`:\n"
        );
        assert_eq!(
            timeout_names(&f),
            Some(vec!["a::b".to_string(), "c".to_string()])
        );
        let none = format!("`s` is red ({RAN_OUT_OF_TIME}).\nLast output of `s`:\n");
        assert_eq!(timeout_names(&none), Some(vec![]));
        assert_eq!(timeout_names("`s` is red (3 tests failed).\n"), None);
    }
}

#[cfg(test)]
mod narrow_batch_tests {
    use super::*;
    use mason_testing::ParserKind;

    fn git_in(d: &Path, args: &[&str]) {
        let ok = std::process::Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
            .args(args)
            .current_dir(d)
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    }

    fn put(d: &Path, p: &str, s: &str) {
        let f = d.join(p);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, s).unwrap();
    }

    /// Two crates, `a` and `b`, one commit: the base.
    fn repo() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        put(
            r,
            "Cargo.toml",
            "[workspace]\nresolver = \"2\"\nmembers = [\"crates/*\"]\n",
        );
        for n in ["a", "b"] {
            put(
                r,
                &format!("crates/{n}/Cargo.toml"),
                &format!("[package]\nname = \"{n}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
            );
            put(r, &format!("crates/{n}/src/lib.rs"), "");
        }
        git_in(r, &["init", "-q"]);
        git_in(r, &["add", "-A"]);
        git_in(r, &["commit", "-q", "-m", "base"]);
        d
    }

    fn base(d: &Path) -> String {
        git::run(d, &["rev-parse", "HEAD"])
            .unwrap()
            .trim()
            .to_string()
    }

    fn commit(d: &Path, file: &str) {
        put(d, file, "// changed\n");
        git_in(d, &["add", "-A"]);
        git_in(d, &["commit", "-q", "-m", "change"]);
    }

    fn suite() -> Suite {
        let mut s = Suite::new("cw", "cargo test --workspace", ParserKind::Tap);
        s.dev_command = Some("cargo test {tests_for_change}".into());
        s
    }

    #[test]
    fn a_batch_touching_one_crate_runs_the_narrowed_command_over_the_union() {
        let d = repo();
        let b = base(d.path());
        commit(d.path(), "crates/a/src/lib.rs");
        let (s, note) = narrow_for_batch(&Definition::empty(), &suite(), d.path(), &b).unwrap();
        assert!(s.narrowed);
        assert!(s.command.starts_with("cargo test "), "{}", s.command);
        assert!(s.command.contains("-p a"), "{}", s.command);
        assert!(!s.command.contains("-p b"), "{}", s.command);
        assert!(note.contains("scheduled full test"), "{note}");
        // A second commit in the other crate joins the union.
        commit(d.path(), "crates/b/src/lib.rs");
        let (s, _) = narrow_for_batch(&Definition::empty(), &suite(), d.path(), &b).unwrap();
        assert!(
            s.command.contains("-p a") && s.command.contains("-p b"),
            "{}",
            s.command
        );
    }

    #[test]
    fn a_batch_touching_cargo_lock_runs_whole() {
        let d = repo();
        let b = base(d.path());
        commit(d.path(), "Cargo.lock");
        let why = narrow_for_batch(&Definition::empty(), &suite(), d.path(), &b).unwrap_err();
        assert!(why.contains("Cargo.lock"), "{why}");
    }

    #[test]
    fn an_empty_union_and_a_suite_without_a_dev_command_run_whole() {
        let d = repo();
        let b = base(d.path());
        assert!(narrow_for_batch(&Definition::empty(), &suite(), d.path(), &b).is_err());
        commit(d.path(), "crates/a/src/lib.rs");
        let mut s = suite();
        s.dev_command = None;
        assert!(narrow_for_batch(&Definition::empty(), &s, d.path(), &b).is_err());
    }

    #[test]
    fn narrowed_suite_drops_the_whole_suites_total() {
        let mut s = SubProgress {
            total: Figure::expected(2356),
            ..SubProgress::default()
        };
        forget_whole_total(&mut s);
        assert_eq!(s.total, Figure::default());
    }

    #[test]
    fn narrowed_suite_keeps_a_reported_total() {
        let mut s = SubProgress {
            total: Figure::reported(337),
            ..SubProgress::default()
        };
        forget_whole_total(&mut s);
        assert_eq!(s.total, Figure::reported(337));
    }
}

#[cfg(test)]
mod batch_cover_tests {
    use super::*;

    fn needs(items: &[(&str, u32)]) -> Needs {
        Needs::of(items)
    }

    /// s943: a run is covered only when the batch's lease really holds its slots.
    #[test]
    fn a_batch_cover_holds_only_what_the_lease_holds() {
        let none = BatchCover::default();
        assert!(
            !none.holds(&needs(&[("test-suite", 1)])),
            "no lease, no cover"
        );
        assert!(!none.holds(&Needs::default()), "no lease: not even nothing");

        let lease = BatchCover {
            owner: None,
            held: Some(needs(&[("test-suite", 2), ("compile", 1)])),
        };
        assert!(lease.holds(&needs(&[("test-suite", 1)])));
        assert!(lease.holds(&needs(&[("test-suite", 2), ("compile", 1)])));
        assert!(!lease.holds(&needs(&[("test-suite", 3)])), "more than held");
        assert!(!lease.holds(&needs(&[("test-small", 1)])), "another slot");
        assert!(!lease.holds(&Needs {
            exclusive: true,
            ..Default::default()
        }));
    }
}

#[cfg(test)]
mod merged_branch_tests {
    use super::merged_branch_commit;
    use std::path::Path;

    fn git(dir: &Path, args: &[&str]) -> String {
        let o = std::process::Command::new("git")
            .current_dir(dir)
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(o.status.success(), "git {args:?}: {o:?}");
        String::from_utf8(o.stdout).unwrap().trim().to_string()
    }

    #[test]
    fn a_deleted_branch_is_found_by_its_merge_and_t96_does_not_match_t966() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        git(d, &["init", "-q", "-b", "main"]);
        git(d, &["commit", "-q", "--allow-empty", "-m", "root"]);
        git(d, &["checkout", "-q", "-b", "mason/mason/t966"]);
        git(d, &["commit", "-q", "--allow-empty", "-m", "work"]);
        let work = git(d, &["rev-parse", "HEAD"]);
        git(d, &["checkout", "-q", "main"]);
        git(d, &["commit", "-q", "--allow-empty", "-m", "other"]);
        git(
            d,
            &[
                "merge",
                "-q",
                "--no-ff",
                "mason/mason/t966",
                "-m",
                "Merge branch 'mason/mason/t966'",
            ],
        );
        git(d, &["branch", "-q", "-D", "mason/mason/t966"]);
        assert_eq!(
            merged_branch_commit(d, "mason/mason/t966", "main"),
            Some(work)
        );
        assert_eq!(merged_branch_commit(d, "mason/mason/t96", "main"), None);
    }
}
