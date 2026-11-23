// SPDX-License-Identifier: GPL-3.0-or-later

//! One budget for every call MASON makes to GitHub's REST API.
//!
//! The account's `core` quota (5000 requests an hour) is shared by every poller and every
//! token of the user, so it is tracked once, for the whole process, from the `x-ratelimit-*`
//! headers of every answer. [`Budget`] is the pure part (an injected clock, no I/O); the
//! process-wide record and the thread's caller name sit below it, and [`crate::http::Curl`]
//! feeds and consults them for every request to `api.github.com`.
//!
//! * A refusal for the quota waits for the reset: while the record says the quota is used up
//!   and the reset is ahead, nothing is sent ([`wait_s`]).
//! * A caller stretches its interval when the recent rate would use the quota up before the
//!   reset ([`paced_interval`], [`paced`]); the account-wide counter and CI history first.
//! * Each caller's calls are counted, charged (any answer but 304) and not ([`snapshot`]).

use serde_json::{json, Value};
use std::cell::Cell;
use std::collections::{BTreeMap, VecDeque};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// How far back "recently" looks, in seconds.
pub const WINDOW_S: u64 = 600;

/// The most an interval is stretched, as a multiple of its normal length.
const MAX_FACTOR: u64 = 30;
/// The longest a stretched interval gets, in seconds.
const MAX_INTERVAL_S: u64 = 1800;

/// The `core` quota as the last answers said.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Quota {
    pub limit: u64,
    pub remaining: u64,
    /// Epoch seconds at which the quota is renewed.
    pub reset: u64,
    pub used: Option<u64>,
}

/// How much a caller's polling matters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Priority {
    /// Collecting a result MASON is waiting for (a remote run, the tracker): last to slow down.
    Collect,
    /// Bookkeeping that can wait (the account-wide counter, CI history): slows down first and
    /// leaves a tenth of the quota for the callers above.
    Background,
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.trim())
}

/// The quota an answer's headers describe, when they are the `core` resource's (or name none).
pub fn quota_of(headers: &[(String, String)]) -> Option<Quota> {
    if header(headers, "x-ratelimit-resource").is_some_and(|r| r != "core") {
        return None;
    }
    let num = |n: &str| header(headers, n).and_then(|v| v.parse::<u64>().ok());
    Some(Quota {
        limit: num("x-ratelimit-limit")?,
        remaining: num("x-ratelimit-remaining")?,
        reset: num("x-ratelimit-reset")?,
        used: num("x-ratelimit-used"),
    })
}

/// How many seconds a refusal says to wait: `Retry-After`, or else the time to
/// `x-ratelimit-reset` (GitHub's primary limit sends only that), at least 1. `None` when the
/// answer names neither.
pub fn retry_after(headers: &[(String, String)], now_s: u64) -> Option<u64> {
    if let Some(s) = header(headers, "retry-after").and_then(|v| v.parse::<u64>().ok()) {
        return Some(s);
    }
    let reset = header(headers, "x-ratelimit-reset").and_then(|v| v.parse::<u64>().ok())?;
    Some(reset.saturating_sub(now_s).max(1))
}

/// The interval to use instead of `base_s`, given the quota, the recent charged rate (requests
/// per second, all callers together) and the caller's priority. The normal interval while the
/// projected use up to the reset fits what is left (less the reserve of a background caller); else
/// stretched by how far it overshoots. Pure.
pub fn paced_interval(q: &Quota, now_s: u64, base_s: u64, rate_per_s: f64, prio: Priority) -> u64 {
    let base = base_s.max(1);
    if q.reset <= now_s {
        return base;
    }
    let left_s = (q.reset - now_s) as f64;
    let reserve = match prio {
        Priority::Collect => 0,
        Priority::Background => q.limit / 10,
    };
    let avail = q.remaining.saturating_sub(reserve);
    let projected = rate_per_s * left_s;
    if projected <= avail as f64 {
        return base;
    }
    let factor = if avail == 0 {
        MAX_FACTOR
    } else {
        ((projected / avail as f64).ceil() as u64).clamp(2, MAX_FACTOR)
    };
    // Below its reserve a background caller is at least four times slower.
    let factor = if prio == Priority::Background && q.remaining <= reserve {
        factor.max(4)
    } else {
        factor
    };
    base.saturating_mul(factor).min(MAX_INTERVAL_S).max(base)
}

#[derive(Clone, Copy, Debug, Default)]
struct Totals {
    calls: u64,
    charged: u64,
}

/// The quota and what each caller has spent. Pure: the clock is passed in.
#[derive(Default)]
pub struct Budget {
    core: Option<Quota>,
    /// Recent calls: (second, caller, charged), oldest first.
    events: VecDeque<(u64, String, bool)>,
    totals: BTreeMap<String, Totals>,
}

impl Budget {
    pub fn new() -> Budget {
        Budget::default()
    }

    /// An answer from GitHub: count it for `caller`, and take the quota from its headers.
    /// Answers arrive from many threads in any order, so a record that is newer (a later reset)
    /// replaces the old one and one of the same window keeps the lower remaining count.
    pub fn record(&mut self, caller: &str, now_s: u64, status: u16, headers: &[(String, String)]) {
        let charged = status != 304;
        self.events.push_back((now_s, caller.to_string(), charged));
        self.prune(now_s);
        let t = self.totals.entry(caller.to_string()).or_default();
        t.calls += 1;
        t.charged += charged as u64;
        let Some(new) = quota_of(headers) else {
            return;
        };
        self.core = Some(match self.core {
            Some(old) if old.reset > now_s && old.reset > new.reset => old,
            Some(old) if old.reset > now_s && old.reset == new.reset => Quota {
                remaining: old.remaining.min(new.remaining),
                used: old.used.max(new.used),
                ..new
            },
            _ => new,
        });
    }

    fn prune(&mut self, now_s: u64) {
        while self
            .events
            .front()
            .is_some_and(|(t, _, _)| now_s.saturating_sub(*t) > WINDOW_S)
        {
            self.events.pop_front();
        }
    }

    /// The quota, while its window has not ended.
    pub fn quota(&self, now_s: u64) -> Option<Quota> {
        self.core.filter(|q| q.reset > now_s)
    }

    /// Seconds until the reset when the quota is used up and the reset is ahead: nothing may be
    /// sent until then. At least 1.
    pub fn wait_s(&self, now_s: u64) -> Option<u64> {
        let q = self.quota(now_s)?;
        (q.remaining == 0).then(|| (q.reset - now_s).max(1))
    }

    /// Charged requests per second over the last [`WINDOW_S`] (less when the record is
    /// younger, but never over less than a minute, so one burst is not a rate).
    pub fn charged_rate(&self, now_s: u64) -> f64 {
        let first = self
            .events
            .iter()
            .filter(|(t, _, _)| now_s.saturating_sub(*t) <= WINDOW_S)
            .map(|(t, _, _)| *t)
            .min();
        let Some(first) = first else {
            return 0.0;
        };
        let n = self
            .events
            .iter()
            .filter(|(t, _, c)| *c && now_s.saturating_sub(*t) <= WINDOW_S)
            .count();
        let span = now_s.saturating_sub(first).clamp(60, WINDOW_S);
        n as f64 / span as f64
    }

    /// `base_s`, stretched for the budget.
    pub fn paced(&self, now_s: u64, base_s: u64, prio: Priority) -> u64 {
        match self.quota(now_s) {
            Some(q) => paced_interval(&q, now_s, base_s, self.charged_rate(now_s), prio),
            None => base_s,
        }
    }

    /// Charged requests of the last [`WINDOW_S`] by caller, the most first.
    pub fn charged_recent(&self, now_s: u64) -> Vec<(String, u64)> {
        let mut m: BTreeMap<&str, u64> = BTreeMap::new();
        for (t, who, c) in &self.events {
            if *c && now_s.saturating_sub(*t) <= WINDOW_S {
                *m.entry(who).or_default() += 1;
            }
        }
        let mut v: Vec<(String, u64)> = m.into_iter().map(|(k, n)| (k.to_string(), n)).collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        v
    }

    /// What `mason resources --json` shows under `github_api`.
    pub fn snapshot(&self, now_s: u64) -> Value {
        let recent = self.charged_recent(now_s);
        let callers: Vec<Value> = self
            .totals
            .iter()
            .map(|(name, t)| {
                json!({
                    "name": name,
                    "calls": t.calls,
                    "charged": t.charged,
                    "not_modified": t.calls - t.charged,
                    "charged_recent": recent.iter().find(|(n, _)| n == name).map_or(0, |(_, c)| *c),
                })
            })
            .collect();
        let mut v = json!({
            "window_s": WINDOW_S,
            "callers": callers,
            "recent": recent.iter().map(|(n, c)| json!({"name": n, "charged": c})).collect::<Vec<_>>(),
        });
        if let Some(q) = self.quota(now_s) {
            v["limit"] = q.limit.into();
            v["remaining"] = q.remaining.into();
            v["reset_ms"] = (q.reset * 1000).into();
            v["exhausted"] = (q.remaining == 0).into();
            if let Some(u) = q.used {
                v["used"] = u.into();
            }
        }
        v
    }
}

// ---- the process-wide record ------------------------------------------------------------

static BUDGET: Mutex<Option<Budget>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut Budget) -> R) -> R {
    let mut g = BUDGET.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(Budget::new))
}

fn now_s() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

thread_local! {
    static CALLER: Cell<&'static str> = const { Cell::new("other") };
}

/// Puts the previous caller name back when dropped.
pub struct CallerGuard(&'static str);

impl Drop for CallerGuard {
    fn drop(&mut self) {
        CALLER.with(|c| c.set(self.0));
    }
}

/// Name the caller of the GitHub requests this thread makes until the guard is dropped
/// (`remote-run`, `other-workflows`, `ci-watch`, `tracker`, `landing`; `other` by default).
pub fn as_caller(name: &'static str) -> CallerGuard {
    CallerGuard(CALLER.with(|c| c.replace(name)))
}

/// An answer from `api.github.com`, charged to this thread's caller.
pub fn note(status: u16, headers: &[(String, String)]) {
    let who = CALLER.with(|c| c.get());
    with(|b| b.record(who, now_s(), status, headers));
}

/// Seconds to wait before any request may be sent, while the quota is used up.
pub fn wait_s() -> Option<u64> {
    with(|b| b.wait_s(now_s()))
}

/// The epoch second the quota is renewed, while it is used up.
pub fn exhausted_until() -> Option<u64> {
    let n = now_s();
    with(|b| b.wait_s(n).map(|w| n + w))
}

/// `base_s`, stretched for what is left of the quota.
pub fn paced(base_s: u64, prio: Priority) -> u64 {
    with(|b| b.paced(now_s(), base_s, prio))
}

/// For `mason resources`.
pub fn snapshot() -> Value {
    with(|b| b.snapshot(now_s()))
}

/// The answer sent in place of a request while the quota is used up: GitHub's own refusal,
/// so every caller's classification reads it as the rate limit with the time left.
pub fn refusal_headers(wait: u64, reset: u64) -> Vec<(String, String)> {
    vec![
        ("retry-after".into(), wait.to_string()),
        ("x-ratelimit-remaining".into(), "0".into()),
        ("x-ratelimit-reset".into(), reset.to_string()),
        ("x-ratelimit-resource".into(), "core".into()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hdrs(limit: u64, remaining: u64, reset: u64) -> Vec<(String, String)> {
        vec![
            ("x-ratelimit-limit".into(), limit.to_string()),
            ("x-ratelimit-remaining".into(), remaining.to_string()),
            ("x-ratelimit-reset".into(), reset.to_string()),
            ("x-ratelimit-used".into(), (limit - remaining).to_string()),
            ("x-ratelimit-resource".into(), "core".into()),
        ]
    }

    #[test]
    fn quota_is_read_from_the_core_headers_only() {
        let q = quota_of(&hdrs(5000, 4000, 9000)).unwrap();
        assert_eq!(
            (q.limit, q.remaining, q.reset, q.used),
            (5000, 4000, 9000, Some(1000))
        );
        let mut search = hdrs(30, 0, 9000);
        search.retain(|(k, _)| k != "x-ratelimit-resource");
        search.push(("x-ratelimit-resource".into(), "search".into()));
        assert_eq!(quota_of(&search), None);
        assert_eq!(quota_of(&[]), None);
    }

    #[test]
    fn retry_after_comes_from_the_header_or_the_reset() {
        let mut h = hdrs(5000, 0, 1100);
        assert_eq!(retry_after(&h, 1000), Some(100));
        // Never less than a second, even when the reset has just passed.
        assert_eq!(retry_after(&h, 1100), Some(1));
        assert_eq!(retry_after(&h, 2000), Some(1));
        h.push(("retry-after".into(), "7".into()));
        assert_eq!(retry_after(&h, 1000), Some(7));
        assert_eq!(retry_after(&[], 1000), None);
    }

    #[test]
    fn an_exhausted_quota_waits_for_the_reset() {
        let mut b = Budget::new();
        assert_eq!(b.wait_s(1000), None);
        b.record("ci-watch", 1000, 200, &hdrs(5000, 3, 1500));
        assert_eq!(b.wait_s(1000), None);
        b.record("ci-watch", 1001, 403, &hdrs(5000, 0, 1500));
        assert_eq!(b.wait_s(1001), Some(499));
        assert_eq!(b.wait_s(1499), Some(1));
        // Past the reset the record is stale: ask again.
        assert_eq!(b.wait_s(1500), None);
        assert_eq!(b.quota(1500), None);
    }

    #[test]
    fn answers_out_of_order_keep_the_newer_window_and_the_lower_count() {
        let mut b = Budget::new();
        b.record("a", 1000, 200, &hdrs(5000, 100, 2000));
        b.record("b", 1000, 200, &hdrs(5000, 90, 2000));
        b.record("c", 1000, 200, &hdrs(5000, 95, 2000));
        assert_eq!(b.quota(1000).unwrap().remaining, 90);
        // An answer of an older window (an earlier reset) changes nothing.
        b.record("a", 1001, 200, &hdrs(5000, 4999, 1500));
        assert_eq!(b.quota(1001).unwrap().remaining, 90);
        // A later window replaces it.
        b.record("a", 1002, 200, &hdrs(5000, 4999, 5600));
        assert_eq!(b.quota(1002).unwrap().remaining, 4999);
        // Another resource never changes the core record.
        let mut s = hdrs(30, 0, 5600);
        s.retain(|(k, _)| k != "x-ratelimit-resource");
        s.push(("x-ratelimit-resource".into(), "search".into()));
        b.record("a", 1003, 403, &s);
        assert_eq!(b.wait_s(1003), None);
    }

    #[test]
    fn calls_are_counted_per_caller_charged_or_not() {
        let mut b = Budget::new();
        b.record("remote-run", 1000, 200, &hdrs(5000, 4000, 4600));
        b.record("remote-run", 1001, 304, &hdrs(5000, 4000, 4600));
        b.record("remote-run", 1002, 200, &hdrs(5000, 3999, 4600));
        b.record("ci-watch", 1003, 200, &hdrs(5000, 3998, 4600));
        assert_eq!(
            b.charged_recent(1003),
            vec![("remote-run".to_string(), 2), ("ci-watch".to_string(), 1)]
        );
        let v = b.snapshot(1003);
        let c = v["callers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == "remote-run")
            .unwrap();
        assert_eq!(c["calls"], 3);
        assert_eq!(c["charged"], 2);
        assert_eq!(c["not_modified"], 1);
        assert_eq!(v["remaining"], 3998);
        assert_eq!(v["reset_ms"], 4_600_000);
        assert_eq!(v["exhausted"], false);
        // Ten minutes on, the old calls are no longer recent but still counted.
        assert!(b.charged_recent(1003 + WINDOW_S + 1).is_empty());
        assert_eq!(b.snapshot(1003 + WINDOW_S + 1)["callers"][1]["charged"], 2);
    }

    #[test]
    fn pacing_leaves_intervals_alone_while_use_fits() {
        let q = Quota {
            limit: 5000,
            remaining: 4000,
            reset: 4600,
            used: Some(1000),
        };
        // 1 request/s for the 600 s left is 600 of 4000.
        assert_eq!(paced_interval(&q, 4000, 30, 1.0, Priority::Collect), 30);
        assert_eq!(paced_interval(&q, 4000, 60, 1.0, Priority::Background), 60);
        // After the reset: the normal interval.
        assert_eq!(
            paced_interval(&q, 5000, 30, 100.0, Priority::Background),
            30
        );
    }

    #[test]
    fn pacing_stretches_background_callers_first() {
        // 1000 left (500 over the reserve of a tenth), 600 s to go, 2 requests/s: 1200 projected.
        let q = Quota {
            limit: 5000,
            remaining: 1000,
            reset: 4600,
            used: None,
        };
        let collect = paced_interval(&q, 4000, 30, 2.0, Priority::Collect);
        let background = paced_interval(&q, 4000, 30, 2.0, Priority::Background);
        assert_eq!(collect, 60); // 1200 / 1000, rounded up to 2
        assert_eq!(background, 90); // 1200 / 500 rounded up to 3
        assert!(background > collect);
    }

    #[test]
    fn pacing_is_bounded_and_slows_background_below_its_reserve() {
        let q = Quota {
            limit: 5000,
            remaining: 400,
            reset: 4600,
            used: None,
        };
        // Under the reserve of 500: nothing is left for a background caller.
        assert_eq!(
            paced_interval(&q, 4000, 60, 0.1, Priority::Background),
            1800
        );
        // A collector with 400 for 60 projected still runs at its pace.
        assert_eq!(paced_interval(&q, 4000, 30, 0.1, Priority::Collect), 30);
        // Never more than 30 times, nor past half an hour.
        let q = Quota {
            limit: 5000,
            remaining: 10,
            reset: 4600,
            used: None,
        };
        assert_eq!(paced_interval(&q, 4000, 10, 50.0, Priority::Collect), 300);
        assert_eq!(paced_interval(&q, 4000, 100, 50.0, Priority::Collect), 1800);
        // A zero interval is one second.
        assert_eq!(paced_interval(&q, 4000, 0, 0.0, Priority::Collect), 1);
    }

    #[test]
    fn rate_is_charged_calls_over_the_recent_span_with_a_minute_floor() {
        let mut b = Budget::new();
        for t in 0..30 {
            b.record("x", 1000 + t, 200, &hdrs(5000, 4000, 9000));
        }
        // A burst of 30 within 30 s is spread over a minute at least.
        assert!((b.charged_rate(1030) - 0.5).abs() < 1e-9);
        // Calls answered 304 are not charged.
        let mut b = Budget::new();
        for t in 0..120 {
            b.record("x", 1000 + t, 304, &hdrs(5000, 4000, 9000));
        }
        assert_eq!(b.charged_rate(1120), 0.0);
    }

    #[test]
    fn budget_paces_from_its_own_record() {
        let mut b = Budget::new();
        assert_eq!(b.paced(1000, 30, Priority::Background), 30);
        for t in 0..600 {
            b.record("remote-run", 1000 + t, 200, &hdrs(5000, 700, 3600));
        }
        // 1 request/s for the 2000 s left against 700, 500 of them the reserve.
        assert_eq!(b.paced(1600, 30, Priority::Background), 300);
        assert_eq!(b.paced(1600, 30, Priority::Collect), 90);
    }

    #[test]
    fn the_caller_name_is_per_thread_and_restored() {
        assert_eq!(CALLER.with(|c| c.get()), "other");
        {
            let _g = as_caller("ci-watch");
            assert_eq!(CALLER.with(|c| c.get()), "ci-watch");
            {
                let _h = as_caller("tracker");
                assert_eq!(CALLER.with(|c| c.get()), "tracker");
            }
            assert_eq!(CALLER.with(|c| c.get()), "ci-watch");
            std::thread::spawn(|| assert_eq!(CALLER.with(|c| c.get()), "other"))
                .join()
                .unwrap();
        }
        assert_eq!(CALLER.with(|c| c.get()), "other");
    }
}
