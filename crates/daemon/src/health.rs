// SPDX-License-Identifier: GPL-3.0-or-later

//! Pipeline self-correction (s432, decision 0425): a scheduled health check, and a health
//! session that proposes or makes fixes.
//!
//! **The check.** Facts are gathered here, off the broker's mutex (the world, the published wait
//! snapshot, the store, the process table and one timed file create), and judged by the pure
//! `mason_core::health::judge`. It runs on its own loop every `[health] interval_m` minutes
//! (default 30; 0: off) and on demand (`mason health`). Every check is stored.
//!
//! **The session.** A finding not handled within the cooldown starts one owner session of kind
//! `health`, with no task, in a read-only checkout of MASON's own project at its base. At most
//! one at a time; never while the provider account is held for usage. It ends with
//! `submit_health`: `false_alarm`, `acted` (only [`HEALTH_ACTIONS`](mason_adapter_api::contract::HEALTH_ACTIONS),
//! each logged as an intervention), `proposed` (one proposal per solution through
//! `proposals::file`), `needs_owner` (the session stays open, with a banner) or `direct_fix`.
//!
//! **The direct fix** is for one case: nothing can land for MASON's own project. The daemon
//! decides that itself (`mason_core::health::cannot_land`). The session then gets a writable
//! worktree on `claude/health-<id>`; a change to a protected path waits for the owner as
//! always, and the daemon pushes nothing and lands nothing by a side road: the finished branch
//! is handed to the owner with `needs_owner`.

use crate::cpu::CpuSource;
use crate::daemon::{now_ms, Daemon};
use mason_adapter_api::contract::{ContractHandle, NoChecks, HEALTH_ACTIONS};
use mason_adapter_api::{AgentEvent, SessionSpec, StepKind};
use mason_adapters::runner::{RunnerConfig, StepEnd, StepRunner};
use mason_core::health::{self, Facts, FailedFact, Finding, Limits, TaskFact};
use mason_core::*;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

/// The session kind.
pub const KIND: &str = "health";
/// Who the interventions of a health session are recorded under.
pub const WHO: &str = "health";
const DAY_MS: u64 = 86_400_000;

// ---- facts -----------------------------------------------------------------------------------

/// One timed file create in the data directory, in milliseconds: a disk that stalls shows here.
fn disk_probe_ms(d: &Daemon) -> u64 {
    let f = d
        .cfg
        .data_dir
        .join(format!(".mason-health-probe-{}", std::process::id()));
    let t = std::time::Instant::now();
    if std::fs::write(&f, b"x").is_ok() {
        let _ = std::fs::remove_file(&f);
    }
    t.elapsed().as_millis() as u64
}

/// A failure's signature: its first line without digits, hashes and paths' noise.
pub fn signature(failure: &str) -> String {
    let line = failure.lines().next().unwrap_or("").to_lowercase();
    let s: String = line
        .chars()
        .map(|c| if c.is_ascii_digit() { '#' } else { c })
        .collect();
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    s.chars().take(80).collect()
}

/// The CPU percentage and where it came from: the configured source, else the
/// `MASON_CPU_FILE` test file, else the load average.
fn load_reading(
    cpu: Option<&dyn CpuSource>,
    cpu_file: Option<std::ffi::OsString>,
) -> (Option<u32>, Option<&'static str>) {
    match (cpu, cpu_file) {
        (Some(c), _) => {
            let (p, s) = c.reading();
            (p, Some(s))
        }
        (None, Some(f)) => {
            let p = crate::cpu::FileCpu(f.into()).busy_pct();
            (p, p.map(|_| "file"))
        }
        (None, None) => {
            let p = crate::cpu::read_loadavg();
            (p, p.map(|_| "loadavg"))
        }
    }
}

/// Everything the judgement needs. Takes no broker lock: the wait reasons are the published
/// snapshot.
pub fn gather(d: &Arc<Daemon>, now: u64) -> Facts {
    let mut x = Facts {
        cpu_gate_held_ms: d.broker.cpu_gate_held_ms(),
        ..Default::default()
    };
    let Ok(w) = d.world() else { return x };
    let ix = crate::views::RunIndex::build(w.step_runs.values());
    let waits = d.broker.waits_snapshot();
    let holders = d.broker.waits_holders();
    let diagnoses = d.store.diagnoses(None).unwrap_or_default();
    let self_pid = crate::proposals::self_project(d).map(|(p, _)| p);
    let mut landing_failed = 0;
    let mut landed = 0;
    for t in w.tasks.values() {
        let in_state = now.saturating_sub(t.state_entered_at.0);
        let last_start = ix.last_start_of(t.id);
        let wait = waits.get(&t.id).cloned();
        let hf = holders.get(&t.id).and_then(|(since, depth, position)| {
            health::holder_fact(since, now, *depth, *position)
        });
        let since_run = now.saturating_sub(t.state_entered_at.0.max(last_start));
        match t.state {
            State::Queued => {
                if t.blockers.is_empty() && !t.paused && wait.is_none() {
                    x.queued += 1;
                    x.queued_oldest_ms = x.queued_oldest_ms.max(in_state);
                }
            }
            State::Landed if Some(t.project) == self_pid => {
                if in_state < DAY_MS {
                    landed += 1;
                }
            }
            State::Failed => {
                let class_code = t.failure_class == Some(FailClass::Code);
                let wants = class_code && d.project(t.project).is_some_and(|p| p.diagnose);
                let failure = t.failure.clone().unwrap_or_default();
                if Some(t.project) == self_pid
                    && in_state < DAY_MS
                    && failure.to_lowercase().contains("land")
                {
                    landing_failed += 1;
                }
                x.failed.push(FailedFact {
                    task: t.id.0,
                    signature: signature(&failure),
                    failed_ms: in_state,
                    undiagnosed: wants && !diagnoses.iter().any(|r| r.task == t.id.0),
                });
            }
            _ => {}
        }
        if t.state.is_step() {
            let running = d.has_job_for_task(t.id);
            if running {
                x.running += 1;
            }
            let silent = d
                .progress_of_task(t.id)
                .iter()
                .map(|p| p.updated_at_ms)
                .max()
                .map_or(in_state, |at| now.saturating_sub(at));
            // A host queued for a lease is judged by overloaded:slots and
            // stuck:wait:N, not as silent.
            if running && silent > 0 && wait.is_none() {
                x.silent_hosts.push((t.id.0, silent));
            }
            x.tasks.push(TaskFact {
                id: t.id.0,
                state: mason_store::state_name(t.state).to_string(),
                is_step: true,
                in_state_ms: in_state,
                typical_hi_ms: ix.typical_hi(&t.step),
                silent_ms: silent,
                waiting: wait.clone().map(|r| (r, since_run)),
                holders: hf.clone(),
            });
        } else if t.state == State::Queued {
            if let Some(r) = wait.clone() {
                x.tasks.push(TaskFact {
                    id: t.id.0,
                    state: "queued".into(),
                    is_step: false,
                    in_state_ms: in_state,
                    typical_hi_ms: None,
                    silent_ms: in_state,
                    waiting: Some((r, in_state)),
                    holders: hf.clone(),
                });
            }
        }
        if let Some(r) = wait {
            let ms = if t.state.is_step() {
                since_run
            } else {
                in_state
            };
            x.slot_waits.push((t.id.0, r, ms));
        }
    }
    // Free slots: nothing that could start is waiting on a resource, and nothing runs.
    x.free_slots = x.running == 0 && waits.is_empty();
    x.paused = !d.store.pauses(now).unwrap_or_default().is_empty();
    x.landing_failed = landing_failed;
    x.landed = landed;
    // Pending diagnoses that still need the owner.
    for r in diagnoses.iter().filter(|r| r.status == "pending") {
        let task_failed = w
            .tasks
            .get(&TaskId(r.task))
            .is_some_and(|t| t.state == State::Failed);
        if task_failed {
            x.pending_diagnoses
                .push((r.id, r.task, now.saturating_sub(r.at_ms)));
        }
    }
    for f in d.store.freezes().unwrap_or_default() {
        x.red.push((
            format!("landing:{}", f.project),
            now.saturating_sub(f.since),
        ));
    }
    // Overload.
    x.load_assumed = d
        .cfg
        .cpu
        .as_ref()
        .is_some_and(|c| c.source() == "assumed-busy");
    (x.load_pct, x.load_source) =
        load_reading(d.cfg.cpu.as_deref(), std::env::var_os("MASON_CPU_FILE"));
    x.load_assumed = x.load_assumed || x.load_source == Some("assumed-busy");
    x.mem_free_mb = crate::sched::measure_free_memory_mb();
    x.stall_ms = disk_probe_ms(d);
    // The launch list is process-wide; tests turn this knob off so they don't read each other's.
    let launching = if d.cfg.launch_frozen_alert {
        mason_step_host::launcher::launching_under(&d.cfg.hosts_dir())
    } else {
        Vec::new()
    };
    x.frozen_launches = launching
        .iter()
        .map(|l| {
            (
                crate::sweep::parse_host_id(&l.id).map(|(t, _)| t),
                // Live state of this process: judged by the wall clock, whatever `now` is.
                now_ms().saturating_sub(l.since_ms),
            )
        })
        .collect();
    let limits = &d.cfg.health.limits;
    let frozen = x
        .frozen_launches
        .iter()
        .any(|(_, ms)| *ms > limits.frozen_launch_ms);
    if frozen {
        x.syspolicyd_down = crate::attention::syspolicyd_running() == Some(false);
    }
    x.launch_blocked = frozen;
    // Compiles stuck in the kernel (a build directory too big to list), from one process table.
    if let Ok(o) = std::process::Command::new("/bin/ps")
        .args(["-axo", "pid=,stat=,etime=,time=,command="])
        .output()
    {
        x.procs = health::parse_ps(&String::from_utf8_lossy(&o.stdout))
            .into_iter()
            .filter(|p| p.state.contains('U'))
            .collect();
    }
    // The daemon: restarts, late loops and a failing self-update, from what earlier checks logged.
    let rows = d
        .store
        .health_since(
            &[
                "start",
                "planned",
                "cancelled",
                "late",
                "update_failed",
                "update_ok",
            ],
            now.saturating_sub(DAY_MS),
        )
        .unwrap_or_default();
    let seq: Vec<&str> = rows.iter().map(|r| r.kind.as_str()).collect();
    x.restarts = health::unplanned_starts(&seq);
    x.late_loops = rows.iter().filter(|r| r.kind == "late").count() as u32;
    let mut fails = 0;
    for r in rows.iter().filter(|r| r.kind.starts_with("update_")) {
        fails = if r.kind == "update_failed" {
            fails + 1
        } else {
            0
        };
    }
    let failing_now = d
        .update_status
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .phase
        == "failed";
    x.update_failures = if failing_now { fails.max(1) } else { 0 };
    x.update_blocked = failing_now && x.update_failures >= limits.update_failures;
    x
}

/// The facts as they are now, judged.
pub fn report(d: &Arc<Daemon>, now: u64) -> (Facts, Vec<Finding>) {
    let facts = gather(d, now);
    let findings = health::judge(&facts, &d.cfg.health.limits);
    (facts, findings)
}

pub fn finding_json(f: &Finding) -> Value {
    json!({
        "key": f.key, "category": f.category, "severity": f.severity.name(),
        "summary": f.summary, "evidence": f.evidence,
    })
}

// ---- the check -------------------------------------------------------------------------------

/// What one tick did.
#[derive(Debug, Default)]
pub struct Tick {
    pub findings: Vec<Finding>,
    /// Findings not handled within the cooldown.
    pub fresh: Vec<String>,
    /// The session this tick started.
    pub session: Option<u64>,
    /// Why no session started, when there were fresh findings.
    pub held_back: String,
}

/// The open health session, if any.
pub fn open_session(d: &Daemon) -> Option<mason_store::owner_sessions::OwnerSessionRow> {
    d.store
        .open_owner_sessions()
        .unwrap_or_default()
        .into_iter()
        .find(|s| s.kind == KIND)
}

/// One check at `now`. Callable directly: tests drive it with a time of their own.
pub fn tick(d: &Arc<Daemon>, now: u64) -> Tick {
    let (_facts, findings) = report(d, now);
    note_late_loop(d, now);
    note_update(d, now);
    let keys: Vec<&str> = findings.iter().map(|f| f.key.as_str()).collect();
    let _ = d.store.add_health(
        "check",
        "",
        None,
        &json!({"findings": keys}).to_string(),
        now,
    );
    let limits = &d.cfg.health.limits;
    let handled: Vec<(String, u64)> = d
        .store
        .health_since(&["handled"], now.saturating_sub(limits.cooldown_ms))
        .unwrap_or_default()
        .into_iter()
        .map(|r| (r.key, r.at_ms))
        .collect();
    let fresh = health::fresh(&findings, &handled, now, limits.cooldown_ms);
    let mut out = Tick {
        fresh: fresh.iter().map(|f| f.key.clone()).collect(),
        findings,
        ..Default::default()
    };
    if fresh.is_empty() {
        return out;
    }
    if open_session(d).is_some() {
        out.held_back = "a health session is open".into();
        return out;
    }
    let Some((pid, project)) = crate::proposals::self_project(d) else {
        out.held_back = "no project of MASON's own is registered".into();
        return out;
    };
    let account = d.account_of(pid).to_string();
    if crate::holds::hold_of(d, &account).is_some() {
        out.held_back = "the provider account is held".into();
        return out;
    }
    let severe = health::severe(&fresh);
    let prompt = prompt(d, &out.findings, &fresh, now);
    let Ok(sid) = d.store.add_owner_session(None, KIND, &account, WHO, now) else {
        return out;
    };
    let _ = d.store.set_owner_session_state(sid, "working", now);
    for f in &fresh {
        let _ = d
            .store
            .add_health("handled", &f.key, Some(sid), &f.summary, now);
    }
    let _ = d.store.add_health(
        "session",
        "",
        Some(sid),
        &json!({"keys": out.fresh, "severe": severe}).to_string(),
        now,
    );
    let _ = d.store.add_owner_message(
        sid,
        "tool",
        &format!("health check: {}", out.fresh.join(", ")),
        now,
    );
    crate::attention::intervene(d, "health_session", None, WHO, &out.fresh.join(", "), "");
    let _ = project;
    d.poke();
    out.session = Some(sid);
    let d2 = d.clone();
    let _ = std::thread::Builder::new()
        .name(format!("health-{sid}"))
        .spawn(move || {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_session(&d2, sid, &prompt, severe)
            }));
            if r.is_err() {
                let _ = d2.store.add_owner_message(
                    sid,
                    "tool",
                    "the health session failed inside the daemon",
                    now_ms(),
                );
                end_session(&d2, sid);
            }
        });
    out
}

/// A late event loop since the last check is one `late` row.
fn note_late_loop(d: &Daemon, now: u64) {
    let (n, _) = d.stall_stats();
    let last: u64 = d
        .store
        .health_since(&["stalls"], 0)
        .unwrap_or_default()
        .last()
        .and_then(|r| r.detail.parse().ok())
        .unwrap_or(0);
    if n > last {
        let _ = d
            .store
            .add_health("late", "", None, &format!("{}", n - last), now);
    }
    if n != last {
        let _ = d.store.add_health("stalls", "", None, &n.to_string(), now);
    }
}

/// The self-update's failure, once per check while it lasts; its recovery ends the run.
fn note_update(d: &Daemon, now: u64) {
    let failed = d
        .update_status
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .phase
        == "failed";
    let last = d
        .store
        .health_since(&["update_failed", "update_ok"], 0)
        .unwrap_or_default()
        .last()
        .map(|r| r.kind.clone());
    if failed {
        let _ = d.store.add_health("update_failed", "", None, "", now);
    } else if last.as_deref() == Some("update_failed") {
        let _ = d.store.add_health("update_ok", "", None, "", now);
    }
}

/// Record a daemon start, and run the loop.
pub fn on_start(d: &Arc<Daemon>) {
    let _ = d.store.add_health("start", "", None, "", now_ms());
    // A session that was running when the daemon stopped did not finish: end it.
    for s in d.store.open_owner_sessions().unwrap_or_default() {
        if s.kind == KIND && s.adapter_session.is_empty() && s.turns == 0 {
            let _ = d.store.add_owner_message(
                s.id,
                "tool",
                "the daemon restarted before the session finished",
                now_ms(),
            );
            end_session(d, s.id);
        }
    }
    let every = d.cfg.health.interval_m;
    if every == 0 {
        return;
    }
    let mut last = now_ms();
    crate::bg::spawn_loop(d, "health", move |weak| {
        std::thread::sleep(Duration::from_secs(30));
        let Some(d) = weak.upgrade() else {
            return false;
        };
        let now = now_ms();
        if now.saturating_sub(last) >= every.saturating_mul(60_000) {
            last = now;
            tick(&d, now);
        }
        true
    });
}

// ---- the session -----------------------------------------------------------------------------

/// The directory a session's checkout lives in.
pub fn checkout_dir(d: &Daemon, sid: u64) -> std::path::PathBuf {
    d.cfg.data_dir.join("health").join(sid.to_string())
}

/// The first message: the report, then the events behind each finding.
pub fn prompt(d: &Arc<Daemon>, all: &[Finding], fresh: &[Finding], now: u64) -> String {
    let mut s = String::from(
        "You are MASON's pipeline health session. The scheduled health check found problems \
         with the pipeline itself. Work out what is wrong and whether it can be fixed. You are in \
         a read-only checkout of MASON's own repository at its base; you may read logs, run \
         `mason` read commands and read the store, and you may not change files.\n\n\
         End with exactly one call of `submit_health`, choosing one or more of: `false_alarm` \
         (why the finding was wrong; if a threshold is wrong, add a proposal to change it), \
         `acted` (an operational fix through these owner-safe actions only: ",
    );
    s.push_str(&HEALTH_ACTIONS.join(", "));
    s.push_str(
        "), `proposed` (every possible solution, one entry each, with evidence naming files; the \
         default for anything that needs a code change), `needs_owner` (only the owner can \
         decide; the recommendation first) or `direct_fix` (only when nothing can land for \
         MASON's own project; the daemon decides that, not you).\n\n# The report\n",
    );
    for f in all {
        s.push_str(&format!(
            "- [{}] `{}` ({}): {}\n",
            f.severity.name(),
            f.key,
            f.category,
            f.summary
        ));
        for e in &f.evidence {
            s.push_str(&format!("    - {e}\n"));
        }
    }
    s.push_str("\n# New since the last session\n");
    for f in fresh {
        s.push_str(&format!("- `{}`\n", f.key));
    }
    s.push_str("\n# The events behind each finding\n");
    let w = d.world().ok();
    for f in fresh {
        s.push_str(&format!("## `{}`\n", f.key));
        let task = f.key.rsplit(':').next().and_then(|n| n.parse::<u64>().ok());
        if let (Some(t), Some(w)) = (task, w.as_ref()) {
            if let Some(task) = w.tasks.get(&TaskId(t)) {
                s.push_str(&format!(
                    "task #{t} \"{}\" is {}, for {} min; failure: {}\n",
                    task.title,
                    mason_store::state_name(task.state),
                    now.saturating_sub(task.state_entered_at.0) / 60_000,
                    task.failure.as_deref().unwrap_or("(none)")
                ));
                for p in d.progress_of_task(TaskId(t)) {
                    s.push_str(&format!("progress: step {} {}\n", p.step, p.message));
                }
            }
            for i in d
                .store
                .interventions(now.saturating_sub(DAY_MS))
                .unwrap_or_default()
                .iter()
                .filter(|i| i.task == Some(t))
            {
                s.push_str(&format!(
                    "intervention: {} by {} {}\n",
                    i.kind, i.who, i.detail
                ));
            }
        }
    }
    let ints = d
        .store
        .interventions(now.saturating_sub(6 * 3_600_000))
        .unwrap_or_default();
    s.push_str("\n# Interventions in the last six hours\n");
    for i in ints.iter().rev().take(30) {
        s.push_str(&format!("- {} {} {}\n", i.kind, i.who, i.detail));
    }
    s
}

fn end_session(d: &Arc<Daemon>, sid: u64) {
    let _ = d.store.set_owner_session_state(sid, "ended", now_ms());
    remove_checkout(d, sid);
    d.poke();
}

pub fn remove_checkout(d: &Daemon, sid: u64) {
    let dir = checkout_dir(d, sid);
    if !dir.exists() {
        return;
    }
    if let Some((_, p)) = crate::proposals::self_project(d) {
        let _ = mason_landing::git::run(
            &p.path,
            &["worktree", "remove", "--force", &dir.to_string_lossy()],
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The read-only checkout: a detached worktree at the project's base. The project's own
/// directory when the worktree cannot be made (a read of the same files).
fn make_checkout(d: &Daemon, sid: u64, p: &crate::config::ProjectCfg) -> std::path::PathBuf {
    let dir = checkout_dir(d, sid);
    let _ = std::fs::create_dir_all(dir.parent().unwrap_or(&dir));
    let ok = mason_landing::git::ok(
        &p.path,
        &[
            "worktree",
            "add",
            "--detach",
            &dir.to_string_lossy(),
            &p.base_ref(),
        ],
    );
    if ok {
        dir
    } else {
        p.path.clone()
    }
}

fn run_session(d: &Arc<Daemon>, sid: u64, text: &str, severe: bool) {
    let Some((pid, project)) = crate::proposals::self_project(d) else {
        end_session(d, sid);
        return;
    };
    let project = project.clone();
    let wt = make_checkout(d, sid, &project);
    let agent = project.agent();
    let eff = crate::tiermap::effective(&d.cfg.config_dir, agent.provider, &project.overlay_text);
    let tier = if severe {
        mason_definition::tiers::Tier::Deep
    } else {
        mason_definition::tiers::Tier::Light
    };
    let mapped = eff
        .mapping
        .entry(tier, mason_definition::tiers::Level::default());
    let adapter = crate::jobs::adapter_for(d, &project, &wt);
    let runner = StepRunner::new(adapter.as_ref(), RunnerConfig::default());
    let contract = ContractHandle::new(StepKind::Health, Box::new(NoChecks));
    let key = format!("health{sid}");
    if contract
        .serve(&d.cfg.sockets_dir().join(format!("{key}.sock")))
        .is_err()
    {
        let _ = d.store.add_owner_message(
            sid,
            "tool",
            "the contract socket could not be made",
            now_ms(),
        );
        end_session(d, sid);
        return;
    }
    let mut spec = SessionSpec::new(StepKind::Health, text, &wt);
    spec.model = mapped.model;
    spec.effort = mapped.effort;
    spec.env = project.agent_env.clone();
    spec.env.retain(|(k, _)| k != "MASON_TASK");
    spec.tool_timeout = Duration::from_secs(120);
    spec.deadline = Some(Duration::from_secs(1800));
    spec.run_key = Some(key);
    let (dd, id) = (d.clone(), sid);
    spec.on_agent_event = Some(Arc::new(move |ev: &AgentEvent| match ev {
        AgentEvent::Text(t) if !t.trim().is_empty() => {
            let _ = dd.store.add_owner_message(id, "agent", t, now_ms());
            dd.poke();
        }
        AgentEvent::ToolCall { name, summary } => {
            let _ = dd.store.add_owner_message(
                id,
                "tool",
                &crate::usersession::tool_line(name, summary),
                now_ms(),
            );
            dd.poke();
        }
        _ => {}
    }));
    let report = runner.run(spec, &contract);
    contract.shutdown();
    let StepEnd::Accepted { submission, .. } = report.end else {
        let _ =
            d.store
                .add_owner_message(sid, "tool", "the session ended without a verdict", now_ms());
        let _ = d
            .store
            .add_health("verdict", "none", Some(sid), "{}", now_ms());
        end_session(d, sid);
        return;
    };
    let _ = d
        .store
        .finish_owner_turn(sid, report.session_id.as_deref(), now_ms());
    let waits = apply(d, sid, pid, &submission, now_ms());
    if waits {
        // The session stays open for the owner, who can write in it.
        let _ = d.store.set_owner_session_state(sid, "open", now_ms());
        d.poke();
    } else {
        end_session(d, sid);
    }
}

/// Do what a verdict says. Returns whether the owner is needed (the session stays open).
pub fn apply(d: &Arc<Daemon>, sid: u64, self_pid: ProjectId, a: &Value, now: u64) -> bool {
    let say = |t: &str| {
        let _ = d.store.add_owner_message(sid, "agent", t, now_ms());
    };
    let mut kinds: Vec<&str> = vec![];
    let mut lines: Vec<String> = vec![];
    let mut needs: Option<(String, String)> = None;
    say(a["summary"].as_str().unwrap_or(""));

    if let Some(fa) = a.get("false_alarm").filter(|v| v.is_object()) {
        kinds.push("false_alarm");
        lines.push(format!(
            "false alarm {}: {}",
            fa["finding"].as_str().unwrap_or(""),
            fa["why"].as_str().unwrap_or("")
        ));
        if let Some(t) = fa.get("threshold_proposal").filter(|v| v.is_object()) {
            let key = fa["finding"].as_str().unwrap_or("threshold");
            lines.push(file(d, sid, self_pid, key, t));
        }
    }
    if let Some(list) = a["acted"].as_array() {
        kinds.push("acted");
        for x in list {
            let action = x["action"].as_str().unwrap_or("");
            let task = x["task"].as_u64().unwrap_or(0);
            let reason = x["reason"].as_str().unwrap_or("");
            let r = act(d, action, task);
            crate::attention::intervene(
                d,
                &format!("health_{action}"),
                Some(TaskId(task)),
                WHO,
                &format!(
                    "{}: {reason}",
                    r.as_ref().map_or_else(|e| e.clone(), |o| o.clone())
                ),
                reason,
            );
            let _ = d.store.add_health(
                "action",
                action,
                Some(sid),
                &json!({"task": task, "reason": reason, "result": format!("{r:?}")}).to_string(),
                now,
            );
            lines.push(match r {
                Ok(o) => format!("{action} #{task}: {o}"),
                Err(e) => format!("{action} #{task} refused: {e}"),
            });
        }
    }
    if let Some(list) = a["proposed"].as_array() {
        kinds.push("proposed");
        for x in list {
            let key = x["finding"].as_str().unwrap_or("").to_string();
            lines.push(file(d, sid, self_pid, &key, x));
        }
    }
    if let Some(n) = a.get("needs_owner").filter(|v| v.is_object()) {
        kinds.push("needs_owner");
        needs = Some((
            n["question"].as_str().unwrap_or("").to_string(),
            n["recommendation"].as_str().unwrap_or("").to_string(),
        ));
    }
    if let Some(f) = a.get("direct_fix").filter(|v| v.is_object()) {
        kinds.push("direct_fix");
        match direct_fix(d, sid, f, now) {
            Ok(line) => {
                lines.push(line.clone());
                needs = Some(("A fix is ready on a branch and needs landing".into(), line));
            }
            Err(why) => {
                lines.push(format!("direct fix refused: {why}"));
                if needs.is_none() {
                    needs = Some((
                        format!("The direct fix was not made: {why}"),
                        a["summary"].as_str().unwrap_or("").to_string(),
                    ));
                }
            }
        }
    }
    for l in &lines {
        let _ = d.store.add_owner_message(sid, "tool", l, now_ms());
    }
    if let Some((q, r)) = &needs {
        say(&format!("Needs you: {q}\nRecommended: {r}"));
    }
    let _ = d.store.add_health(
        "verdict",
        &kinds.join("+"),
        Some(sid),
        &json!({
            "summary": a["summary"], "outcomes": lines,
            "question": needs.as_ref().map(|n| n.0.clone()),
            "recommendation": needs.as_ref().map(|n| n.1.clone()),
        })
        .to_string(),
        now,
    );
    needs.is_some()
}

/// File one proposal to MASON's own project. The finding key is the dedupe key: a proposal
/// already filed for it and its title, and still live, is not filed again.
fn file(d: &Arc<Daemon>, sid: u64, pid: ProjectId, key: &str, x: &Value) -> String {
    let title = x["title"].as_str().unwrap_or("").trim().to_string();
    let dedupe = format!("{key}|{title}");
    let live = d
        .store
        .health_since(&["proposal"], 0)
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.key == dedupe)
        .filter_map(|r| r.detail.parse::<u64>().ok())
        .any(|task| {
            d.world().ok().is_some_and(|w| {
                w.tasks.get(&TaskId(task)).is_some_and(|t| {
                    !matches!(t.state, State::Landed | State::Canceled | State::Failed)
                })
            })
        });
    if live {
        return format!("proposal for {key} already filed: {title:?}");
    }
    let f = crate::proposals::Filing {
        from_task: None,
        project: pid,
        title: title.clone(),
        body: format!(
            "{}\n\n_Filed by the health session #{sid} for finding `{key}`._",
            x["body"].as_str().unwrap_or("")
        ),
        evidence: x["evidence"].as_str().unwrap_or("").to_string(),
        blocks_task: false,
        explored: false,
        estimate: None,
        lookback: None,
    };
    match crate::proposals::file(d, &f) {
        Ok(filed) => {
            let _ = d.store.add_health(
                "proposal",
                &dedupe,
                Some(sid),
                &filed.task.0.to_string(),
                now_ms(),
            );
            format!("filed proposal #{}: {title:?}", filed.task.0)
        }
        Err(e) => format!("proposal {title:?} not filed: {e}"),
    }
}

/// One owner-safe action, as the owner could take it.
fn act(d: &Arc<Daemon>, action: &str, task: u64) -> Result<String, String> {
    if !HEALTH_ACTIONS.contains(&action) {
        return Err(format!("{action:?} is not an allowed action"));
    }
    let w = d.world().map_err(|e| e.to_string())?;
    let t = w.tasks.get(&TaskId(task)).ok_or("no such task")?;
    let latest = crate::http::latest_run(&w, TaskId(task));
    match action {
        "retry" => {
            if t.state != State::Failed && t.state != State::NeedsAttention {
                return Err(format!(
                    "#{task} is {}; only a failed task is retried",
                    mason_store::state_name(t.state)
                ));
            }
            drop(w);
            crate::http::retry_task(
                d,
                task,
                crate::http::RetryAsk {
                    seen: Some(latest),
                    ..Default::default()
                },
            )
            .map(|(to, step)| format!("retried from `{step}` ({})", mason_store::state_name(to)))
            .map_err(|(_, e)| e.0["error"].as_str().unwrap_or("refused").to_string())
        }
        "unblock" => {
            drop(w);
            let who = d.cfg.owner_names.first().cloned().unwrap_or_default();
            crate::blockers::release(d, &who, TaskId(task), crate::blockers::Which::All)
                .map_err(|(_, e)| e)
        }
        "release_holder" => {
            drop(w);
            let reg = mason_step_host::registry::Registry::new(d.cfg.hosts_dir());
            let mut stopped = 0;
            for h in reg.scan().unwrap_or_default() {
                let mine = crate::sweep::parse_host_id(&h.info.id).is_some_and(|(t, _)| t == task);
                if mine
                    && h.state == mason_step_host::registry::HostState::Running
                    && !crate::hung::is_agent_host(&h.info.id)
                {
                    d.stop_hosts(&h.info.id);
                    stopped += 1;
                }
            }
            if stopped == 0 {
                Err(format!("#{task} has no command host to stop"))
            } else {
                Ok(format!("stopped {stopped} command host(s) of #{task}"))
            }
        }
        _ => {
            if t.state != State::Failed {
                return Err(format!("#{task} is not failed"));
            }
            drop(w);
            crate::diagnose::on_failed(d, TaskId(task), StepRunId(latest));
            Ok(format!(
                "diagnosis of #{task} started (at most {} per task)",
                crate::diagnose::MAX_PER_TASK
            ))
        }
    }
}

// ---- the direct fix --------------------------------------------------------------------------

/// Direct fixes tried so far that did not end well, today.
const MAX_FAILED_FIXES: usize = 2;

/// The branch of a session's direct fix.
pub fn fix_branch(sid: u64) -> String {
    format!("claude/health-{sid}")
}

/// Only when the daemon itself finds that nothing can land for MASON's own project. Makes a
/// writable worktree on `claude/health-<id>` from the base, lets the agent make the smallest
/// fix there, and hands the branch to the owner: nothing is pushed or landed from here.
fn direct_fix(d: &Arc<Daemon>, sid: u64, f: &Value, now: u64) -> Result<String, String> {
    let facts = gather(d, now);
    let why = health::cannot_land(&facts, &d.cfg.health.limits).ok_or(
        "the pipeline can still take in a task, so it can heal itself: propose the fix instead",
    )?;
    let rows = d
        .store
        .health_since(
            &["direct_fix", "direct_fix_failed"],
            now.saturating_sub(DAY_MS),
        )
        .unwrap_or_default();
    if rows
        .iter()
        .filter(|r| r.kind == "direct_fix_failed")
        .count()
        >= MAX_FAILED_FIXES
    {
        return Err("two direct fixes failed today; the owner decides".into());
    }
    let open = rows.iter().filter(|r| r.kind == "direct_fix").count()
        > rows
            .iter()
            .filter(|r| r.kind == "direct_fix_failed" || r.kind == "direct_fix_done")
            .count();
    if open {
        return Err("a direct fix is already under way".into());
    }
    let (_, project) = crate::proposals::self_project(d).ok_or("no project of MASON's own")?;
    let project = project.clone();
    let _ = d
        .store
        .add_health("direct_fix", &fix_branch(sid), Some(sid), &why, now);
    let result = make_fix(d, sid, &project, f);
    let kind = if result.is_ok() {
        "direct_fix_done"
    } else {
        "direct_fix_failed"
    };
    let _ = d.store.add_health(
        kind,
        &fix_branch(sid),
        Some(sid),
        &format!("{result:?}"),
        now_ms(),
    );
    result
}

fn make_fix(
    d: &Arc<Daemon>,
    sid: u64,
    p: &crate::config::ProjectCfg,
    f: &Value,
) -> Result<String, String> {
    let dir = d.cfg.data_dir.join("health").join(format!("fix-{sid}"));
    let _ = std::fs::create_dir_all(dir.parent().unwrap_or(&dir));
    let branch = fix_branch(sid);
    let base = p.base_ref();
    mason_landing::git::run(
        &p.path,
        &[
            "worktree",
            "add",
            "-b",
            &branch,
            &dir.to_string_lossy(),
            &base,
        ],
    )
    .map_err(|e| format!("the worktree could not be made: {e}"))?;
    let adapter = crate::jobs::adapter_for(d, p, &dir);
    let text = format!(
        "Make the smallest fix for this, in this worktree (branch {branch}), and commit it with \
         `git commit -s`. Do not push. It must pass fmt, clippy, the tests and the exit scripts \
         before it lands; run what you can. The fix:\n\n{}",
        f["summary"].as_str().unwrap_or("")
    );
    let mut spec = SessionSpec::new(StepKind::Chat, &text, &dir);
    spec.env = p.agent_env.clone();
    spec.env.retain(|(k, _)| k != "MASON_TASK");
    spec.tool_timeout = Duration::from_secs(120);
    spec.deadline = Some(Duration::from_secs(1800));
    spec.run_key = Some(format!("health{sid}fix"));
    let mut h = adapter
        .start(spec)
        .map_err(|e| format!("the agent could not be started: {e}"))?;
    while let Some(ev) = h.next_event() {
        if matches!(ev, AgentEvent::Finished(_)) {
            break;
        }
    }
    let changed =
        mason_landing::git::run(&dir, &["diff", "--name-only", &format!("{base}...HEAD")])
            .map_err(|e| e.to_string())?;
    let files: Vec<&str> = changed.lines().filter(|l| !l.trim().is_empty()).collect();
    if files.is_empty() {
        return Err("the agent made no commit".into());
    }
    let protected: Vec<String> = files
        .iter()
        .filter_map(|f| crate::protect::protected_because(f, &[]).map(|why| format!("{f} ({why})")))
        .collect();
    let mut line = format!(
        "the fix is on branch {branch} in {} ({} file(s)); the daemon did not push or land it, and \
         did not run fmt, clippy, the tests or the exit scripts on it",
        dir.display(),
        files.len()
    );
    if !protected.is_empty() {
        line.push_str(&format!(
            "; it changes protected paths and waits for your approval: {}",
            protected.join(", ")
        ));
    }
    Ok(line)
}

// ---- views and history -------------------------------------------------------------------------

/// The header banner: a health session that waits for the owner. `(session, text)`.
pub fn banner(
    d: &Daemon,
    sessions: &[mason_store::owner_sessions::OwnerSessionRow],
) -> Option<(u64, String)> {
    let s = sessions
        .iter()
        .find(|s| s.kind == KIND && s.state == "open")?;
    let detail = d
        .store
        .health_since(&["verdict"], 0)
        .unwrap_or_default()
        .into_iter()
        .rev()
        .find(|r| r.session == Some(s.id))
        .and_then(|r| serde_json::from_str::<Value>(&r.detail).ok());
    let text = detail
        .as_ref()
        .and_then(|v| v["question"].as_str().map(String::from))
        .or_else(|| {
            detail
                .as_ref()
                .and_then(|v| v["summary"].as_str().map(String::from))
        })
        .unwrap_or_else(|| "the pipeline health session needs you".into());
    let line = text.lines().next().unwrap_or("").to_string();
    Some((s.id, format!("Pipeline health #{}: {line}", s.id)))
}

/// `mason health` / `GET /health`: the report as it is now, JSON.
pub fn report_json(d: &Arc<Daemon>, now: u64) -> Value {
    let (_, findings) = report(d, now);
    let l: &Limits = &d.cfg.health.limits;
    json!({
        "healthy": findings.is_empty(),
        "findings": findings.iter().map(finding_json).collect::<Vec<_>>(),
        "interval_m": d.cfg.health.interval_m,
        "cooldown_h": l.cooldown_ms / 3_600_000,
        "session": open_session(d).map(|s| s.id),
    })
}

/// `mason health history`: what self-correction did, newest first.
pub fn history_json(d: &Daemon, n: usize) -> Value {
    let rows = d.store.health_recent(n.max(1) * 4).unwrap_or_default();
    let shown: Vec<Value> = rows
        .iter()
        .filter(|r| {
            matches!(
                r.kind.as_str(),
                "check"
                    | "session"
                    | "verdict"
                    | "action"
                    | "proposal"
                    | "direct_fix"
                    | "direct_fix_done"
                    | "direct_fix_failed"
            )
        })
        .take(n)
        .map(|r| {
            json!({"id": r.id, "at_ms": r.at_ms, "kind": r.kind, "key": r.key,
                   "session": r.session, "detail": r.detail})
        })
        .collect();
    json!({"history": shown})
}

#[cfg(test)]
mod load_tests {
    use super::*;

    #[derive(Debug)]
    struct Fixed(u32, &'static str);

    impl CpuSource for Fixed {
        fn busy_pct(&self) -> Option<u32> {
            Some(self.0)
        }
        fn source(&self) -> &'static str {
            self.1
        }
    }

    #[test]
    fn load_reading_carries_the_source() {
        let busy = Fixed(100, "assumed-busy");
        assert_eq!(
            load_reading(Some(&busy), None),
            (Some(100), Some("assumed-busy"))
        );
        let mach = Fixed(40, "mach");
        assert_eq!(load_reading(Some(&mach), None), (Some(40), Some("mach")));
    }

    #[test]
    fn file_reading_is_called_file() {
        let dir = std::env::temp_dir().join(format!("mason-load-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("cpu");
        std::fs::write(&f, "37\n").unwrap();
        assert_eq!(load_reading(None, Some(f.into())), (Some(37), Some("file")));
        std::fs::remove_dir_all(&dir).ok();
    }
}
