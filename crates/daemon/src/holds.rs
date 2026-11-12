// SPDX-License-Identifier: GPL-3.0-or-later

//! Account-wide holds (DESIGN Part 6): "A usage limit or outage is a
//! provider-wide hold, never a failure: affected tasks wait with no time
//! charged, you're told once, and they resume at the reset."
//!
//! A hold lives in the store (so a restart keeps it) and is about one
//! account. While it is in force no new agent session on that account is
//! granted; sessions already running carry on. It ends at the reset the
//! provider gave, or, when there is none we can read, by a probe: one session
//! at a time, tried again on a backoff that doubles from `hold_retry` up to 30
//! minutes. Reset texts that name a zone we cannot resolve count as no reset
//! (decision 0049).
//!
//! Kinds: `limit` (a usage limit), `outage` (the provider is down), `credentials`
//! (the agent cannot authenticate; probed like an outage, raises the `credentials`
//! alert, and `mason account release` clears it at once), `overage`
//! (the plan's own limit is reached while extra billing would carry on and the
//! owner chose `hold`), `budget` (an api account reached `monthly_budget_usd`)
//! and `manual` (the owner held it). `budget` and `manual` end only when the
//! owner releases the account (or, for a manual hold given a duration, when it
//! runs out).

use crate::attention::intervene;
use crate::daemon::{now_ms, Daemon};
use crate::providers::{Account, Billing, Overage as OveragePolicy};
use mason_adapter_api::{OverageState, UsageSnapshot, Window, WindowState};
use mason_adapters::runner::HoldReason;
use mason_core::reset_text::{self, ResetAt};
use mason_core::usage_text as words;
use mason_store::AccountHold;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The longest wait between probes.
pub const BACKOFF_CAP_MS: u64 = 30 * 60 * 1000;
/// A probe session that has neither ended nor been released by now is forgotten.
const PROBE_STALE: Duration = Duration::from_secs(3600);

/// Who is trying an account again, and since when.
#[derive(Default)]
pub struct Probes(Mutex<HashMap<String, (u64, Instant)>>);

impl Probes {
    fn claim(&self, account: &str, task: u64) -> bool {
        let mut m = self.0.lock().unwrap_or_else(|e| e.into_inner());
        match m.get(account) {
            Some((t, at)) if *t != task && at.elapsed() < PROBE_STALE => false,
            _ => {
                m.insert(account.to_string(), (task, Instant::now()));
                true
            }
        }
    }

    fn owner(&self, account: &str) -> Option<u64> {
        let m = self.0.lock().unwrap_or_else(|e| e.into_inner());
        m.get(account)
            .filter(|(_, at)| at.elapsed() < PROBE_STALE)
            .map(|(t, _)| *t)
    }

    fn release(&self, account: &str, task: u64) {
        let mut m = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if m.get(account).is_some_and(|(t, _)| *t == task) {
            m.remove(account);
        }
    }

    fn release_task(&self, task: u64) {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|_, (t, _)| *t != task);
    }

    fn clear(&self, account: &str) {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(account);
    }

    /// How many probes are in flight (tests).
    pub fn in_flight(&self) -> usize {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
}

/// What the gate says to a task about to start a session.
#[derive(Debug, PartialEq, Eq)]
pub enum Gate {
    Open,
    /// Wait; this is what to show.
    Wait(String),
    /// Go, as the one probe session for the account.
    Probe,
}

// ---- pure parts ----

/// The next wait between probes: `base` the first time, then double, to the cap.
pub fn next_backoff(prev_ms: u64, base_ms: u64) -> u64 {
    let base = base_ms.max(1);
    if prev_ms == 0 {
        base.min(BACKOFF_CAP_MS.max(base))
    } else {
        prev_ms.saturating_mul(2).min(BACKOFF_CAP_MS.max(base))
    }
}

/// "Tue 09:00 UTC".
pub fn when(at_secs: u64) -> String {
    const DAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    let day = DAYS[((at_secs / 86400) % 7) as usize];
    let m = (at_secs % 86400) / 60;
    format!("{day} {:02}:{:02} UTC", m / 60, m % 60)
}

/// What the readings in one window say will happen at the recent rate.
/// `points` are `(at, utilization)` of that window, oldest first. `None` unless
/// utilization was reported at two different times.
pub fn projection(points: &[(u64, f64)], resets_at: u64, now: u64) -> Option<String> {
    if points.len() < 2 {
        return None;
    }
    let (a, b) = (points[points.len() - 2], points[points.len() - 1]);
    if b.0 <= a.0 {
        return None;
    }
    let rate = (b.1 - a.1) / (b.0 - a.0) as f64;
    if rate <= 0.0 {
        return Some("it is not rising at the recent rate".into());
    }
    let left = ((1.0 - b.1).max(0.0) / rate) as u64;
    let hit = b.0 + left;
    if hit >= resets_at {
        Some("at the recent rate it will not reach the limit before the reset".into())
    } else {
        Some(format!(
            "at the recent rate it reaches the limit in {}, before the reset",
            words::span(hit.saturating_sub(now))
        ))
    }
}

fn label_of(kind: &str) -> &'static str {
    match kind {
        "outage" => "provider outage",
        "credentials" => "not logged in",
        "overage" => "plan limit reached (extra billing held)",
        "budget" => "monthly budget reached",
        "manual" => "held by the owner",
        _ => "usage limit",
    }
}

/// Why and until when, as one phrase: "weekly limit, resets Tue 09:00 UTC (in 2 d 3 h)".
pub fn describe(h: &AccountHold, now_ms: u64) -> String {
    describe_with(h, now_ms, None)
}

/// [`describe`], told which task (if any) is running the account's probe now.
pub fn describe_with(h: &AccountHold, now_ms: u64, probe_owner: Option<u64>) -> String {
    let now = now_ms / 1000;
    let why = if h.why.is_empty() {
        label_of(&h.kind)
    } else {
        &h.why
    };
    match h.until_ms {
        Some(u) if u / 1000 > now => format!(
            "{why}, resets {} (in {})",
            when(u / 1000),
            words::span(u / 1000 - now)
        ),
        Some(_) => format!("{why}, reset due"),
        None if h.kind == "budget" => format!(
            "{why}; raise the budget or `mason account release {}`",
            h.account
        ),
        None if h.kind == "manual" => {
            format!("{why}; `mason account release {}` lifts it", h.account)
        }
        None => {
            let next = h.probe_ms.saturating_sub(now_ms) / 1000;
            if h.kind == "credentials" {
                match probe_owner {
                    Some(t) => format!(
                        "{why}; checking now with task #{t}; `mason account release {}` lifts it now",
                        h.account
                    ),
                    None => format!(
                        "{why}; logging in (`claude /login`) does not lift it: the hold lifts at the next successful check (in {}), or `mason account release {}` now",
                        words::span(next),
                        h.account
                    ),
                }
            } else if h.kind == "outage" {
                format!("{why}, trying again in {}", words::span(next))
            } else {
                format!(
                    "{why}, reset time unknown, trying again in {}",
                    words::span(next)
                )
            }
        }
    }
}

/// The text a waiting task shows.
pub fn waiting_text(h: &AccountHold, now_ms: u64, probe_owner: Option<u64>) -> String {
    format!(
        "waiting for {}: {}",
        h.account,
        describe_with(h, now_ms, probe_owner)
    )
}

fn alert_kind(kind: &str) -> &'static str {
    match kind {
        "outage" => "provider_outage",
        "credentials" => "credentials",
        "overage" => "provider_overage",
        "budget" => "provider_budget",
        "manual" => "provider_hold",
        _ => "provider_limit",
    }
}

// ---- the store, with the account's own rules ----

pub fn hold_of(d: &Daemon, account: &str) -> Option<AccountHold> {
    d.store
        .account_holds()
        .unwrap_or_default()
        .into_iter()
        .find(|h| h.account == account)
}

fn account_of<'a>(d: &'a Daemon, name: &str) -> Option<&'a Account> {
    d.accounts.iter().find(|a| a.name == name)
}

fn changed(d: &Daemon) {
    crate::attention::sync_alerts(d);
    d.poke();
}

/// Hold the account, or extend what is there. `prober`: the session reporting
/// this is the account's probe, so an unreadable reset means "still refused":
/// the backoff doubles.
fn put(
    d: &Daemon,
    account: &str,
    kind: &str,
    why: &str,
    until_ms: Option<u64>,
    prober: bool,
    by: &str,
) {
    let now = now_ms();
    let base = d.cfg.hold_retry.as_millis() as u64;
    let old = hold_of(d, account);
    let new = match old {
        None => AccountHold {
            account: account.into(),
            kind: kind.into(),
            why: why.into(),
            since_ms: now,
            until_ms,
            backoff_ms: if until_ms.is_some() {
                0
            } else {
                next_backoff(0, base)
            },
            probe_ms: if until_ms.is_some() {
                0
            } else {
                now + next_backoff(0, base)
            },
            by: by.into(),
        },
        Some(e) => {
            // The owner's holds and the budget are the owner's to lift.
            if matches!(e.kind.as_str(), "manual" | "budget") && e.kind != kind {
                return;
            }
            match until_ms {
                Some(u) => AccountHold {
                    kind: kind.into(),
                    why: why.into(),
                    until_ms: Some(e.until_ms.map_or(u, |o| o.max(u))),
                    backoff_ms: 0,
                    probe_ms: 0,
                    ..e
                },
                None if prober && e.until_ms.is_none() => {
                    let b = next_backoff(e.backoff_ms, base);
                    AccountHold {
                        kind: kind.into(),
                        why: why.into(),
                        backoff_ms: b,
                        probe_ms: now + b,
                        ..e
                    }
                }
                // A running session saying it was refused changes nothing about
                // a hold already in force.
                None => return,
            }
        }
    };
    if hold_of(d, account).as_ref() == Some(&new) {
        return;
    }
    if let Err(e) = d.store.put_account_hold(new) {
        tracing::warn!(account, "cannot store the account hold: {e}");
        return;
    }
    changed(d);
}

/// A session on `account` ended in a hold: hold the account.
pub fn observe_end(d: &Daemon, account: &str, task: u64, reason: &HoldReason) {
    let prober = d.probes.owner(account) == Some(task);
    match reason {
        HoldReason::UsageLimit { resets_at } => {
            let until = resets_at
                .as_deref()
                .and_then(|t| match reset_text::parse(t, now_ms() / 1000) {
                    ResetAt::At(s) => Some(s * 1000),
                    ResetAt::Unknown => None,
                })
                // A reset already past (a stale text) is no reset.
                .filter(|u| *u > now_ms());
            put(
                d,
                account,
                "limit",
                "usage limit",
                until,
                prober,
                "pipeline",
            );
        }
        HoldReason::ProviderDown { .. } => {
            put(
                d,
                account,
                "outage",
                "provider outage",
                None,
                prober,
                "pipeline",
            );
        }
        HoldReason::NotLoggedIn { .. } => {
            put(
                d,
                account,
                "credentials",
                "the agent is not logged in",
                None,
                prober,
                "pipeline",
            );
        }
    }
}

/// A snapshot arrived: hold on what it says, warn on what it nears.
pub fn on_snapshot(d: &Daemon, account: &str, s: &UsageSnapshot) {
    let Some(a) = account_of(d, account) else {
        return;
    };
    let now = now_ms();
    if let Some(b) = &s.blocked {
        let until = b.until.map(|u| u * 1000).filter(|u| *u > now);
        // A block with a reset in the past is stale news.
        if b.until.is_none() || until.is_some() {
            let why = blocked_why(s, b);
            put(d, account, "limit", &why, until, false, "pipeline");
        }
    }
    if a.overage == OveragePolicy::Hold && s.overage.state == OverageState::InUse {
        let full: Vec<&Window> = s.windows.iter().filter(|w| at_limit(w)).collect();
        if !full.is_empty() {
            let until = full
                .iter()
                .map(|w| w.resets_at)
                .collect::<Option<Vec<_>>>()
                .and_then(|v| v.into_iter().max())
                .map(|u| u * 1000)
                .filter(|u| *u > now);
            let label = full[0].label.clone();
            put(
                d,
                account,
                "overage",
                &format!("{label} limit reached, extra billing held"),
                until,
                false,
                "pipeline",
            );
        }
    }
    warn(d, a, s);
}

fn at_limit(w: &Window) -> bool {
    w.state == WindowState::Blocked || w.utilization.is_some_and(|u| u >= 1.0)
}

fn blocked_why(s: &UsageSnapshot, b: &mason_adapter_api::Blocked) -> String {
    use mason_adapter_api::BlockedWhy;
    match &b.why {
        BlockedWhy::Window(id) => {
            let l = s
                .windows
                .iter()
                .find(|w| &w.id == id)
                .map_or(id.as_str(), |w| w.label.as_str());
            format!("{l} limit")
        }
        BlockedWhy::OverageUsedUp => "overage used up".into(),
        BlockedWhy::AccountDisabled(w) => format!("account disabled ({w})"),
        BlockedWhy::RateLimit => "rate limit".into(),
        BlockedWhy::Unreported(_) => "usage limit".into(),
    }
}

/// One warning per window per reset, once a window passes `warn_at`.
fn warn(d: &Daemon, a: &Account, s: &UsageSnapshot) {
    if a.billing != Billing::Subscription {
        return;
    }
    let now = now_ms();
    let mut any = false;
    for w in &s.windows {
        let (Some(u), Some(r)) = (w.utilization, w.resets_at) else {
            continue;
        };
        if u >= a.warn_at && !at_limit(w) && r * 1000 > now {
            if let Ok(true) = d.store.note_warning(&a.name, &w.id, r, now) {
                any = true;
            }
        }
    }
    if any {
        changed(d);
    }
}

/// Spend against the budget of an api account, after tokens were recorded.
pub fn check_budget(d: &Daemon, account: &str) {
    let Some(a) = account_of(d, account) else {
        return;
    };
    let (Billing::Api, Some(budget)) = (a.billing, a.monthly_budget_usd) else {
        return;
    };
    let now = now_ms();
    let start = crate::usage::month_start(now / 1000);
    let spent: f64 = d
        .store
        .usage_runs_since(start)
        .unwrap_or_default()
        .iter()
        .filter(|r| r.account == account)
        .filter_map(|r| r.cost_usd)
        .sum();
    let next_month = crate::usage::month_start(start + 32 * 86400);
    if spent >= budget {
        let waived = d
            .store
            .warnings()
            .unwrap_or_default()
            .iter()
            .any(|(ac, w, r)| ac == account && w == WAIVED && *r == next_month);
        if !waived {
            put(
                d,
                account,
                "budget",
                &format!("monthly budget ${budget:.2} reached (${spent:.2} spent)"),
                None,
                false,
                "pipeline",
            );
        }
    } else if spent >= a.warn_at * budget
        && matches!(
            d.store.note_warning(account, BUDGET_WARN, next_month, now),
            Ok(true)
        )
    {
        changed(d);
    }
}

const WAIVED: &str = "budget-waived";
const BUDGET_WARN: &str = "budget";

// ---- the gate ----

/// May `task` start a session on `account`? Claims the probe when it is due.
pub fn gate(d: &Daemon, account: &str, task: u64) -> Gate {
    let Some(h) = hold_of(d, account) else {
        return Gate::Open;
    };
    let now = now_ms();
    match h.until_ms {
        Some(u) if now < u => return Gate::Wait(waiting_text(&h, now, d.probes.owner(account))),
        Some(_) => {
            // The reset has come: release the account.
            if d.store.clear_account_hold(account).unwrap_or(false) {
                d.probes.clear(account);
                changed(d);
            }
            return Gate::Open;
        }
        None => {}
    }
    if matches!(h.kind.as_str(), "manual" | "budget") || now < h.probe_ms {
        return Gate::Wait(waiting_text(&h, now, d.probes.owner(account)));
    }
    if d.probes.claim(account, task) {
        Gate::Probe
    } else {
        Gate::Wait(waiting_text(&h, now, d.probes.owner(account)))
    }
}

/// The probe's session got a real answer from the provider: lift the hold now.
pub fn probe_ok(d: &Daemon, account: &str, task: u64) {
    if d.probes.owner(account) != Some(task) {
        return;
    }
    d.probes.release(account, task);
    if let Some(h) = hold_of(d, account) {
        if !matches!(h.kind.as_str(), "manual" | "budget")
            && d.store.clear_account_hold(account).unwrap_or(false)
        {
            changed(d);
        }
    }
}

/// A session that went through the gate has ended. If it was the probe and
/// the provider did not refuse it, the hold is over.
pub fn session_ended(d: &Daemon, account: &str, task: u64, refused: bool) {
    if d.probes.owner(account) != Some(task) {
        return;
    }
    d.probes.release(account, task);
    if !refused {
        if let Some(h) = hold_of(d, account) {
            if !matches!(h.kind.as_str(), "manual" | "budget")
                && d.store.clear_account_hold(account).unwrap_or(false)
            {
                changed(d);
            }
        }
    }
}

/// The task's job is over: it holds no probe.
pub fn task_done(d: &Daemon, task: u64) {
    d.probes.release_task(task);
}

// ---- the owner ----

pub fn release(d: &Daemon, account: &str, who: &str) -> Result<bool, String> {
    let a = account_of(d, account).ok_or_else(|| format!("no account {account:?}"))?;
    let Some(h) = hold_of(d, account) else {
        return Ok(false);
    };
    let _ = d.store.clear_account_hold(account);
    d.probes.clear(account);
    if h.kind == "budget" && a.monthly_budget_usd.is_some() {
        // Released with the spend still over: do not hold again this month.
        let next =
            crate::usage::month_start(crate::usage::month_start(now_ms() / 1000) + 32 * 86400);
        let _ = d.store.note_warning(account, WAIVED, next, now_ms());
    }
    intervene(d, "account_release", None, who, account, &h.kind);
    changed(d);
    Ok(true)
}

pub fn hold(
    d: &Daemon,
    account: &str,
    who: &str,
    for_secs: Option<u64>,
    reason: &str,
) -> Result<(), String> {
    account_of(d, account).ok_or_else(|| format!("no account {account:?}"))?;
    let now = now_ms();
    let why = if reason.is_empty() {
        format!("held by {who}")
    } else {
        format!("held by {who}: {reason}")
    };
    d.store
        .put_account_hold(AccountHold {
            account: account.into(),
            kind: "manual".into(),
            why,
            since_ms: now,
            until_ms: for_secs.map(|s| now + s * 1000),
            backoff_ms: 0,
            probe_ms: 0,
            by: who.into(),
        })
        .map_err(|e| e.to_string())?;
    intervene(d, "account_hold", None, who, account, reason);
    changed(d);
    Ok(())
}

// ---- telling the owner ----

/// The provider conditions true right now, as `(key, kind, text)`: one per
/// event, however many tasks wait.
pub fn alerts(d: &Daemon) -> Vec<(String, String, String)> {
    let mut out = vec![];
    let now = now_ms();
    for h in d.store.account_holds().unwrap_or_default() {
        if h.until_ms.is_some_and(|u| u <= now) {
            continue;
        }
        let tail = match h.kind.as_str() {
            "budget" | "manual" => "waiting tasks start no sessions until you release it",
            _ => "waiting tasks start no sessions until then",
        };
        out.push((
            format!("account-hold:{}", h.account),
            alert_kind(&h.kind).into(),
            format!(
                "{}: {}; {tail}",
                h.account,
                describe_with(&h, now, d.probes.owner(&h.account))
            ),
        ));
    }
    let latest: HashMap<String, UsageSnapshot> = d
        .store
        .usage_latest()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(a, _, j)| {
            let v = serde_json::from_str(&j).ok()?;
            Some((a, crate::usage::snapshot_from_json(&v)?))
        })
        .collect();
    for a in &d.accounts {
        if let Some(s) = latest.get(&a.name) {
            if a.overage == OveragePolicy::Allow && s.overage.state == OverageState::InUse {
                let cost = s
                    .spend_usd
                    .map_or(String::new(), |c| format!(", cost ${c:.2} so far"));
                out.push((
                    format!("extra-billing:{}", a.name),
                    "provider_extra_billing".into(),
                    format!(
                        "{}: working on extra billing (overage allowed){cost}",
                        a.name
                    ),
                ));
            }
        }
    }
    let readings = d.store.readings_since(0).unwrap_or_default();
    for (account, win, r) in d.store.warnings().unwrap_or_default() {
        if r * 1000 <= now || win == WAIVED {
            continue;
        }
        let Some(a) = account_of(d, &account) else {
            continue;
        };
        let text = if win == BUDGET_WARN {
            let spent = spent_this_month(d, &account);
            format!(
                "{account}: spend ${spent:.2} is past {:.0}% of the ${:.2} monthly budget; at 100% the account is held",
                a.warn_at * 100.0,
                a.monthly_budget_usd.unwrap_or(0.0)
            )
        } else {
            let w = latest.get(&account).and_then(|s| {
                s.windows
                    .iter()
                    .find(|w| w.id == win && w.resets_at == Some(r))
            });
            let Some(w) = w else { continue };
            let pts: Vec<(u64, f64)> = readings
                .iter()
                .filter(|x| x.account == account && x.window == win && x.resets_at == Some(r))
                .filter_map(|x| x.utilization.map(|u| (x.at, u)))
                .collect();
            let mut t = format!(
                "{account}: {} limit at {}, resets {}",
                w.label.to_lowercase(),
                words::percent(w.utilization.unwrap_or(0.0)),
                when(r)
            );
            if let Some(p) = projection(&pts, r, now / 1000) {
                t += &format!("; {p}");
            }
            t
        };
        out.push((
            format!("usage-warn:{account}:{win}:{r}"),
            "provider_warning".into(),
            text,
        ));
    }
    out
}

fn spent_this_month(d: &Daemon, account: &str) -> f64 {
    d.store
        .usage_runs_since(crate::usage::month_start(now_ms() / 1000))
        .unwrap_or_default()
        .iter()
        .filter(|r| r.account == account)
        .filter_map(|r| r.cost_usd)
        .sum()
}

/// The banners: the same events, one each.
pub fn banners(d: &Daemon) -> Vec<(String, String)> {
    alerts(d).into_iter().map(|(_, k, t)| (k, t)).collect()
}

/// What MASON is doing about an account, for the usage page.
pub fn doing(d: &Daemon, account: &str) -> String {
    let now = now_ms();
    match hold_of(d, account) {
        Some(h) => format!(
            "holding new sessions: {}",
            describe_with(&h, now, d.probes.owner(account))
        ),
        None => "nothing".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_starts_at_the_base_doubles_and_caps() {
        let base = 60_000;
        let mut b = next_backoff(0, base);
        assert_eq!(b, 60_000);
        let mut seen = vec![b];
        for _ in 0..8 {
            b = next_backoff(b, base);
            seen.push(b);
        }
        assert_eq!(&seen[..4], &[60_000, 120_000, 240_000, 480_000]);
        assert_eq!(*seen.last().unwrap(), BACKOFF_CAP_MS);
        assert!(seen.windows(2).all(|w| w[1] >= w[0]));
    }

    #[test]
    fn projection_needs_two_readings() {
        assert_eq!(projection(&[], 10_000, 0), None);
        assert_eq!(projection(&[(0, 0.5)], 10_000, 0), None);
        let p = projection(&[(0, 0.5), (3600, 0.6)], 1_000_000, 3600).unwrap();
        assert!(p.contains("reaches the limit in 4 h"), "{p}");
        let q = projection(&[(0, 0.5), (3600, 0.51)], 20_000, 3600).unwrap();
        assert!(q.contains("not reach the limit before the reset"), "{q}");
        let r = projection(&[(0, 0.5), (3600, 0.5)], 20_000, 3600).unwrap();
        assert!(r.contains("not rising"), "{r}");
    }

    #[test]
    fn when_names_the_weekday() {
        // 2026-11-07 is a Saturday.
        assert_eq!(when(1_794_052_800), "Sat 12:00 UTC");
    }

    #[test]
    fn a_waiting_text_says_why_and_until_when() {
        let h = AccountHold {
            account: "claude".into(),
            kind: "limit".into(),
            why: "weekly limit".into(),
            since_ms: 0,
            until_ms: Some(1_794_052_800_000 + 2 * 86_400_000 + 3 * 3_600_000),
            backoff_ms: 0,
            probe_ms: 0,
            by: "pipeline".into(),
        };
        assert_eq!(
            waiting_text(&h, 1_794_052_800_000, None),
            "waiting for claude: weekly limit, resets Mon 15:00 UTC (in 2 d 3 h)"
        );
    }

    fn creds() -> AccountHold {
        AccountHold {
            account: "claude".into(),
            kind: "credentials".into(),
            why: "not logged in".into(),
            since_ms: 0,
            until_ms: None,
            backoff_ms: 60_000,
            probe_ms: 120_000,
            by: "pipeline".into(),
        }
    }

    #[test]
    fn a_credentials_hold_says_a_running_probe_is_checking_now() {
        let t = describe_with(&creds(), 130_000, Some(190));
        assert!(t.contains("checking now with task #190"), "{t}");
        assert!(!t.contains("next check in"), "{t}");
    }

    #[test]
    fn a_credentials_hold_says_the_login_alone_does_not_lift_it() {
        let t = describe_with(&creds(), 60_000, None);
        assert!(
            t.contains("lifts at the next successful check (in 1 min"),
            "{t}"
        );
        assert!(t.contains("`mason account release claude` now"), "{t}");
        assert!(t.contains("does not lift it"), "{t}");
    }

    #[test]
    fn a_credentials_hold_with_a_due_probe_nobody_has_claimed_names_the_check() {
        let t = describe_with(&creds(), 130_000, None);
        assert!(t.contains("next successful check"), "{t}");
    }
}
