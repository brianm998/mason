// SPDX-License-Identifier: GPL-3.0-or-later

//! The CPU readings over time, for the monitors' graph. Recent readings are kept
//! as sampled (`FINE_CAP` of them, an hour at 5 s); every reading is also folded
//! into one-minute buckets (`COARSE_CAP`, 24 h), each the mean of its readings with
//! a share missing from a reading counting as 0. Held in memory only: a restart
//! starts the graph afresh (decision 0321).

use mason_protocol::{CpuHistory, CpuPoint, CpuShare, CpuUsage};
use std::collections::{BTreeMap, VecDeque};

/// Raw readings kept: an hour at the 5 s sampling period.
pub const FINE_CAP: usize = 720;
/// Minute buckets kept: 24 hours.
pub const COARSE_CAP: usize = 1440;
pub const MINUTE_MS: u64 = 60_000;
/// The sampling period the fine buffer is sized for.
pub const FINE_STEP_MS: u64 = 5_000;
/// The longest span served.
pub const MAX_SPAN_MS: u64 = COARSE_CAP as u64 * MINUTE_MS;

type Key = (String, u64);

#[derive(Default)]
struct Bucket {
    minute: u64,
    n: u64,
    sums: BTreeMap<Key, (String, u64)>,
    busy: i64,
}

#[derive(Default)]
pub struct CpuRing {
    fine: VecDeque<CpuPoint>,
    coarse: VecDeque<CpuPoint>,
    acc: Option<Bucket>,
}

fn strip(u: &CpuUsage, at_ms: u64) -> CpuPoint {
    CpuPoint {
        at_ms,
        busy_permille: u.busy_permille,
        shares: u
            .shares
            .iter()
            .map(|s| CpuShare {
                task: s.task,
                kind: s.kind.clone(),
                label: s.label.clone(),
                title: String::new(),
                project: String::new(),
                permille: s.permille,
            })
            .collect(),
    }
}

impl Bucket {
    fn point(&self) -> CpuPoint {
        CpuPoint {
            at_ms: self.minute * MINUTE_MS,
            busy_permille: (self.busy / self.n as i64) as i32,
            shares: self
                .sums
                .iter()
                .map(|((kind, task), (label, sum))| CpuShare {
                    task: *task,
                    kind: kind.clone(),
                    label: label.clone(),
                    title: String::new(),
                    project: String::new(),
                    permille: ((sum + self.n / 2) / self.n) as u32,
                })
                .filter(|s| s.permille >= 1)
                .collect(),
        }
    }
}

impl CpuRing {
    /// Record one reading taken at `at_ms`. Readings with an unknown busy figure
    /// (nothing read) are not recorded.
    pub fn push(&mut self, at_ms: u64, u: &CpuUsage) {
        if u.busy_permille < 0 {
            return;
        }
        if self.fine.back().is_some_and(|p| p.at_ms >= at_ms) {
            return;
        }
        self.fine.push_back(strip(u, at_ms));
        while self.fine.len() > FINE_CAP {
            self.fine.pop_front();
        }
        let minute = at_ms / MINUTE_MS;
        if self.acc.as_ref().is_some_and(|a| a.minute != minute) {
            if let Some(done) = self.acc.take() {
                self.coarse.push_back(done.point());
                while self.coarse.len() > COARSE_CAP {
                    self.coarse.pop_front();
                }
            }
        }
        let a = self.acc.get_or_insert_with(|| Bucket {
            minute,
            ..Bucket::default()
        });
        a.n += 1;
        a.busy += u.busy_permille as i64;
        for s in &u.shares {
            let e = a
                .sums
                .entry((s.kind.clone(), s.task))
                .or_insert_with(|| (s.label.clone(), 0));
            e.1 += s.permille as u64;
        }
    }

    /// The readings in `[now_ms - span_ms, now_ms]`, oldest first. A span of an
    /// hour or less is served from the raw readings; a longer one from the minute
    /// buckets, then the raw readings newer than the last of them.
    pub fn query(&self, now_ms: u64, span_ms: u64) -> CpuHistory {
        let span_ms = span_ms.clamp(FINE_STEP_MS, MAX_SPAN_MS);
        let from = now_ms.saturating_sub(span_ms);
        let fine_span = FINE_CAP as u64 * FINE_STEP_MS;
        let mut points: Vec<CpuPoint> = vec![];
        let step_ms;
        if span_ms <= fine_span {
            step_ms = FINE_STEP_MS;
        } else {
            step_ms = MINUTE_MS;
            points.extend(self.coarse.iter().cloned());
            // Buckets are stamped at their start; the open minute is the raw tail.
        }
        let last_coarse = points.last().map_or(0, |p| p.at_ms + MINUTE_MS);
        points.extend(
            self.fine
                .iter()
                .filter(|p| span_ms <= fine_span || p.at_ms >= last_coarse)
                .cloned(),
        );
        points.retain(|p| p.at_ms >= from && p.at_ms <= now_ms);
        CpuHistory {
            span_ms,
            step_ms,
            points,
        }
    }
}

/// A span as the route takes it: milliseconds, or `5m`, `1h`, `6h`, `24h`, `30s`.
pub fn parse_span(s: &str) -> Option<u64> {
    let s = s.trim();
    let (num, mult) = match s.chars().last()? {
        's' => (&s[..s.len() - 1], 1_000),
        'm' => (&s[..s.len() - 1], 60_000),
        'h' => (&s[..s.len() - 1], 3_600_000),
        'd' => (&s[..s.len() - 1], 86_400_000),
        _ => (s, 1),
    };
    num.parse::<u64>()
        .ok()?
        .checked_mul(mult)
        .filter(|v| *v > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(busy: i32, shares: &[(&str, u64, u32)]) -> CpuUsage {
        CpuUsage {
            busy_permille: busy,
            cores: 4,
            sampled_ms: 1,
            shares: shares
                .iter()
                .map(|(k, t, p)| CpuShare {
                    task: *t,
                    kind: (*k).into(),
                    label: format!("#{t}"),
                    title: "t".into(),
                    project: "p".into(),
                    permille: *p,
                })
                .collect(),
        }
    }

    #[test]
    fn past_capacity_the_newest_are_kept() {
        let mut r = CpuRing::default();
        let n = FINE_CAP as u64 + 10;
        for i in 1..=n {
            r.push(
                i * FINE_STEP_MS,
                &usage(100, &[("task", 1, i as u32 % 900 + 1)]),
            );
        }
        let h = r.query(n * FINE_STEP_MS, 3_600_000);
        assert_eq!(h.points.len(), FINE_CAP);
        assert_eq!(h.points.last().unwrap().at_ms, n * FINE_STEP_MS);
        assert_eq!(h.points[0].at_ms, 11 * FINE_STEP_MS);
    }

    #[test]
    fn a_span_returns_only_readings_inside_it_in_order() {
        let mut r = CpuRing::default();
        for i in 1..=100u64 {
            r.push(
                i * FINE_STEP_MS,
                &usage(500, &[("task", 7, 300), ("other", 0, 200)]),
            );
        }
        let now = 100 * FINE_STEP_MS;
        let h = r.query(now, 300_000);
        assert_eq!(h.step_ms, FINE_STEP_MS);
        assert_eq!(h.points.len(), 61);
        assert!(h.points.windows(2).all(|w| w[0].at_ms < w[1].at_ms));
        assert!(h.points.iter().all(|p| p.at_ms >= now - 300_000));
        let first = &h.points[0];
        assert_eq!(first.shares.len(), 2);
        assert_eq!(first.shares[0].permille, 300);
    }

    #[test]
    fn minute_buckets_keep_totals_within_rounding() {
        let mut r = CpuRing::default();
        // Three hours of readings, alternating 100 and 201 for task 3.
        let total = 3 * 3600 / 5;
        for i in 1..=total as u64 {
            let p = if i % 2 == 0 { 100 } else { 201 };
            r.push(
                i * FINE_STEP_MS,
                &usage(400, &[("task", 3, p), ("masond", 0, 50)]),
            );
        }
        let now = total as u64 * FINE_STEP_MS;
        let h = r.query(now, 6 * 3_600_000);
        assert_eq!(h.step_ms, MINUTE_MS);
        let mut prev = 0;
        for p in &h.points {
            assert!(p.at_ms > prev || prev == 0);
            prev = p.at_ms;
        }
        let full: Vec<&CpuPoint> = h
            .points
            .iter()
            // The first minute is partial (11 readings, not 12), so its mean is off.
            .filter(|p| p.at_ms >= MINUTE_MS && p.at_ms % MINUTE_MS == 0 && p.shares.len() == 2)
            .collect();
        assert!(full.len() > 100);
        for p in full.iter().take(100) {
            let t3 = p.shares.iter().find(|s| s.task == 3).unwrap().permille;
            assert!(
                (148..=153).contains(&t3),
                "mean of 100 and 201 is 150.5, got {t3}"
            );
            assert_eq!(p.busy_permille, 400);
        }
    }

    #[test]
    fn unread_cpu_is_not_recorded() {
        let mut r = CpuRing::default();
        r.push(5_000, &usage(-1, &[]));
        assert!(r.query(5_000, 60_000).points.is_empty());
    }

    #[test]
    fn spans_parse() {
        assert_eq!(parse_span("5m"), Some(300_000));
        assert_eq!(parse_span("24h"), Some(86_400_000));
        assert_eq!(parse_span("1500"), Some(1500));
        assert_eq!(parse_span("x"), None);
        assert_eq!(parse_span("0"), None);
    }
}
