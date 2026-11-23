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

/// How far apart two answers' `x-ratelimit-reset` may be and still name the same window.
/// GitHub's answers for one window differ by a few seconds (2026-11-23: 1795455576 on most,
/// 1795455580 on some), and windows are an hour apart.
const RESET_JITTER_S: u64 = 120;

/// How many answers of a window other than the record's must agree, and outnumber the record's
/// own, before they replace it while the record's window is live and no `/rate_limit` read
/// vouches for it ([`Budget::record`]).
const AGREE: u32 = 3;
/// How often `GET /rate_limit` (free: it is not charged to the quota) is read, in seconds.
const AUTH_EVERY_S: u64 = 300;
/// The soonest a read is tried again after an attempt, failed or not, in seconds.
const AUTH_RETRY_S: u64 = 60;
/// How long a `/rate_limit` read is trusted to name the live window, in seconds: while it is,
/// answers of any other window are ignored.
const AUTH_TRUST_S: u64 = 900;

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
    /// How many answers have named the record's window.
    votes: u32,
    /// Answers of another window than the record's, which is live: the window and how many.
    cand: Option<(Quota, u32)>,
    /// When `GET /rate_limit` last set the record, and when one was last tried.
    auth_at: Option<u64>,
    attempt_at: Option<u64>,
}

/// Do two quotas name one window (resets a few seconds apart)?
fn same_window(a: &Quota, b: &Quota) -> bool {
    a.reset.abs_diff(b.reset) <= RESET_JITTER_S
}

/// Two answers of one window as one quota: the lowest remaining, the highest used, the later
/// reset.
fn merge(old: &Quota, new: &Quota) -> Quota {
    Quota {
        remaining: old.remaining.min(new.remaining),
        used: old.used.max(new.used),
        reset: old.reset.max(new.reset),
        ..*new
    }
}

/// The `core` quota of a `GET /rate_limit` body (`{"resources": {"core": {...}}}`).
pub fn authoritative_of(body: &str) -> Option<Quota> {
    let v: Value = serde_json::from_str(body).ok()?;
    let c = v.get("resources")?.get("core")?;
    let num = |n: &str| c.get(n).and_then(Value::as_u64);
    Some(Quota {
        limit: num("limit")?,
        remaining: num("remaining")?,
        reset: num("reset")?,
        used: num("used"),
    })
}

impl Budget {
    pub fn new() -> Budget {
        Budget::default()
    }

    /// An answer from GitHub: count it for `caller`, and take the quota from its headers.
    /// Answers arrive from many threads in any order, and not all describe the quota that
    /// counts: on 2026-11-23 `/actions/permissions`, `/actions/caches`, `/actions/cache/usage`
    /// and `/releases/latest` answered with `x-ratelimit-resource: core` and a window of their
    /// own (a reset 379 s later, a count of 4795 while the real one was 4368), and the later
    /// reset replaced the record. So, while the record's window is live:
    ///
    /// * an answer of the same window (a reset within [`RESET_JITTER_S`]) keeps the lower
    ///   remaining count;
    /// * an answer of another window changes nothing when a `/rate_limit` read of the last
    ///   [`AUTH_TRUST_S`] seconds vouches for the record ([`Budget::note_authoritative`]);
    /// * else it is only a candidate, and replaces the record once [`AGREE`] answers named
    ///   that window and they outnumber those of the record's own (so a first odd answer after a
    ///   restart loses to the real window's answers that follow, earlier or later).
    ///
    /// Once the record's window has ended, the next answer starts a new one.
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
        let Some(old) = self.core.filter(|q| q.reset > now_s) else {
            self.core = Some(new);
            self.votes = 1;
            self.cand = None;
            return;
        };
        if same_window(&old, &new) {
            self.core = Some(merge(&old, &new));
            self.votes = self.votes.saturating_add(1);
            return;
        }
        if self.authority_fresh(now_s) {
            return;
        }
        let (cand, n) = match self.cand {
            Some((c, n)) if same_window(&c, &new) => (merge(&c, &new), n + 1),
            _ => (new, 1),
        };
        if n >= AGREE && n > self.votes {
            self.core = Some(cand);
            self.votes = n;
            self.cand = None;
        } else {
            self.cand = Some((cand, n));
        }
    }

    fn authority_fresh(&self, now_s: u64) -> bool {
        self.auth_at
            .is_some_and(|t| now_s.saturating_sub(t) <= AUTH_TRUST_S)
    }

    /// The answer of `GET /rate_limit`: the truth. It sets the record to that window, keeping
    /// the lower remaining count when the record named the same one (an answer may have
    /// arrived after the read was made).
    pub fn note_authoritative(&mut self, now_s: u64, q: Quota) {
        self.auth_at = Some(now_s);
        self.attempt_at = Some(now_s);
        self.cand = None;
        self.votes = AGREE;
        self.core = Some(match self.core.filter(|o| o.reset > now_s) {
            Some(old) if same_window(&old, &q) => Quota {
                remaining: old.remaining.min(q.remaining),
                used: old.used.max(q.used),
                ..q
            },
            _ => q,
        });
    }

    /// Whether a `/rate_limit` read is due, and if so it is now taken as tried (the caller makes
    /// it): never within [`AUTH_RETRY_S`] of the last attempt, else when none has succeeded,
    /// the last is [`AUTH_EVERY_S`] old, or the record's window has ended.
    pub fn claim_refresh(&mut self, now_s: u64) -> bool {
        if self
            .attempt_at
            .is_some_and(|t| now_s.saturating_sub(t) < AUTH_RETRY_S)
        {
            return false;
        }
        let due = self.quota(now_s).is_none()
            || self
                .auth_at
                .is_none_or(|t| now_s.saturating_sub(t) >= AUTH_EVERY_S);
        if due {
            self.attempt_at = Some(now_s);
        }
        due
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
        if let Some(t) = self.auth_at {
            v["checked_ms"] = (t * 1000).into();
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
/// (`remote-run`, `other-workflows`, `ci-watch`, `tracker`, `landing`, ...; by default the
/// thread's name under `other:`, see [`thread_label`]).
pub fn as_caller(name: &'static str) -> CallerGuard {
    CallerGuard(CALLER.with(|c| c.replace(name)))
}

/// What a thread without a caller name is counted under: `other:` and the start of its
/// thread's name (`other:batch` for `batch-3`, `other:unnamed` for none), so the unnamed
/// spenders show up in `mason resources` by where they run.
pub fn thread_label(thread: Option<&str>) -> String {
    let kind = thread
        .and_then(|n| n.split(['-', ':', '/']).next())
        .filter(|k| !k.is_empty())
        .unwrap_or("unnamed");
    format!("other:{kind}")
}

/// The caller this thread's requests are counted under now.
pub fn current_caller() -> String {
    match CALLER.with(|c| c.get()) {
        "other" => thread_label(std::thread::current().name()),
        named => named.to_string(),
    }
}

/// The request path without the host or the query: what a debug line may show.
pub(crate) fn path_of(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let path = rest.find('/').map_or("", |i| &rest[i..]);
    path.split('?').next().unwrap_or(path)
}

/// An answer from `api.github.com`, charged to this thread's caller.
pub fn note(status: u16, headers: &[(String, String)]) {
    let who = current_caller();
    with(|b| b.record(&who, now_s(), status, headers));
}

/// [`note`], with a debug line saying who asked for what (the path, never the query or a
/// token): `RUST_LOG=mason_adapter_api=debug` shows each request's caller and thread.
pub fn note_request(method: &str, url: &str, status: u16, headers: &[(String, String)]) {
    let who = current_caller();
    tracing::debug!(
        caller = %who,
        thread = std::thread::current().name().unwrap_or("unnamed"),
        method,
        path = path_of(url),
        status,
        "GitHub request"
    );
    with(|b| b.record(&who, now_s(), status, headers));
}

/// Whether this thread should read `GET /rate_limit` now (it is then taken as tried: see
/// [`Budget::claim_refresh`]).
pub fn claim_refresh() -> bool {
    with(|b| b.claim_refresh(now_s()))
}

/// The body of a `GET /rate_limit` answer: the record's truth.
pub fn note_rate_limit(body: &str) {
    if let Some(q) = authoritative_of(body) {
        with(|b| b.note_authoritative(now_s(), q));
    }
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
        // A later window replaces it once the old one has ended.
        b.record("a", 2000, 200, &hdrs(5000, 4999, 5600));
        assert_eq!(b.quota(2000).unwrap().remaining, 4999);
        // Another resource never changes the core record.
        let mut s = hdrs(30, 0, 5600);
        s.retain(|(k, _)| k != "x-ratelimit-resource");
        s.push(("x-ratelimit-resource".into(), "search".into()));
        b.record("a", 2001, 403, &s);
        assert_eq!(b.wait_s(2001), None);
    }

    /// Two answers of one window as GitHub sent them on 2026-11-23: the same token and hour,
    /// resets 4 s apart. The later reset arrived first and made the record ignore every answer
    /// of the earlier one, so `remaining` stayed at 4737 while GitHub's count went to 3443.
    #[test]
    fn answers_of_one_window_with_resets_a_few_seconds_apart_keep_the_lowest_count() {
        let now = 1_795_451_900;
        let mut b = Budget::new();
        b.record("a", now, 200, &hdrs(5000, 4737, 1_795_455_580));
        b.record("remote-run", now, 200, &hdrs(5000, 3443, 1_795_455_576));
        let q = b.quota(now).unwrap();
        assert_eq!((q.remaining, q.used), (3443, Some(1557)));
        assert_eq!(q.reset, 1_795_455_580);
        // A later answer with the higher reset does not lift it again, nor does a 304's.
        b.record("a", now + 1, 200, &hdrs(5000, 3440, 1_795_455_580));
        b.record("a", now + 2, 304, &hdrs(5000, 4999, 1_795_455_580));
        assert_eq!(b.quota(now + 2).unwrap().remaining, 3440);
        // The next hour's window replaces the record.
        b.record("a", 1_795_455_600, 200, &hdrs(5000, 4990, 1_795_459_180));
        assert_eq!(b.quota(1_795_455_600).unwrap().remaining, 4990);
    }

    /// Measured 2026-11-23: GitHub's `/rate_limit` said remaining 4368, reset 1795466385, while
    /// `/actions/permissions`, `/actions/caches`, `/actions/cache/usage` and `/releases/latest`
    /// answered 4795 with reset 1795466764 (a window of their own, 379 s later), and the record
    /// took it.
    const REAL: u64 = 1_795_466_385;
    const ODD: u64 = 1_795_466_764;
    const NOW: u64 = 1_795_464_800;

    #[test]
    fn one_answer_of_a_later_window_does_not_replace_a_live_record() {
        let mut b = Budget::new();
        b.record("a", NOW, 200, &hdrs(5000, 4368, REAL));
        b.record("ci-watch", NOW + 1, 200, &hdrs(5000, 4795, ODD));
        let q = b.quota(NOW + 1).unwrap();
        assert_eq!((q.remaining, q.reset), (4368, REAL));
        // Nor do two, nor does it lift the count when the real window answers again.
        b.record("ci-watch", NOW + 2, 200, &hdrs(5000, 4794, ODD));
        b.record("a", NOW + 3, 200, &hdrs(5000, 4360, REAL - 1));
        let q = b.quota(NOW + 3).unwrap();
        assert_eq!((q.remaining, q.reset), (4360, REAL));
    }

    #[test]
    fn a_new_window_is_taken_once_the_old_one_has_ended() {
        let mut b = Budget::new();
        b.record("a", NOW, 200, &hdrs(5000, 10, REAL));
        // The odd one is ahead while the record is live: ignored. At the reset the record is
        // stale and the next answer, of whichever window, starts afresh.
        b.record("a", NOW + 1, 200, &hdrs(5000, 4795, ODD));
        assert_eq!(b.quota(NOW + 1).unwrap().remaining, 10);
        assert_eq!(b.quota(REAL), None);
        b.record("a", REAL + 1, 200, &hdrs(5000, 4999, REAL + 3600));
        let q = b.quota(REAL + 1).unwrap();
        assert_eq!((q.remaining, q.reset), (4999, REAL + 3600));
    }

    #[test]
    fn after_a_restart_a_first_odd_answer_loses_to_the_answers_that_follow() {
        let mut b = Budget::new();
        // The first answer after the start is the odd bucket's.
        b.record("a", NOW, 200, &hdrs(5000, 4825, ODD));
        assert_eq!(b.quota(NOW).unwrap().reset, ODD);
        // The real window's answers, earlier by 379 s, are only candidates at first ...
        b.record("a", NOW + 1, 200, &hdrs(5000, 4368, REAL));
        b.record("a", NOW + 2, 200, &hdrs(5000, 4367, REAL));
        assert_eq!(b.quota(NOW + 2).unwrap().reset, ODD);
        // ... and replace the record when three agree and outnumber the odd one's.
        b.record("a", NOW + 3, 200, &hdrs(5000, 4366, REAL - 1));
        let q = b.quota(NOW + 3).unwrap();
        assert_eq!((q.remaining, q.reset), (4366, REAL));
        // Odd answers now are the minority.
        b.record("a", NOW + 4, 200, &hdrs(5000, 4793, ODD));
        b.record("a", NOW + 5, 200, &hdrs(5000, 4792, ODD));
        b.record("a", NOW + 6, 200, &hdrs(5000, 4791, ODD));
        assert_eq!(b.quota(NOW + 6).unwrap().reset, REAL);
    }

    #[test]
    fn a_rate_limit_read_is_the_truth_and_shields_the_record() {
        let mut b = Budget::new();
        b.record("a", NOW, 200, &hdrs(5000, 4825, ODD));
        let body = r#"{"resources":{"core":{"limit":5000,"used":632,"remaining":4368,"reset":1795466385},
            "search":{"limit":30,"used":0,"remaining":30,"reset":1795464860}}}"#;
        let q = authoritative_of(body).unwrap();
        assert_eq!((q.remaining, q.used, q.reset), (4368, Some(632), REAL));
        b.note_authoritative(NOW + 1, q);
        let q = b.quota(NOW + 1).unwrap();
        assert_eq!((q.remaining, q.reset), (4368, REAL));
        // However many odd answers follow, they change nothing; the real ones are merged.
        for i in 0..10 {
            b.record("a", NOW + 2 + i, 200, &hdrs(5000, 4790 - i, ODD));
        }
        b.record("a", NOW + 20, 200, &hdrs(5000, 4300, REAL - 1));
        let q = b.quota(NOW + 20).unwrap();
        assert_eq!((q.remaining, q.reset), (4300, REAL));
        assert_eq!(b.snapshot(NOW + 20)["checked_ms"], (NOW + 1) * 1000);
        // A read that is not a rate-limit body names nothing.
        assert_eq!(authoritative_of("{}"), None);
        assert_eq!(authoritative_of("not json"), None);
    }

    #[test]
    fn a_rate_limit_read_that_is_stale_no_longer_shields_the_record() {
        let mut b = Budget::new();
        let q = authoritative_of(&format!(
            r#"{{"resources":{{"core":{{"limit":5000,"used":1000,"remaining":4000,"reset":{REAL}}}}}}}"#
        ))
        .unwrap();
        b.note_authoritative(NOW, q);
        // Within the trust time the odd window is ignored however often it answers.
        for i in 0..5 {
            b.record("a", NOW + AUTH_TRUST_S, 200, &hdrs(5000, 4700 - i, ODD));
        }
        assert_eq!(b.quota(NOW + AUTH_TRUST_S).unwrap().reset, REAL);
        // Past it (reads kept failing) the voting rule decides: the read counts as AGREE
        // answers, so the other window needs more than that.
        let later = NOW + AUTH_TRUST_S + 1;
        for i in 0..AGREE as u64 {
            b.record("a", later, 200, &hdrs(5000, 4700 - i, ODD));
        }
        assert_eq!(b.quota(later).unwrap().reset, REAL);
        b.record("a", later, 200, &hdrs(5000, 4690, ODD));
        assert_eq!(b.quota(later).unwrap().reset, ODD);
    }

    #[test]
    fn a_rate_limit_read_is_due_at_the_start_every_few_minutes_and_after_a_reset() {
        let mut b = Budget::new();
        assert!(b.claim_refresh(NOW));
        // Taken as tried: a second thread does not repeat it, nor within a minute.
        assert!(!b.claim_refresh(NOW));
        assert!(!b.claim_refresh(NOW + AUTH_RETRY_S - 1));
        // A failed read (nothing noted) is tried again after the retry time.
        assert!(b.claim_refresh(NOW + AUTH_RETRY_S));
        let q = Quota {
            limit: 5000,
            remaining: 4000,
            reset: REAL,
            used: Some(1000),
        };
        b.note_authoritative(NOW + 100, q);
        assert!(!b.claim_refresh(NOW + 100 + AUTH_EVERY_S - 1));
        assert!(b.claim_refresh(NOW + 100 + AUTH_EVERY_S));
        // When the window has ended a read is due as soon as the retry time allows.
        b.note_authoritative(REAL - 10, q);
        assert!(!b.claim_refresh(REAL + 1));
        assert!(b.claim_refresh(REAL + AUTH_RETRY_S));
    }

    #[test]
    fn answers_without_core_headers_and_304s_of_the_live_window_do_not_disturb_it() {
        let mut b = Budget::new();
        b.record("a", NOW, 200, &hdrs(5000, 4368, REAL));
        // A 404 or a 304 that carries the window's headers is the same window (as probed);
        // one with no ratelimit headers at all sets nothing.
        b.record("a", NOW + 1, 404, &hdrs(5000, 4367, REAL - 1));
        b.record("a", NOW + 2, 304, &hdrs(5000, 4367, REAL));
        b.record("a", NOW + 3, 404, &[]);
        let q = b.quota(NOW + 3).unwrap();
        assert_eq!((q.remaining, q.reset), (4367, REAL));
        assert_eq!(b.snapshot(NOW + 3)["callers"][0]["calls"], 4);
    }

    #[test]
    fn unnamed_threads_are_counted_under_their_thread_name() {
        assert_eq!(thread_label(Some("batch-3")), "other:batch");
        assert_eq!(thread_label(Some("job-mason/full-ci")), "other:job");
        assert_eq!(thread_label(Some("http-worker-1")), "other:http");
        assert_eq!(thread_label(Some("")), "other:unnamed");
        assert_eq!(thread_label(None), "other:unnamed");
        std::thread::Builder::new()
            .name("sweeper-1".into())
            .spawn(|| {
                assert_eq!(current_caller(), "other:sweeper");
                let _g = as_caller("remote-sweep");
                assert_eq!(current_caller(), "remote-sweep");
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn a_debug_line_names_the_path_only() {
        assert_eq!(
            path_of("https://api.github.com/repos/a/b/actions/runs?head_sha=abc&per_page=20"),
            "/repos/a/b/actions/runs"
        );
        assert_eq!(path_of("https://api.github.com"), "");
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
