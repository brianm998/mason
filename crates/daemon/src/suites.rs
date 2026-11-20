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
    pub why: &'static str,
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
    let selection = match paths {
        Some(paths) => t.impact.select(&paths, &step.suites),
        None => Selection::All,
    };
    let bound: Vec<String> = t.bound(when).iter().map(|s| s.name.clone()).collect();
    let why_not = match when {
        When::Development => "its level is not bound to development",
        _ => "its level is not bound to the test step",
    };
    step.suites
        .iter()
        .filter_map(|n| t.suite(n))
        .map(|s| {
            let (selected, why) = match &selection {
                _ if !bound.contains(&s.name) => (false, why_not),
                Selection::All => (true, "an unmapped path runs everything"),
                Selection::Some(v) if v.contains(&s.name) => (true, "the impact map selects it"),
                Selection::Some(_) => (false, "the impact map does not select it"),
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
    // A development command lists packages itself (`-p a -p b`); `--workspace` would override
    // that list, so the project's test command goes in without it (its prefix, such as the
    // owner's CARGO_TARGET_DIR, stays).
    let narrow = t.replace(" --workspace", "");
    s.dev_command = s.dev_command.map(|b| b.replace(KEY, &narrow));
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
) -> Result<Ran, String> {
    run_hosted_resuming(
        d, id, cmd, cwd, limit, log_dir, check, run, watch, th, env, None,
    )
}

/// [`run_hosted`]; with a `resume` template (`resume::template`), a lost host's finished cargo
/// test executables are kept and only the others run on the fresh host, the verdict reading
/// both (#621). A second loss in a row still fails the step.
#[allow(clippy::too_many_arguments)]
fn run_hosted_resuming(
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
    resume: Option<&str>,
) -> Result<Ran, String> {
    use crate::hostloss as hl;
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
                    why: "it ran out of time".into(),
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
            notes: vec![format!(
                "{}: green for this exact tree already ({executed} tests, from task #{task})",
                suite.name
            )],
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
        ) {
            Ok(r) => r,
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
                let rerun = if failures.is_empty() {
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
        let result = match (result, &ran.resumed) {
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
    let cmd = match &suite.rerun {
        Some(t) => t.replace(
            "{tests}",
            &failures
                .iter()
                .map(|f| quote(f))
                .collect::<Vec<_>>()
                .join(" "),
        ),
        None => suite.command.clone(),
    };
    let cmd = crate::jobs::pty_wrap(suite.pty, &lifecycle::with_device(&cmd, dev_id(dev)));
    if let Some(f) = &suite.results_file {
        remove_results(cwd, f);
    }
    let id = if attempt == 0 {
        format!("{}-r-{}", who.id_prefix, suite.name)
    } else {
        format!("{}-r{}-{}", who.id_prefix, now_ms(), suite.name)
    };
    let ran = run_hosted(
        &who.d,
        &id,
        &cmd,
        cwd,
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
    let j = judge_ran_with(suite, &ran, cwd, who.plugin_parser(suite).as_deref());
    if j.results.executed() == 0 {
        // It ran nothing, so it proved nothing: not flaky.
        return None;
    }
    Some(j.results.failed_names())
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
fn lease_suite(c: &Ctx, needs: Needs, check: &Check, expected_ms: Option<u64>) -> Leased {
    match check() {
        Stop::Skipped => return Leased::Skipped,
        Stop::Gone => return Leased::Gone,
        Stop::No => {}
    }
    if !needs.exclusive && needs.amounts.is_empty() {
        return Leased::Got;
    }
    let wanted = || check() == Stop::No;
    match crate::jobs::acquire_needs_expecting(c, needs, &wanted, false, expected_ms) {
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
/// The narrowed copy of `suite` for this task's test step (s386), with a note naming what it
/// runs, or `None` to run it whole: no `narrow_*` keys, the change unreadable, or
/// [`mason_testing::narrow::select`] picks nothing or most of it.
fn narrow_for_step(c: &Ctx, suite: &Suite) -> Option<(Suite, String)> {
    if suite.narrow_packages {
        return narrow_packages_for_step(c, suite);
    }
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
    let picked = mason_testing::narrow::select(n, &files, &change)?;
    let total = mason_testing::narrow::units(n, &files).len();
    let mut s = suite.clone();
    s.command = mason_testing::narrow::command(n, &picked);
    s.min_tests = 1;
    s.narrowed = true;
    let names: Vec<&str> = picked.iter().take(8).map(|u| u.name.as_str()).collect();
    let more = picked.len().saturating_sub(names.len());
    let note = format!(
        "{}: narrowed to {} of {} test units ({}{}); the landing batch runs it whole",
        suite.name,
        picked.len(),
        total,
        names.join(", "),
        if more > 0 {
            format!(" and {more} more")
        } else {
            String::new()
        }
    );
    Some((s, note))
}

/// The package-mode narrowing of `suite` for a task's test step (s396): its `dev_command`
/// (and `dev_build`) with `{packages}` for the crates the change touches and their dependents,
/// as `run_check` does. `None` (runs whole) when the suite has no `dev_command`, or no package
/// list can be found. Like a test-unit narrowing, it records no evidence.
fn narrow_packages_for_step(c: &Ctx, suite: &Suite) -> Option<(Suite, String)> {
    let dc = suite.dev_command.as_ref()?;
    let all = worktree_paths(&c.wt, &c.project.base_ref())?;
    let (paths, dropped) = split_for_suite(&c.def, suite, &all);
    // A `{tests_for_change}` command narrows to test targets and adds the smoke set (decision
    // 0539); a `{packages}` one to whole packages.
    let n = crate::scope::narrow_for(&c.wt, &paths, crate::scope::wants_tests(dc), true);
    let Some(n) = n else {
        // A path outside every package (a root file) or no cargo means the suite runs whole.
        eprintln!(
            "suite {}: narrowing found no list for {} changed paths; running whole",
            suite.name,
            paths.len()
        );
        return None;
    };
    let mut s = suite.clone();
    s.command = crate::scope::expand(dc, &n, false);
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
    Some((s, note))
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
        let suite = match narrow_for_step(c, &suite) {
            Some((narrowed, note)) => {
                // Said in the closing note too: a narrowed green is not the whole suite's.
                details.push(note.clone());
                notes.push(note);
                narrowed
            }
            None => suite,
        };
        let ran_beside = side.remove(&name);
        if per_suite && ran_beside.is_none() {
            c.release_lease();
            let reused = matches!(d.store.evidence(&tree.tree, &suite.name), Ok(Some(_)));
            if let Some(b) = suite
                .build
                .as_ref()
                .filter(|b| !reused && !built.contains(b))
            {
                let needs = needs_of(&crate::jobs::build_uses(), None);
                who.env = held_env(c, &needs);
                match lease_suite(c, needs, &who.check(&name), estimate(&who, &name, "build")) {
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
            let needs = run_needs(&c.def, &c.step, &suite);
            who.env = held_env(c, &needs);
            // A skipped suite holds nothing: `run_suite` sees the skip before it runs. Nor does
            // one already green for this exact tree, which `run_suite` reuses without running
            // (s380): it must not wait in the queue to learn that.
            if !proven(d, &tree, &name) {
                if let Leased::Gone =
                    lease_suite(c, needs, &who.check(&name), estimate(&who, &name, "run"))
                {
                    return;
                }
            }
        }
        let ran = match ran_beside {
            Some(r) => r,
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
        let c = git::run(
            &p.path,
            &[
                "rev-parse",
                "--verify",
                &format!("{}^{{commit}}", p.branch(row.task)),
            ],
        )
        .ok()?;
        c.trim().to_string()
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
        let suites = debt_suites(&d.current_def(pid), &landing_tier(d, pid), &row.scope);
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

static IDLE_RUN: AtomicBool = AtomicBool::new(false);

/// One debt payment at a time, machine-wide. Each is a full run of the skipped suites on the
/// base branch's tip; a landing of several test-skipped tasks used to start them all at once,
/// outside every broker limit, and the burst hung the machine.
static PAYING: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Hold what the definition's test step `uses` (`compile` and `test-suite` for MASON) while a
/// debt runs, ranked behind the task's own work, so a payment queues like any test step and
/// never runs beside a full set of them. Nothing to hold gives `Ok(None)`.
fn hold_for_debt(
    d: &Arc<Daemon>,
    def: &Definition,
    task: TaskId,
    debt: u64,
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
    if needs.amounts.keys().any(|n| n.starts_with("device:")) {
        d.targets.declare_all(d);
    }
    for name in needs.amounts.keys() {
        if !name.starts_with("llm:") && !name.starts_with("device:") {
            d.broker
                .declare(name, def.resources.get(name).copied().unwrap_or(1));
        }
    }
    let rank = match d.world().ok().and_then(|w| w.tasks.get(&task).cloned()) {
        Some(t) => d.rank_for(&t, def, &step.name),
        // A task the world no longer holds: behind everything, at low priority.
        None => mason_scheduler::Rank::new(false, 100, 3, now_ms(), task.0, false),
    };
    // Beside: the task may still hold a step's own request (a debt paid by hand).
    d.broker
        .acquire_beside(
            task,
            &format!("debt #{debt}"),
            rank,
            needs,
            &|| true,
            &mut |_| {},
        )
        .map(Some)
        .map_err(|_| "the wait for the test step's slots was cancelled".to_string())
}

/// Run the debts still open, one at a time, as work below every task: only while the compile
/// queue is empty and a slot is free, and each debt once (a debt a run found red keeps
/// `tried_at` and waits for the owner). Only debts whose task landed are run.
pub fn run_idle_debts(d: &Arc<Daemon>) {
    if IDLE_RUN.swap(true, Ordering::SeqCst) {
        return;
    }
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            IDLE_RUN.store(false, Ordering::SeqCst);
        }
    }
    let _reset = Reset;
    // (project, scope, tip) already run in this pass: the same suites never run twice on one
    // tip. The result is settled onto every debt the tip holds before another is chosen (s476).
    let mut ran: Vec<(String, String, String)> = vec![];
    loop {
        settle_debts(d);
        let c = d.broker.compilation();
        // A limit of 0 means no `compile` pool is declared: nothing to wait for.
        if !c.pending.is_empty() || (c.limit > 0 && c.used >= c.limit) {
            return;
        }
        let Ok(rows) = d.store.debts(None, true) else {
            return;
        };
        let next = rows.into_iter().find(|row| {
            row.tried_at == 0
                && d.project_id_by_name(&row.project)
                    .and_then(|pid| d.project(pid).cloned())
                    .and_then(|p| {
                        let tip = git::run(&p.path, &["rev-parse", &p.base_ref()]).ok()?;
                        let tip = tip.trim().to_string();
                        debt_landed(d, &p, row, &tip)?;
                        let key = (row.project.clone(), row.scope.clone(), tip);
                        (!ran.contains(&key)).then_some(())
                    })
                    .is_some()
        });
        let Some(row) = next else { return };
        let key = d
            .project_id_by_name(&row.project)
            .and_then(|pid| d.project(pid).cloned())
            .and_then(|p| git::run(&p.path, &["rev-parse", &p.base_ref()]).ok())
            .map(|t| (row.project.clone(), row.scope.clone(), t.trim().to_string()));
        if let Some(k) = key {
            ran.push(k);
        }
        match pay(d, row.id) {
            Ok(m) => tracing::info!(debt = row.id, "idle run: {m}"),
            Err(e) => {
                let _ = d.store.debt_tried(row.id, now_ms());
                tracing::warn!(debt = row.id, "idle run: {e}");
            }
        }
    }
}

/// Look at the open debts once an hour (`debt_interval`), the first time soon after start.
pub fn spawn_debts(d: &Arc<Daemon>) {
    let every = d.cfg.debt_interval;
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
    let suites = debt_suites(&def, &landing_tier(d, pid), &row.scope);
    if suites.is_empty() {
        return Err("the debt's suite no longer exists in the definition".into());
    }
    let _one = PAYING.lock().unwrap_or_else(|e| e.into_inner());
    // Another payment may have settled this one while it waited.
    if d.store
        .debts(None, false)
        .map_err(|e| e.to_string())?
        .iter()
        .any(|x| x.id == debt && x.status != "open")
    {
        return Ok("already paid".into());
    }
    let _lease = hold_for_debt(d, &def, TaskId(row.task), debt)?;
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
                verdicts.push(notes.join("; "));
            }
            SuiteRun::Product { feedback, .. } => {
                all_green = false;
                verdicts.push(feedback.lines().next().unwrap_or("red").to_string());
            }
            SuiteRun::Gone => all_green = false,
        }
    }
    {
        let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
        git::remove_worktree(&p.path, &wt);
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

/// What running suites over a batch's integration branch said.
pub struct OverRun {
    pub green: bool,
    /// The suites that were red.
    pub failed: std::collections::BTreeSet<String>,
    /// One line per suite, and the feedback of a red.
    pub notes: Vec<String>,
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
    };
    let _ = std::fs::create_dir_all(log_dir);
    let mut out = OverRun {
        green: true,
        failed: Default::default(),
        notes: vec![],
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
        match run_suite(&who, &p.path, &lock, def, suite, wt, &tree, &mut |_| {}) {
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
            SuiteRun::Product { feedback, .. } => {
                out.green = false;
                out.failed.insert(name.clone());
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
