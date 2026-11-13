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
use std::sync::Arc;
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

/// Whether the step queues suite by suite (some selected suite builds first), instead of
/// holding everything for the whole step.
pub fn builds_per_suite(
    project: &crate::config::ProjectCfg,
    def: &Definition,
    step: &Step,
    branch: &str,
) -> bool {
    plan(project, def, step, branch)
        .iter()
        .any(|s| s.selected && s.suite.build.is_some())
}

/// What one suite's run holds in a step that queues suite by suite: the step's `uses`
/// (without `compile` when the suite built first) and the suite's own.
pub(crate) fn run_needs(def: &Definition, step: &Step, suite: &Suite) -> Needs {
    let mut uses = if suite.build.is_some() {
        crate::jobs::tests_uses(&step.uses)
    } else {
        step.uses.clone()
    };
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

/// A suite with the project's `test` command written in: `{project_test}` in its
/// `command`, `build` and `rerun` becomes that command, so the owner's per-worktree
/// build directory (set in the project file's `test`) applies to a suite too.
pub(crate) fn with_project_test(suite: &Suite, test: Option<&str>) -> Result<Suite, String> {
    const KEY: &str = "{project_test}";
    let uses = |s: &str| s.contains(KEY);
    let mut s = suite.clone();
    let any = uses(&s.command)
        || s.build.as_deref().is_some_and(uses)
        || s.rerun.as_deref().is_some_and(uses)
        || s.dev_command.as_deref().is_some_and(uses)
        || s.dev_build.as_deref().is_some_and(uses);
    if !any {
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
    use crate::hostloss as hl;
    let hosts = d.cfg.hosts_dir();
    let again_id = hl::rerun_id(id);
    let mut first = None;
    let mut on = id.to_string();
    // A progress file left by an earlier attempt says nothing about this one.
    if !hosts.join(id).join("host.kv").exists() && !hosts.join(&again_id).join("host.kv").exists() {
        if let Some(f) = watch.and_then(|w| w.progress.as_ref()) {
            let _ = std::fs::remove_file(cwd.join(f));
        }
    }
    if hosts.join(&again_id).join("host.kv").exists() {
        first = mason_step_host::registry::lost_report(&hosts.join(id)).ok();
        on = again_id.clone();
    }
    loop {
        match run_hosted_once(
            d,
            &on,
            cmd,
            cwd,
            limit,
            log_dir,
            check.clone(),
            run,
            watch,
            th,
            env.clone(),
        ) {
            Ok(r) => return Ok(r),
            Err(HostedErr::Other(e)) => return Err(e),
            Err(HostedErr::Lost(rep)) if on == id => {
                hl::record(d, run, id, &rep, true);
                first = Some(rep);
                on = again_id.clone();
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
    Lost(mason_step_host::registry::LostReport),
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
                Ok(rep) => HostedErr::Lost(rep),
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
        timed_out: why.load(Ordering::SeqCst) == 1,
        stalled: stall.as_ref().and_then(|w| w.stalled()),
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

/// What a finished attempt says, once parsed and judged.
struct Judged {
    results: Results,
    verdict: Verdict,
    output: String,
}

fn judge_ran(suite: &Suite, ran: &Ran, cwd: &Path) -> Judged {
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
        Some(f) => match crate::jobs::read_lossy(&cwd.join(f)) {
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
    match parse(suite.parser, &text, suite) {
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
}

impl Who {
    /// The step run that shows this run's progress, if any.
    fn run(&self) -> Option<StepRunId> {
        self.task.map(|(_, r)| r).or(self.sink)
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
        let (d, task, step, suite) = (
            self.d.clone(),
            self.task,
            self.step.clone(),
            suite.to_string(),
        );
        Arc::new(move || {
            let Some((t, run)) = task else {
                return Stop::No;
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

/// An open debt that skips this suite or its whole step.
fn skip_for(d: &Daemon, task: TaskId, suite: &str, step: &str) -> Option<u64> {
    d.store
        .debts(Some(task.0), true)
        .ok()?
        .into_iter()
        .find(|x| x.scope == format!("suite:{suite}") || x.scope == format!("step:{step}"))
        .map(|x| x.id)
}

enum SuiteRun {
    Done {
        outcome: SuiteOutcome,
        results: Option<Results>,
        notes: Vec<String>,
    },
    /// Red, and the task's.
    Product { feedback: String },
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
    let Some((kind, os)) = def
        .testing
        .environment(&suite.env)
        .and_then(|e| crate::targets::kind_of_env(e).map(|k| (k, e.os.clone())))
    else {
        return Ok(None);
    };
    let label = format!("#{} {}", who.task.map_or(0, |t| t.0 .0), suite.name);
    match who.d.targets.lease(kind, os.as_deref(), &label) {
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
    if let Ok(Some(b)) = who.d.store.baseline(base, &suite.name) {
        return Some(b);
    }
    let wt = who.log_dir.join(format!("base-{}", suite.name));
    {
        let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
        git::remove_worktree(c_repo, &wt);
        git::run(
            c_repo,
            &["worktree", "add", "--detach", &wt.to_string_lossy(), base],
        )
        .ok()?;
    }
    let outcome = (|| {
        env_up(who, def, suite, &wt, dev).ok()?;
        if let Some(f) = &suite.results_file {
            let _ = std::fs::remove_file(wt.join(f));
        }
        let id = format!("{}-b-{}", who.id_prefix, suite.name);
        let ran = run_hosted(
            &who.d,
            &id,
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
        if ran.stop != Stop::No {
            return None;
        }
        let j = judge_ran(suite, &ran, &wt);
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
    let (failing, names, executed) = outcome?;
    let _ = who
        .d
        .store
        .put_baseline(base, &suite.name, &failing, &names, executed, now_ms());
    Some((failing, names))
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
    let quarantined: Vec<String> = d
        .store
        .quarantined(now_ms())
        .unwrap_or_default()
        .into_iter()
        .filter(|q| q.0 == suite.name)
        .map(|q| q.1)
        .collect();
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
            let _ = std::fs::remove_file(cwd.join(f));
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
        let dev: Option<Dev> = match def
            .testing
            .environment(&suite.env)
            .and_then(|e| crate::targets::kind_of_env(e).map(|k| (k, e.os.clone())))
        {
            Some((kind, os)) => {
                let label = format!("#{} {}", who.task.map_or(0, |t| t.0 .0), suite.name);
                match d.targets.lease(kind, os.as_deref(), &label) {
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
        let ran = match run_hosted(
            d,
            &id,
            &crate::jobs::pty_wrap(
                suite.pty,
                &lifecycle::with_device(&suite.command, dev_id(dev.as_ref())),
            ),
            cwd,
            limit,
            &who.log_dir,
            check,
            who.run(),
            Some(suite),
            who.thresholds(),
            who.env.clone(),
        ) {
            Ok(r) => r,
            Err(e) => {
                env_down(who, def, suite, cwd, dev.as_ref());
                return SuiteRun::Product {
                    feedback: format!("`{}` could not run: {e}", suite.name),
                };
            }
        };
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
        let j = judge_ran(suite, &ran, cwd);
        let out = |extra: &str| {
            format!(
                "{extra}Last output of `{}`:\n{}",
                suite.name,
                tail(&j.output, 40)
            )
        };
        let result = match &j.verdict {
            Verdict::Green => {
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
                let base = if persistent.is_empty() {
                    None
                } else {
                    baseline(who, c_repo, def, suite, &tree.base, repo_lock, dev.as_ref())
                        .map(|b| b.0)
                };
                let class = classify(&Inputs {
                    failing: failures,
                    output: &j.output,
                    signatures,
                    rerun_failing: rerun.as_deref(),
                    baseline_failing: base.as_deref(),
                    quarantined: &quarantined,
                });
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
                        notes: vec![format!(
                            "{}: excused, also failing at the merge base ({}): {}",
                            suite.name,
                            &tree.base[..tree.base.len().min(10)],
                            tests.join(", ")
                        )],
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
                        let mut extra = format!("`{}` is red ({why}).", suite.name);
                        if !tests.is_empty() {
                            extra.push_str(&format!(" Failing: {}.", tests.join(", ")));
                        }
                        if !excused.is_empty() {
                            extra.push_str(&format!(
                                " Not yours (failing at the base too, or quarantined): {}.",
                                excused.join(", ")
                            ));
                        }
                        if !flaky.is_empty() {
                            extra.push_str(&format!(
                                " Flaked and passed on rerun: {}.",
                                flaky.join(", ")
                            ));
                        }
                        extra.push('\n');
                        SuiteRun::Product {
                            feedback: out(&extra),
                        }
                    }
                }
            }
        };
        env_down(who, def, suite, cwd, dev.as_ref());
        if waiting {
            on_wait(None);
        }
        return result;
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
        let _ = std::fs::remove_file(cwd.join(f));
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
    let j = judge_ran(suite, &ran, cwd);
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

pub fn run(c: &Ctx) {
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
        id_prefix: format!("t{}r{}", c.task.0, c.run.0),
        sink: None,
        env: vec![],
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
    let per_suite = sels.iter().any(|s| s.selected && s.suite.build.is_some());
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
        if per_suite {
            c.release_lease();
            let reused = matches!(d.store.evidence(&tree.tree, &suite.name), Ok(Some(_)));
            if let Some(b) = suite
                .build
                .as_ref()
                .filter(|b| !reused && !built.contains(b))
            {
                let needs = needs_of(&crate::jobs::build_uses(), None);
                who.env = held_env(c, &needs);
                if !lease(c, needs) {
                    return;
                }
                let ran = run_build(&who, &suite, b, built.len(), &c.wt);
                c.release_lease();
                match ran {
                    Built::Gone => return,
                    Built::Red(feedback) => {
                        outcomes.push((name.clone(), SuiteOutcome::Failed));
                        let (v, note) = closing(&outcomes);
                        let _ = d.store.set_closing(c.task.0, v.name(), &note, now_ms());
                        let reason = format!("the build of suite `{name}` is red");
                        return c.conclude(Outcome::Red, reason, Some(feedback));
                    }
                    Built::Ok => built.push(b.clone()),
                }
            }
            let needs = run_needs(&c.def, &c.step, &suite);
            who.env = held_env(c, &needs);
            if !lease(c, needs) {
                return;
            }
        }
        let ran = run_suite(
            &who,
            &c.project.path,
            &lock,
            &c.def,
            &suite,
            &c.wt,
            &tree,
            &mut on_wait,
        );
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
                }
                notes.extend(n);
                if let Some(r) = results {
                    head_results.push((sel.suite.clone(), r));
                }
                outcomes.push((name, outcome));
            }
            SuiteRun::Product { feedback } => {
                outcomes.push((name.clone(), SuiteOutcome::Failed));
                let (v, note) = closing(&outcomes);
                let _ = d.store.set_closing(c.task.0, v.name(), &note, now_ms());
                let reason = format!("suite `{name}` is red");
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
            let _ = std::fs::remove_file(wt.join(f));
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
        Some(judge_ran(suite, &ran, &wt).results)
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
/// the closing verdict is downgraded, and a debt is recorded.
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
    let (kind, name) = scope
        .split_once(':')
        .ok_or("scope is `suite:NAME` or `step:NAME`")?;
    let def = d
        .pinned_def(&t.def)
        .unwrap_or_else(|| d.current_def(t.project));
    match kind {
        "suite" if def.testing.suite(name).is_none() => {
            return Err(format!("there is no suite `{name}`"))
        }
        "step" if def.step(name).is_none() => return Err(format!("there is no step `{name}`")),
        "suite" | "step" => {}
        _ => return Err("scope is `suite:NAME` or `step:NAME`".into()),
    }
    d.store
        .add_debt(task.0, &project, scope, reason, owner, deferred, now_ms())
        .map_err(|e| e.to_string())
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
    let (kind, name) = row.scope.split_once(':').unwrap_or(("", ""));
    let suites: Vec<Suite> = match kind {
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
    };
    if suites.is_empty() {
        return Err("the debt's suite no longer exists in the definition".into());
    }
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
    let who = Who {
        d: d.clone(),
        task: None,
        step: String::new(),
        project: p.id.clone(),
        log_dir: dir.clone(),
        id_prefix: format!("debt{debt}"),
        sink: None,
        env: vec![],
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
                notes,
                ..
            } => verdicts.push(notes.join("; ")),
            SuiteRun::Done { notes, .. } => {
                all_green = false;
                verdicts.push(notes.join("; "));
            }
            SuiteRun::Product { feedback } => {
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
    if all_green {
        d.store
            .pay_debt(debt, now_ms())
            .map_err(|e| e.to_string())?;
        Ok(format!("paid: {}", verdicts.join("; ")))
    } else {
        Err(format!("still owed: {}", verdicts.join("; ")))
    }
}

/// After a task lands, run what it skipped and asked to have run later.
pub fn after_landing(d: &Arc<Daemon>, task: TaskId) {
    let Ok(rows) = d.store.debts(Some(task.0), true) else {
        return;
    };
    for r in rows.into_iter().filter(|r| r.deferred) {
        let d = d.clone();
        let _ = std::thread::Builder::new()
            .name(format!("debt-{}", r.id))
            .spawn(move || match pay(&d, r.id) {
                Ok(m) => tracing::info!(debt = r.id, "{m}"),
                Err(e) => tracing::warn!(debt = r.id, "deferred run: {e}"),
            });
    }
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
/// They belong to no task: nothing can skip them. Evidence is by tree, so a
/// tree that was green is not run twice, whoever asks. `base` is where the
/// batch started (baselines are taken there).
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
                    | SuiteOutcome::NotSelected,
                notes,
                ..
            } => out.notes.push(notes.join("; ")),
            SuiteRun::Done { notes, .. } => {
                out.green = false;
                out.failed.insert(name.clone());
                out.notes.push(notes.join("; "));
            }
            SuiteRun::Product { feedback } => {
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
