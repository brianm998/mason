// SPDX-License-Identifier: GPL-3.0-or-later

//! MASON working on its own repository (Part 13), as the *running* daemon sees
//! it. A project with a `[self]` section is MASON's own:
//!
//! - **Holds.** Before a self-change joins the landing queue, the running
//!   version's compiled rules (`protect`) look at its diff. If it touches a gate,
//!   guard, budget, verdict or trust file, or weakens one anywhere (a removed
//!   assertion, a raised limit, a broadened skip), the task waits for the owner,
//!   and the approval is for that exact diff.
//! - **Limits.** Open self-tasks, tasks per week and agent steps per period are
//!   capped for the work MASON starts for itself (agent proposals, self-filed fixes,
//!   outside issues); filing past a cap is refused with the number and the limit.
//!   The owner's own tasks are never counted or refused for how many there are.
//! - **Rollback.** A rollback starts a cooldown during which no self-task may be
//!   filed, and files one proposal (waiting for the owner) saying what failed.

use crate::config::{ProjectCfg, SelfCfg};
use crate::daemon::now_ms;
use crate::jobs::Ctx;
use crate::protect;
use crate::update::{Boots, Runtimes};
use crate::Daemon;
use mason_core::*;
use mason_landing::git;
use std::sync::Arc;
use std::time::Duration;

pub fn is_self(p: &ProjectCfg) -> bool {
    p.self_cfg.is_some()
}

pub enum Gate {
    /// Nothing stops it (or the owner approved this diff).
    Go,
    /// The task left the step while it waited.
    Gone,
}

/// The change a task's branch makes against what it lands on.
pub struct Change {
    pub files: Vec<protect::Changed>,
    pub diff: String,
    /// The git range the diff was read from, for the owner to open.
    pub range: String,
}

fn resolves(repo: &std::path::Path, r: &str) -> bool {
    git::run(
        repo,
        &["rev-parse", "--verify", "-q", &format!("{r}^{{commit}}")],
    )
    .is_ok()
}

fn is_ancestor(repo: &std::path::Path, a: &str, b: &str) -> bool {
    git::run(repo, &["merge-base", "--is-ancestor", a, b]).is_ok()
}

/// The ref to diff against: the freshest of the remote-tracking base and the local
/// base (the one whose history holds the other). A stale `origin/develop` would make
/// work that already landed look like this task's change. Never fetches.
pub fn scan_base(repo: &std::path::Path, base_ref: &str, base: &str) -> String {
    if base_ref == base || !resolves(repo, base) {
        return base_ref.to_string();
    }
    if !resolves(repo, base_ref) || is_ancestor(repo, base_ref, base) {
        return base.to_string();
    }
    base_ref.to_string()
}

/// The change `branch` adds to `base_ref` (or the local `base` if that is ahead):
/// only what the branch did since the merge-base, so files that came in by merging
/// the base into the branch do not count.
pub fn change_in(
    repo: &std::path::Path,
    base_ref: &str,
    base: &str,
    branch: &str,
) -> Result<Change, String> {
    let from = scan_base(repo, base_ref, base);
    let range = format!("{from}...{branch}");
    let names = git::run(
        repo,
        &["diff", "--name-status", "-z", "--no-renames", &range],
    )
    .map_err(|e| e.to_string())?;
    let diff = git::run(repo, &["diff", "-U0", "--no-color", &range]).map_err(|e| e.to_string())?;
    let mut files = vec![];
    let mut it = names.split('\0').filter(|x| !x.is_empty());
    while let (Some(status), Some(path)) = (it.next(), it.next()) {
        files.push(protect::Changed {
            path: path.to_string(),
            added: status == "A",
        });
    }
    Ok(Change { files, diff, range })
}

pub fn change_of(p: &ProjectCfg, branch: &str) -> Result<Change, String> {
    change_in(&p.path, &p.base_ref(), &p.base, branch)
}

/// Run by the landing job before the task joins the queue.
pub fn gate(c: &Ctx) -> Gate {
    let Some(cfg) = c.project.self_cfg.clone() else {
        return Gate::Go;
    };
    let d = &c.d;
    let ch = match change_of(&c.project, &c.branch) {
        Ok(x) => x,
        Err(e) => {
            // If it cannot be read it cannot be vouched for: it waits like a flagged change.
            return wait_for_owner(
                c,
                &format!("its change could not be read ({e})"),
                "unreadable",
            );
        }
    };
    let found = protect::inspect_changed(&ch.files, &ch.diff, &cfg.protect);
    if found.clean() {
        let _ = d.store.clear_hold(c.task.0);
        return Gate::Go;
    }
    let cmd = format!("git -C {} diff {}", c.project.path.display(), ch.range);
    wait_for_owner(c, &found.short(&cmd), &protect::diff_hash(&ch.diff))
}

fn wait_for_owner(c: &Ctx, why: &str, hash: &str) -> Gate {
    let d = &c.d;
    let hold = match d.store.put_hold(c.task.0, hash, why, now_ms()) {
        Ok(h) => h,
        Err(_) => return Gate::Gone,
    };
    if hold.approved_by.is_some() {
        return Gate::Go; // approved before (a restart, or a retry of the same diff)
    }
    let on = format!("waiting for your approval: {why}");
    let key = format!("self-hold:{}", c.task.0);
    d.landing().raise(
        c.project_id,
        &key,
        "needs_you",
        &format!("Task #{} changes MASON's own rules and waits for your approval: {why}. Approve with `mason task approve {}`.", c.task.0, c.task.0),
    );
    d.poke();
    let waited = c.emit(EventKind::WaitStarted { task: c.task, on });
    let result = loop {
        std::thread::sleep(Duration::from_millis(300));
        if !matches!(d.world(), Ok(w) if w.open_run(c.task) == Some(c.run)) {
            break Gate::Gone;
        }
        if d.store
            .hold(c.task.0)
            .ok()
            .flatten()
            .is_some_and(|h| h.approved_by.is_some() && h.diff_hash == hash)
        {
            break Gate::Go;
        }
    };
    d.landing().clear(c.project_id, &key);
    if waited && matches!(result, Gate::Go) {
        c.emit(EventKind::WaitEnded { task: c.task });
    }
    d.poke();
    result
}

/// The owner approves a held self-change. Only the owner's identity counts, as
/// for an answer; the approval is for the diff that was held.
pub fn approve(d: &Arc<Daemon>, task: TaskId, who: &str) -> Result<bool, (u16, String)> {
    let Some(hold) = d.store.hold(task.0).map_err(|e| (500, e.to_string()))? else {
        return Ok(false); // not a held task: the ordinary approve applies
    };
    if hold.approved_by.is_some() {
        return Err((
            409,
            format!(
                "task #{} was already approved by {}",
                task.0,
                hold.approved_by.unwrap_or_default()
            ),
        ));
    }
    if !crate::attention::is_owner(d, who) {
        crate::attention::intervene(
            d,
            "refused_answer",
            Some(task),
            who,
            "not the owner's identity",
            "",
        );
        return Err((403, format!("{who:?} is not the owner's identity; only the owner can approve a change to MASON's own rules")));
    }
    d.store
        .approve_hold(task.0, who, now_ms())
        .map_err(|e| (500, e.to_string()))?;
    crate::attention::intervene(d, "self_approve", Some(task), who, &hold.reasons, "");
    d.landing().wake();
    d.poke();
    Ok(true)
}

// ---- limits ---------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub open: u32,
    pub week: u32,
    pub steps: u32,
    pub cooldown_until_ms: Option<u64>,
}

const WEEK_MS: u64 = 7 * 24 * 3600 * 1000;

/// What the week's filings come to: `status.week` counts only the work MASON started for
/// itself; `yours` is what the owner filed, or cancelled before it spent anything.
/// `status.open` and `status.steps` also count only that work; `yours_open` is how many
/// of the owner's own tasks are open, shown but never limited.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Usage {
    pub status: Status,
    pub yours: u32,
    pub yours_open: u32,
}

/// The week's filings as `(counted, not_counted)`. A task counts unless the owner filed it,
/// or it was cancelled before any agent step started (it spent nothing).
pub fn count<'a>(tasks: impl Iterator<Item = &'a Task>, now: u64) -> (u32, u32) {
    let (mut counted, mut yours) = (0, 0);
    for t in tasks.filter(|t| now.saturating_sub(t.enqueued_at.0) <= WEEK_MS) {
        let unstarted = t.state == State::Canceled && t.attempts.is_empty();
        if t.trust == TrustSource::Owner || unstarted {
            yours += 1;
        } else {
            counted += 1;
        }
    }
    (counted, yours)
}

pub fn status(d: &Daemon, pid: ProjectId, cfg: &SelfCfg, now: u64) -> Status {
    usage(d, pid, cfg, now).status
}

pub fn usage(d: &Daemon, pid: ProjectId, cfg: &SelfCfg, now: u64) -> Usage {
    let w = d.world().ok();
    let (mut open, mut yours_open, mut week, mut yours, mut steps) = (0, 0, 0, 0, 0);
    if let Some(w) = &w {
        let is_open =
            |t: &&Task| !matches!(t.state, State::Landed | State::Failed | State::Canceled);
        let mine = |t: &&Task| t.project == pid;
        open = w
            .tasks
            .values()
            .filter(mine)
            .filter(is_open)
            .filter(|t| t.trust != TrustSource::Owner)
            .count() as u32;
        yours_open = w
            .tasks
            .values()
            .filter(mine)
            .filter(is_open)
            .filter(|t| t.trust == TrustSource::Owner)
            .count() as u32;
        (week, yours) = count(w.tasks.values().filter(|t| t.project == pid), now);
        let period = cfg.period_h * 3600 * 1000;
        steps = w
            .step_runs
            .values()
            .filter(|r| {
                w.tasks
                    .get(&r.task)
                    .is_some_and(|t| t.project == pid && t.trust != TrustSource::Owner)
            })
            .filter(|r| matches!(r.step, State::Exploring | State::Developing | State::Review))
            .filter(|r| now.saturating_sub(r.started_at.0) <= period)
            .count() as u32;
    }
    let cooldown_until_ms = Runtimes::new(&d.cfg.data_dir)
        .rollback()
        .map(|(at, ..)| at + cfg.cooldown_h * 3600 * 1000)
        .filter(|until| *until > now);
    Usage {
        status: Status {
            open,
            week,
            steps,
            cooldown_until_ms,
        },
        yours,
        yours_open,
    }
}

/// May a task be filed in this project now? Not for MASON's own repository past a cap
/// or during a cooldown. (Other projects have no such limits.) Strict: the weekly cap
/// applies whoever files; see `check_filing_as`.
pub fn check_filing(d: &Daemon, pid: ProjectId) -> Result<(), String> {
    let Some(p) = d.project(pid) else {
        return Ok(());
    };
    let Some(cfg) = &p.self_cfg else {
        return Ok(());
    };
    check(&status(d, pid, cfg, now_ms()), cfg)
}

/// As `check_filing`, for a task of this trust: the owner's own filings are held to no
/// count at all (open, weekly or steps): those caps bound the work MASON starts for
/// itself. Only the cooldown after a rollback still stops them.
pub fn check_filing_as(d: &Daemon, pid: ProjectId, trust: TrustSource) -> Result<(), String> {
    let Some(p) = d.project(pid) else {
        return Ok(());
    };
    let Some(cfg) = &p.self_cfg else {
        return Ok(());
    };
    let u = usage(d, pid, cfg, now_ms());
    check_as(&u.status, u.yours, cfg, trust)
}

pub fn check(s: &Status, cfg: &SelfCfg) -> Result<(), String> {
    check_as(s, 0, cfg, TrustSource::OwnerAgent)
}

pub fn check_as(s: &Status, yours: u32, cfg: &SelfCfg, trust: TrustSource) -> Result<(), String> {
    if let Some(until) = s.cooldown_until_ms {
        let hours = until.saturating_sub(now_ms()).div_ceil(3_600_000);
        return Err(format!("self-tasks are cooling down after a rollback, for about {hours} more hour(s); look at the proposal it filed"));
    }
    if trust != TrustSource::Owner && s.open >= cfg.max_open {
        return Err(format!(
            "{} self-tasks are open and the limit is {}: let some land first",
            s.open, cfg.max_open
        ));
    }
    if trust != TrustSource::Owner && s.week >= cfg.per_week {
        return Err(format!(
            "{} self-tasks were filed this week and the limit is {}: {} of the {} are agent or outside tasks, which count; {} of yours do not",
            s.week,
            cfg.per_week,
            s.week,
            s.week + yours,
            yours
        ));
    }
    if trust != TrustSource::Owner && s.steps >= cfg.max_steps {
        return Err(format!(
            "self-tasks have started {} agent steps in the last {} hours and the limit is {}",
            s.steps, cfg.period_h, cfg.max_steps
        ));
    }
    Ok(())
}

/// At start: if the last rollback has not been looked into, file one proposal in each
/// of MASON's own projects, waiting for the owner, with what is known.
pub fn on_start(d: &Arc<Daemon>) {
    let rt = Runtimes::new(&d.cfg.data_dir);
    let Some((at, from, to, filed)) = rt.rollback() else {
        return;
    };
    if filed {
        return;
    }
    let boots: Vec<String> = Boots::new(&rt)
        .all()
        .into_iter()
        .filter(|b| b.sha == from)
        .rev()
        .take(8)
        .map(|b| format!("{} {} {}", b.at_ms, &b.sha[..b.sha.len().min(12)], b.event))
        .collect();
    let mut any = false;
    for (pid, p) in &d.projects {
        if !is_self(p) {
            continue;
        }
        any = true;
        let _ = d.emit(EventKind::TaskFiled {
            project: *pid,
            title: format!("Investigate the rollback from {}", &from[..from.len().min(12)]),
            body: format!(
                "The update to {from} crashed three times in five minutes and the daemon rolled back to {to} (at {at} ms).\\n\\nStarts of that build, newest first:\\n{}\\n\\nFind out why it crashed and fix it. Self-tasks are on hold until the cooldown ends.",
                boots.join("\\n")
            ),
            priority: Priority::High,
            trust: TrustSource::Owner,
            explored: false,
            proposed: true,
            blockers: vec![],
            not_before: None,
        });
    }
    if any {
        rt.mark_rollback_filed();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> SelfCfg {
        SelfCfg {
            protect: vec![],
            max_open: 3,
            per_week: 10,
            max_steps: 40,
            period_h: 24,
            cooldown_h: 24,
        }
    }

    fn st(open: u32, week: u32, steps: u32, cooldown: Option<u64>) -> Status {
        Status {
            open,
            week,
            steps,
            cooldown_until_ms: cooldown,
        }
    }

    #[test]
    fn each_limit_refuses_with_its_number_and_its_limit() {
        let c = cfg();
        assert!(check(&st(2, 9, 39, None), &c).is_ok());
        let e = check(&st(3, 0, 0, None), &c).unwrap_err();
        assert!(
            e.contains("3 self-tasks are open") && e.contains("limit is 3"),
            "{e}"
        );
        let e = check(&st(0, 10, 0, None), &c).unwrap_err();
        assert!(
            e.contains("10 self-tasks were filed this week") && e.contains("limit is 10"),
            "{e}"
        );
        let e = check(&st(0, 0, 40, None), &c).unwrap_err();
        assert!(
            e.contains("40 agent steps in the last 24 hours") && e.contains("limit is 40"),
            "{e}"
        );
    }

    #[test]
    fn a_cooldown_comes_first_and_says_how_long() {
        let until = now_ms() + 5 * 3_600_000 - 1000;
        let e = check(&st(99, 99, 99, Some(until)), &cfg()).unwrap_err();
        assert!(
            e.contains("cooling down after a rollback") && e.contains("5 more hour"),
            "{e}"
        );
    }

    fn task(trust: TrustSource, state: State, started: bool, at: u64) -> Task {
        let mut t = Task {
            id: TaskId(0),
            project: ProjectId(0),
            title: String::new(),
            body: String::new(),
            priority: Priority::Normal,
            trust,
            enqueued_at: Timestamp(at),
            state,
            activity: Activity::Idle,
            landed: None,
            explored: false,
            blockers: Default::default(),
            not_before: None,
            paused: false,
            failures: 0,
            attempts: Default::default(),
            decision: None,
            cancel_keeps_branch: true,
            failure: None,
            def: String::new(),
            step: String::new(),
            step_attempts: Default::default(),
            route: None,
            explore_returns: 0,
            approved: false,
            force_explore: false,
        };
        if started {
            t.attempts.insert(State::Developing, 1);
        }
        t
    }

    #[test]
    fn only_work_masons_starts_for_itself_counts() {
        let now = 10 * WEEK_MS;
        let ts = [
            task(TrustSource::Owner, State::Queued, false, now),
            task(TrustSource::Owner, State::Landed, true, now),
            task(TrustSource::OwnerAgent, State::Queued, false, now),
            task(TrustSource::Outside, State::Queued, false, now),
            task(TrustSource::Outside, State::Canceled, false, now),
            task(TrustSource::OwnerAgent, State::Canceled, true, now),
            task(
                TrustSource::OwnerAgent,
                State::Queued,
                false,
                now - WEEK_MS - 1,
            ),
        ];
        assert_eq!(count(ts.iter(), now), (3, 3));
    }

    #[test]
    fn the_limit_refuses_at_ten_counted_but_not_the_owner() {
        let c = cfg();
        let now = 10 * WEEK_MS;
        let mut ts: Vec<Task> = (0..10)
            .map(|_| task(TrustSource::OwnerAgent, State::Queued, false, now))
            .collect();
        ts.extend((0..5).map(|_| task(TrustSource::Owner, State::Queued, false, now)));
        let (week, yours) = count(ts.iter(), now);
        assert_eq!((week, yours), (10, 5));
        let s = st(0, week, 0, None);
        let e = check_as(&s, yours, &c, TrustSource::OwnerAgent).unwrap_err();
        assert!(
            e.contains("10 self-tasks were filed this week")
                && e.contains("limit is 10")
                && e.contains("10 of the 15 are agent or outside tasks")
                && e.contains("5 of yours do not"),
            "{e}"
        );
        assert!(check_as(&s, yours, &c, TrustSource::Outside).is_err());
        assert!(check_as(&s, yours, &c, TrustSource::Owner).is_ok());
        // No count stops the owner; the same counts still stop everyone else.
        assert!(check_as(&st(99, 99, 99, None), 0, &c, TrustSource::Owner).is_ok());
        for trust in [TrustSource::OwnerAgent, TrustSource::Outside] {
            assert!(check_as(&st(3, 0, 0, None), 0, &c, trust).is_err());
            assert!(check_as(&st(0, 0, 40, None), 0, &c, trust).is_err());
        }
        // A cooldown after a rollback is not a count: it stops the owner too.
        let until = now_ms() + 3_600_000;
        assert!(check_as(&st(0, 0, 0, Some(until)), 0, &c, TrustSource::Owner).is_err());
    }
}
