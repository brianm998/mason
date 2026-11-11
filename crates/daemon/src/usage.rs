// SPDX-License-Identifier: GPL-3.0-or-later

//! LLM usage: what each provider account last reported (kept across a
//! restart), the readings that make the trend, and the tokens and cost of
//! each step attempt. Only reported figures are stored and shown; a provider
//! that reports nothing shows nothing, never zero. Every figure is shown with
//! its age (decision 0045).

use crate::daemon::{now_ms, Daemon};
use mason_adapter_api::{
    AgentAdapter, BillingKind, Blocked, BlockedWhy, Overage, OverageState, UsageSnapshot, Window,
    WindowState,
};
use mason_core::usage_text as words;
use mason_protocol::v1::{
    ProviderStatus, SpendView, TokenLine, UsageTokens, UsageView, UsageWindow,
};
use mason_store::{UsageReading, UsageRun};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

const DAY: u64 = 86400;

pub fn now_secs() -> u64 {
    now_ms() / 1000
}

// ---- the snapshot as JSON, for the store ----

fn state_name(s: WindowState) -> &'static str {
    match s {
        WindowState::Ok => "ok",
        WindowState::Warning => "warning",
        WindowState::Blocked => "blocked",
    }
}

fn state_from(s: &str) -> WindowState {
    match s {
        "warning" => WindowState::Warning,
        "blocked" => WindowState::Blocked,
        _ => WindowState::Ok,
    }
}

pub fn snapshot_json(s: &UsageSnapshot) -> Value {
    let (ostate, owhy) = match &s.overage.state {
        OverageState::NotOffered => ("not_offered", String::new()),
        OverageState::Off => ("off", String::new()),
        OverageState::Disabled(w) => ("disabled", w.clone()),
        OverageState::InUse => ("in_use", String::new()),
        OverageState::UsedUp => ("used_up", String::new()),
    };
    let blocked = s.blocked.as_ref().map(|b| {
        let (kind, detail) = match &b.why {
            BlockedWhy::Window(w) => ("window", w.clone()),
            BlockedWhy::OverageUsedUp => ("overage_used_up", String::new()),
            BlockedWhy::AccountDisabled(w) => ("account_disabled", w.clone()),
            BlockedWhy::RateLimit => ("rate_limit", String::new()),
            BlockedWhy::Unreported(w) => ("unreported", w.clone()),
        };
        json!({"kind": kind, "detail": detail, "until": b.until})
    });
    json!({
        "windows": s.windows.iter().map(|w| json!({
            "id": w.id, "label": w.label, "length_secs": w.length_secs,
            "utilization": w.utilization, "resets_at": w.resets_at,
            "state": state_name(w.state),
        })).collect::<Vec<_>>(),
        "overage": {"state": ostate, "why": owhy, "resets_at": s.overage.resets_at},
        "blocked": blocked,
        "spend_usd": s.spend_usd,
        "observed_at": s.observed_at,
        "session_id": s.session_id,
    })
}

pub fn snapshot_from_json(v: &Value) -> Option<UsageSnapshot> {
    let windows = v["windows"]
        .as_array()?
        .iter()
        .map(|w| Window {
            id: w["id"].as_str().unwrap_or_default().into(),
            label: w["label"].as_str().unwrap_or_default().into(),
            length_secs: w["length_secs"].as_u64(),
            utilization: w["utilization"].as_f64(),
            resets_at: w["resets_at"].as_u64(),
            state: state_from(w["state"].as_str().unwrap_or("ok")),
        })
        .collect();
    let o = &v["overage"];
    let why = o["why"].as_str().unwrap_or_default().to_string();
    let state = match o["state"].as_str().unwrap_or("not_offered") {
        "off" => OverageState::Off,
        "disabled" => OverageState::Disabled(why),
        "in_use" => OverageState::InUse,
        "used_up" => OverageState::UsedUp,
        _ => OverageState::NotOffered,
    };
    let blocked = v["blocked"].as_object().map(|b| {
        let detail = b["detail"].as_str().unwrap_or_default().to_string();
        let why = match b["kind"].as_str().unwrap_or_default() {
            "window" => BlockedWhy::Window(detail),
            "overage_used_up" => BlockedWhy::OverageUsedUp,
            "account_disabled" => BlockedWhy::AccountDisabled(detail),
            "rate_limit" => BlockedWhy::RateLimit,
            _ => BlockedWhy::Unreported(detail),
        };
        Blocked {
            why,
            until: b["until"].as_u64(),
        }
    });
    Some(UsageSnapshot {
        windows,
        overage: Overage {
            state,
            resets_at: o["resets_at"].as_u64(),
        },
        blocked,
        spend_usd: v["spend_usd"].as_f64(),
        observed_at: v["observed_at"].as_u64(),
        session_id: v["session_id"].as_str().map(String::from),
    })
}

// ---- recording ----

/// Keep a snapshot as the account's latest and its windows as readings.
pub fn record_snapshot(d: &Daemon, account: &str, s: &UsageSnapshot) {
    let at = s.observed_at.unwrap_or_else(now_secs);
    let mut s = s.clone();
    s.observed_at = Some(at);
    if let Err(e) = d
        .store
        .put_usage_latest(account, at, &snapshot_json(&s).to_string())
    {
        tracing::warn!(account, "cannot store the usage snapshot: {e}");
        return;
    }
    for w in &s.windows {
        let r = UsageReading {
            account: account.into(),
            window: w.id.clone(),
            at,
            utilization: w.utilization,
            resets_at: w.resets_at,
            state: state_name(w.state).into(),
        };
        if let Err(e) = d.store.record_reading(r) {
            tracing::warn!(account, "cannot store a usage reading: {e}");
        }
    }
    // What MASON does with it: hold on a block, warn on a window nearing its limit.
    crate::holds::on_snapshot(d, account, &s);
}

/// Who an attempt's figures belong to.
pub struct Attempt<'a> {
    pub key: &'a str,
    pub task: u64,
    pub project: &'a str,
    pub step: &'a str,
    pub account: &'a str,
    pub model: Option<&'a str>,
}

/// Keep what one step attempt reported. Nothing reported: no row.
pub fn record_tokens(d: &Daemon, who: &Attempt, t: &mason_adapters::runner::TokenTotals) {
    if t.input.is_none()
        && t.output.is_none()
        && t.cache_read.is_none()
        && t.cache_create.is_none()
        && t.cost_usd.is_none()
    {
        return;
    }
    let row = UsageRun {
        attempt: who.key.into(),
        task: who.task,
        project: who.project.into(),
        step: who.step.into(),
        account: who.account.into(),
        model: who.model.map(String::from),
        input: t.input,
        output: t.output,
        cache_read: t.cache_read,
        cache_create: t.cache_create,
        cost_usd: t.cost_usd,
        at: now_secs(),
    };
    if let Err(e) = d.store.record_usage_run(row) {
        tracing::warn!(task = who.task, "cannot store the step's tokens: {e}");
    }
    // An api account's spend may have reached its budget.
    crate::holds::check_budget(d, who.account);
}

// ---- the view ----

#[derive(Default, Clone)]
struct Agg {
    input: Option<u64>,
    output: Option<u64>,
    cache_read: Option<u64>,
    cache_write: Option<u64>,
    cost: Option<f64>,
    attempts: u32,
    newest: u64,
}

fn plus(a: &mut Option<u64>, b: Option<u64>) {
    if let Some(b) = b {
        *a = Some(a.unwrap_or(0) + b);
    }
}

impl Agg {
    fn add(&mut self, r: &UsageRun) {
        plus(&mut self.input, r.input);
        plus(&mut self.output, r.output);
        plus(&mut self.cache_read, r.cache_read);
        plus(&mut self.cache_write, r.cache_create);
        if let Some(c) = r.cost_usd {
            self.cost = Some(self.cost.unwrap_or(0.0) + c);
        }
        self.attempts += 1;
        self.newest = self.newest.max(r.at);
    }

    fn line(&self, name: &str, now: u64, task: u64) -> TokenLine {
        TokenLine {
            name: name.into(),
            has_input: self.input.is_some(),
            input: self.input.unwrap_or(0),
            has_output: self.output.is_some(),
            output: self.output.unwrap_or(0),
            has_cache_read: self.cache_read.is_some(),
            cache_read: self.cache_read.unwrap_or(0),
            has_cache_write: self.cache_write.is_some(),
            cache_write: self.cache_write.unwrap_or(0),
            has_cost: self.cost.is_some(),
            cost_usd: self.cost.unwrap_or(0.0),
            attempts: self.attempts,
            age: words::age(now, self.newest),
            task,
        }
    }
}

fn tokens(
    rows: &[&UsageRun],
    period: &str,
    now: u64,
    title: &dyn Fn(u64) -> String,
) -> UsageTokens {
    let mut by_project: BTreeMap<String, Agg> = BTreeMap::new();
    let mut by_model: BTreeMap<String, Agg> = BTreeMap::new();
    let mut by_task: BTreeMap<u64, Agg> = BTreeMap::new();
    for r in rows {
        by_project.entry(r.project.clone()).or_default().add(r);
        let m = r
            .model
            .clone()
            .unwrap_or_else(|| "(model not reported)".into());
        by_model.entry(m).or_default().add(r);
        by_task.entry(r.task).or_default().add(r);
    }
    let mut tasks: Vec<(u64, Agg)> = by_task.into_iter().collect();
    // Most expensive first; tokens when no cost was reported.
    let weight = |a: &Agg| {
        a.cost.unwrap_or(0.0) * 1e6 + (a.input.unwrap_or(0) + a.output.unwrap_or(0)) as f64
    };
    tasks.sort_by(|a, b| weight(&b.1).total_cmp(&weight(&a.1)).then(a.0.cmp(&b.0)));
    UsageTokens {
        period: period.into(),
        by_project: by_project.iter().map(|(k, a)| a.line(k, now, 0)).collect(),
        by_model: by_model.iter().map(|(k, a)| a.line(k, now, 0)).collect(),
        top_tasks: tasks
            .iter()
            .take(5)
            .map(|(t, a)| a.line(&format!("#{t} {}", title(*t)), now, *t))
            .collect(),
    }
}

/// On a plan the stream's cost figure is notional, not money spent (decision
/// 0043): it is kept in the store but only an `api` account shows it.
fn without_notional_cost(mut t: UsageTokens, api: bool) -> UsageTokens {
    if !api {
        for l in t
            .by_project
            .iter_mut()
            .chain(t.by_model.iter_mut())
            .chain(t.top_tasks.iter_mut())
        {
            l.has_cost = false;
            l.cost_usd = 0.0;
        }
    }
    t
}

/// The adapter's own model of what it can report, for an account.
fn model_of(a: &crate::providers::Account) -> mason_adapter_api::UsageModel {
    let billing = billing_kind(a);
    match a.adapter {
        crate::config::Provider::Claude => {
            mason_adapters::claude::ClaudeAdapter::new("claude", "", "").usage_model(billing)
        }
        crate::config::Provider::Codex => {
            mason_adapters::codex::CodexAdapter::new("codex", "", "").usage_model(billing)
        }
    }
}

pub fn billing_kind(a: &crate::providers::Account) -> BillingKind {
    match a.billing {
        crate::providers::Billing::Subscription => BillingKind::Subscription,
        crate::providers::Billing::Api => BillingKind::Api,
    }
}

/// Days since 1970-01-01 to (year, month, day), proleptic Gregorian.
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    ((yoe + era * 400) + i64::from(m <= 2), m, d)
}

/// Epoch seconds of 00:00 UTC on the first of `now`'s month.
pub fn month_start(now: u64) -> u64 {
    let (_, _, d) = civil((now / DAY) as i64);
    (now / DAY - u64::from(d - 1)) * DAY
}

fn window_line(
    w: &Window,
    reported: bool,
    now: u64,
    at: u64,
    trend: Option<String>,
) -> UsageWindow {
    let pct = w.utilization.filter(|_| reported);
    let resets_text = match w.resets_at {
        Some(r) if reported => words::resets(now, r),
        _ => String::new(),
    };
    let state = if reported { state_name(w.state) } else { "ok" };
    let mut short = w.label.clone();
    if let Some(p) = pct {
        short += &format!(" {}", words::percent(p));
    } else if state != "ok" {
        short += &format!(" {state}");
    }
    if let Some(r) = w.resets_at.filter(|_| reported) {
        short += &format!(", {}", words::resets_short(now, r));
    }
    UsageWindow {
        id: w.id.clone(),
        label: w.label.clone(),
        reported,
        has_percent: pct.is_some(),
        percent: pct.map_or(0.0, |p| p * 100.0),
        has_reset: reported && w.resets_at.is_some(),
        resets_at: if reported {
            w.resets_at.unwrap_or(0)
        } else {
            0
        },
        resets_text,
        state: state.into(),
        trend: trend.unwrap_or_default(),
        age: if reported {
            words::age(now, at)
        } else {
            String::new()
        },
        short_text: short,
    }
}

fn blocked_text(b: &Blocked, windows: &[Window], now: u64) -> String {
    let why = match &b.why {
        BlockedWhy::Window(id) => {
            let l = windows
                .iter()
                .find(|w| &w.id == id)
                .map_or(id.as_str(), |w| w.label.as_str());
            format!("{l} window is full")
        }
        BlockedWhy::OverageUsedUp => "overage is used up".into(),
        BlockedWhy::AccountDisabled(w) => format!("account disabled ({w})"),
        BlockedWhy::RateLimit => "rate limited".into(),
        BlockedWhy::Unreported(w) if w.is_empty() => "the provider refused and said no more".into(),
        BlockedWhy::Unreported(w) => format!("the provider refused: {w}"),
    };
    words::blocked(&why, now, b.until)
}

fn overage_text(o: &Overage, now: u64) -> String {
    match &o.state {
        OverageState::NotOffered => String::new(),
        OverageState::Off => words::overage_off(""),
        OverageState::Disabled(w) => words::overage_off(w),
        // The stream gives no start time or cost for overage in use.
        OverageState::InUse => words::overage_in_use(now, None, None),
        OverageState::UsedUp => "overage: used up".into(),
    }
}

pub fn view(d: &Daemon, now: u64, title: &dyn Fn(u64) -> String) -> UsageView {
    let latest: BTreeMap<String, (u64, UsageSnapshot)> = d
        .store
        .usage_latest()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(a, at, j)| {
            let v: Value = serde_json::from_str(&j).ok()?;
            Some((a, (at, snapshot_from_json(&v)?)))
        })
        .collect();
    let week_ago = now.saturating_sub(7 * DAY);
    let runs = d
        .store
        .usage_runs_since(week_ago.min(month_start(now)))
        .unwrap_or_default();
    let readings = d.store.readings_since(week_ago).unwrap_or_default();
    let holds = d.store.account_holds().unwrap_or_default();
    let mut providers = vec![];
    let mut top_bar = vec![];
    for a in &d.accounts {
        let llm = crate::sched::llm_resource(&a.name);
        let api = matches!(a.billing, crate::providers::Billing::Api);
        let snap = latest.get(&a.name);
        let model = model_of(a);
        let mut windows: Vec<UsageWindow> = vec![];
        let mut raw: Vec<Window> = vec![];
        if !api {
            let at = snap.map_or(0, |s| s.0);
            let trend_of = |id: &str| {
                let pts: Vec<(u64, f64)> = readings
                    .iter()
                    .filter(|r| r.account == a.name && r.window == id)
                    .filter_map(|r| r.utilization.map(|u| (r.at, u)))
                    .collect();
                words::trend(&pts)
            };
            let have = snap.map(|s| s.1.windows.clone()).unwrap_or_default();
            for spec in &model.windows {
                match have.iter().find(|w| w.id == spec.id) {
                    Some(w) => {
                        windows.push(window_line(w, true, now, at, trend_of(&w.id)));
                        raw.push(w.clone());
                    }
                    // With a snapshot, a window it lacks is simply not there (the Opus
                    // window appears only for plans that have one); with none yet, the
                    // adapter's windows are listed as not reported.
                    None if snap.is_some() => {}
                    None => {
                        let w = Window {
                            id: spec.id.clone(),
                            label: spec.label.clone(),
                            length_secs: spec.length_secs,
                            ..Default::default()
                        };
                        windows.push(window_line(&w, false, now, at, None));
                    }
                }
            }
            for w in have
                .iter()
                .filter(|w| !model.windows.iter().any(|s| s.id == w.id))
            {
                windows.push(window_line(w, true, now, at, trend_of(&w.id)));
                raw.push(w.clone());
            }
        }
        let mine: Vec<&UsageRun> = runs.iter().filter(|r| r.account == a.name).collect();
        let in_week: Vec<&UsageRun> = mine.iter().copied().filter(|r| r.at >= week_ago).collect();
        // The current window is the shortest reported one.
        let current = raw
            .iter()
            .filter(|w| w.length_secs.is_some())
            .min_by_key(|w| w.length_secs);
        let (win_rows, win_period): (Vec<&UsageRun>, String) = match current {
            Some(w) => {
                let len = w.length_secs.unwrap_or(0);
                let start = match w.resets_at {
                    Some(r) if r > now => r.saturating_sub(len),
                    _ => now.saturating_sub(len),
                };
                (
                    in_week.iter().copied().filter(|r| r.at >= start).collect(),
                    format!("current {} window", w.label),
                )
            }
            None => (vec![], String::new()),
        };
        let spend = api.then(|| {
            let sum = |since: u64| {
                let c: Vec<f64> = mine
                    .iter()
                    .filter(|r| r.at >= since)
                    .filter_map(|r| r.cost_usd)
                    .collect();
                (!c.is_empty()).then(|| c.iter().sum::<f64>())
            };
            let newest = mine
                .iter()
                .filter(|r| r.cost_usd.is_some())
                .map(|r| r.at)
                .max();
            SpendView {
                has_spend: newest.is_some(),
                today_usd: sum(now / DAY * DAY).unwrap_or(0.0),
                week_usd: sum(week_ago).unwrap_or(0.0),
                month_usd: sum(month_start(now)).unwrap_or(0.0),
                has_budget: a.monthly_budget_usd.is_some(),
                budget_usd: a.monthly_budget_usd.unwrap_or(0.0),
                age: newest.map_or(String::new(), |n| words::age(now, n)),
            }
        });
        // A block whose reset has passed is old news, not a block.
        let blocked = snap
            .and_then(|s| s.1.blocked.as_ref())
            .filter(|b| b.until.is_none_or(|u| u > now))
            .map(|b| blocked_text(b, &raw, now));
        // The most constrained window: the fullest, else one that is not ok, else the soonest reset.
        let worst = windows.iter().filter(|w| w.reported).max_by(|x, y| {
            let key = |w: &UsageWindow| (w.has_percent, w.percent, w.state != "ok");
            let (kx, ky) = (key(x), key(y));
            kx.0.cmp(&ky.0)
                .then(kx.1.total_cmp(&ky.1))
                .then(kx.2.cmp(&ky.2))
        });
        if let Some(w) = worst {
            top_bar.push(format!("{} {}", a.name, w.short_text));
        } else if api {
            if let Some(s) = spend.as_ref().filter(|s| s.has_spend) {
                top_bar.push(format!("{} spend ${:.2} this month", a.name, s.month_usd));
            }
        }
        // A hold whose reset has passed is over, even if no session has
        // reached the gate yet to clear it.
        let hold = holds
            .iter()
            .find(|h| h.account == a.name && h.until_ms.is_none_or(|u| u > now * 1000));
        if let Some(h) = hold {
            top_bar.push(format!(
                "{} {}",
                a.name,
                if h.kind == "outage" {
                    "provider outage"
                } else if h.kind == "credentials" {
                    "not logged in"
                } else {
                    "held"
                }
            ));
        }
        // A hold is a block too; an outage is told apart from a limit.
        let blocked = match hold {
            Some(h) => Some(match blocked {
                Some(b) if h.kind == "limit" => b,
                _ => crate::holds::describe(h, now * 1000),
            }),
            None => blocked,
        };
        let status = match hold {
            Some(h) if h.kind == "outage" => "outage",
            _ if blocked.is_some() => "blocked",
            _ => "ok",
        };
        providers.push(ProviderStatus {
            name: a.name.clone(),
            status: status.into(),
            sessions_used: d.broker.used(&llm),
            sessions_limit: d.broker.limit(&llm).unwrap_or(0),
            adapter: crate::providers::adapter_name(a.adapter).into(),
            adapter_name: crate::providers::adapter_name(a.adapter).into(),
            billing: a.billing.name().into(),
            warn_at: a.warn_at,
            overage: a.overage.name().into(),
            monthly_budget_usd: a.monthly_budget_usd.unwrap_or(0.0),
            windows,
            overage_text: snap.map_or(String::new(), |s| {
                let mut t = overage_text(&s.1.overage, now);
                if matches!(s.1.overage.state, OverageState::InUse) {
                    t += &match a.overage {
                        crate::providers::Overage::Allow => {
                            let cost =
                                s.1.spend_usd
                                    .map_or(String::new(), |c| format!(", ${c:.2} so far"));
                            format!(" (allowed: work carries on, extra billing{cost})")
                        }
                        crate::providers::Overage::Hold => " (held at the plan's limit)".into(),
                    };
                }
                t
            }),
            blocked_text: blocked.unwrap_or_default(),
            has_snapshot: snap.is_some(),
            as_of: snap.map_or(0, |s| s.0),
            age: snap.map_or(String::new(), |s| words::age(now, s.0)),
            spend,
            tokens_window: Some(without_notional_cost(
                tokens(&win_rows, &win_period, now, title),
                api,
            )),
            tokens_week: Some(without_notional_cost(
                tokens(&in_week, "last 7 days", now, title),
                api,
            )),
            doing: crate::holds::doing(d, &a.name),
        });
    }
    UsageView {
        providers,
        note: "Figures are what each provider reported, with their age; between sessions nothing new arrives.".into(),
        top_bar,
    }
}

/// A task's own tokens, one line per step (summed over its attempts).
pub fn step_tokens(d: &Daemon, task: u64, now: u64) -> Vec<TokenLine> {
    let api = |account: &str| {
        d.accounts
            .iter()
            .any(|a| a.name == account && matches!(a.billing, crate::providers::Billing::Api))
    };
    let mut by: BTreeMap<String, Agg> = BTreeMap::new();
    let mut order: Vec<String> = vec![];
    for r in d
        .store
        .usage_runs_since(0)
        .unwrap_or_default()
        .iter()
        .filter(|r| r.task == task)
    {
        if !by.contains_key(&r.step) {
            order.push(r.step.clone());
        }
        let mut r = r.clone();
        // A plan's cost figure is notional (decision 0043); only api accounts show it.
        if !api(&r.account) {
            r.cost_usd = None;
        }
        by.entry(r.step.clone()).or_default().add(&r);
    }
    order.iter().map(|s| by[s].line(s, now, task)).collect()
}

// ---- idle refresh ----

/// How often idle accounts are probed.
pub const PROBE_EVERY: Duration = Duration::from_secs(300);

/// One refresh pass: ask each account's adapter for a reading without a
/// session. Only adapters that implement `probe_usage` return anything; none
/// does yet, so this changes nothing today.
pub fn probe_once(d: &Daemon) {
    for a in &d.accounts {
        let billing = billing_kind(a);
        let got = match a.adapter {
            crate::config::Provider::Claude => {
                mason_adapters::claude::ClaudeAdapter::new("claude", "", "").probe_usage(billing)
            }
            crate::config::Provider::Codex => {
                mason_adapters::codex::CodexAdapter::new("codex", "", "").probe_usage(billing)
            }
        };
        match got {
            Ok(Some(s)) => record_snapshot(d, &a.name, &s),
            Ok(None) => {}
            Err(e) => tracing::debug!(account = %a.name, "usage probe failed: {e}"),
        }
    }
}

pub fn spawn_probe(d: &Arc<Daemon>) {
    crate::bg::spawn_loop(d, "usage-probe", move |weak| {
        std::thread::sleep(PROBE_EVERY);
        let Some(d) = weak.upgrade() else {
            return false;
        };
        probe_once(&d);
        true
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn month_start_is_the_first_at_midnight() {
        // 2026-11-07 12:00 UTC
        let now = 1_794_052_800;
        assert_eq!(month_start(now) % DAY, 0);
        let (y, m, d) = civil((month_start(now) / DAY) as i64);
        assert_eq!((y, m, d), (2026, 11, 1));
    }

    #[test]
    fn snapshot_round_trips_through_json() {
        let s = UsageSnapshot {
            windows: vec![Window {
                id: "five_hour".into(),
                label: "5-hour".into(),
                length_secs: Some(18000),
                utilization: None,
                resets_at: Some(99),
                state: WindowState::Warning,
            }],
            overage: Overage {
                state: OverageState::Disabled("org".into()),
                resets_at: None,
            },
            blocked: Some(Blocked {
                why: BlockedWhy::Window("five_hour".into()),
                until: Some(99),
            }),
            spend_usd: Some(1.5),
            observed_at: Some(10),
            session_id: Some("s".into()),
        };
        assert_eq!(snapshot_from_json(&snapshot_json(&s)).unwrap(), s);
    }
}
