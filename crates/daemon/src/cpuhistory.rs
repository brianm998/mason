// SPDX-License-Identifier: GPL-3.0-or-later

//! The CPU readings over time, for the monitors' graph. Recent readings are kept
//! as sampled (`FINE_CAP` of them, an hour at 5 s); every reading is also folded
//! into one-minute buckets (`COARSE_CAP`, 24 h), each the mean of its readings with
//! a share missing from a reading counting as 0. The minute buckets are saved to a
//! file under the data dir and loaded on start (decision 0465, replacing the
//! memory-only rule of 0321); the fine ring is memory only.

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

/// The file under the data dir.
pub const FILE: &str = "cpu-history.txt";

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
                let pt = done.point();
                // A bucket restored from the file for the same minute is replaced.
                if self.coarse.back().is_some_and(|b| b.at_ms >= pt.at_ms) {
                    self.coarse.pop_back();
                }
                self.coarse.push_back(pt);
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

    /// The minute buckets (and the open minute's so far), one line each:
    /// `at_ms TAB busy TAB kind:task:permille:label ...`.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        let open = self.acc.as_ref().map(Bucket::point);
        for p in self.coarse.iter().chain(open.iter()) {
            out.push_str(&format!("{}\t{}", p.at_ms, p.busy_permille));
            for s in &p.shares {
                let label: String = s
                    .label
                    .chars()
                    .map(|c| if c.is_control() { ' ' } else { c })
                    .collect();
                let kind: String = s.kind.chars().filter(|c| *c != ':' && !c.is_control()).collect();
                out.push_str(&format!("\t{kind}:{}:{}:{label}", s.task, s.permille));
            }
            out.push('\n');
        }
        out
    }

    /// Buckets from [`CpuRing::to_text`], dropping those older than 24 h before
    /// `now_ms`, those out of order and any line that does not parse.
    pub fn from_text(text: &str, now_ms: u64) -> CpuRing {
        let from = now_ms.saturating_sub(MAX_SPAN_MS);
        let mut r = CpuRing::default();
        for l in text.lines() {
            let mut f = l.split('\t');
            let (Some(at), Some(busy)) = (f.next(), f.next()) else {
                continue;
            };
            let (Ok(at_ms), Ok(busy_permille)) = (at.parse::<u64>(), busy.parse::<i32>()) else {
                continue;
            };
            if at_ms < from || at_ms > now_ms || r.coarse.back().is_some_and(|b| b.at_ms >= at_ms)
            {
                continue;
            }
            let mut shares = vec![];
            for s in f {
                let mut q = s.splitn(4, ':');
                let (Some(k), Some(t), Some(pm), Some(label)) =
                    (q.next(), q.next(), q.next(), q.next())
                else {
                    continue;
                };
                let (Ok(task), Ok(permille)) = (t.parse(), pm.parse()) else {
                    continue;
                };
                shares.push(CpuShare {
                    task,
                    kind: k.to_string(),
                    label: label.to_string(),
                    title: String::new(),
                    project: String::new(),
                    permille,
                });
            }
            r.coarse.push_back(CpuPoint {
                at_ms,
                busy_permille,
                shares,
            });
            while r.coarse.len() > COARSE_CAP {
                r.coarse.pop_front();
            }
        }
        r
    }

    /// Read the file; an absent or unreadable one gives an empty ring.
    pub fn load(path: &std::path::Path, now_ms: u64) -> CpuRing {
        std::fs::read_to_string(path)
            .map(|t| CpuRing::from_text(&t, now_ms))
            .unwrap_or_default()
    }

    /// Write the file (through a temporary one, so a crash leaves the old). A
    /// failure is logged and the memory copy kept.
    pub fn save(&self, path: &std::path::Path) {
        let tmp = path.with_extension("tmp");
        let r = std::fs::write(&tmp, self.to_text()).and_then(|()| std::fs::rename(&tmp, path));
        if let Err(e) = r {
            tracing::warn!("cannot save the CPU history to {}: {e}", path.display());
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
        // Start at 0 so the first minute holds 12 readings, 6 of each value, like every other.
        let total = 3 * 3600 / 5;
        for i in 0..total as u64 {
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
    fn the_buckets_round_trip_through_the_file_and_old_ones_are_dropped() {
        let mut r = CpuRing::default();
        let now = 30 * 3_600_000u64;
        // Hours 2..30 of readings, one per minute-and-a-bit; the first hours are over 24 h old.
        let mut at = 2 * 3_600_000u64;
        while at <= now {
            r.push(
                at,
                &usage(500, &[("task", 7, 300), ("batch", 303, 100), ("other", 0, 100)]),
            );
            at += 20_000;
        }
        let text = r.to_text();
        let back = CpuRing::from_text(&text, now);
        let h = back.query(now, MAX_SPAN_MS);
        assert!(!h.points.is_empty());
        assert!(h.points.iter().all(|p| p.at_ms >= now - MAX_SPAN_MS));
        assert!(back.coarse.front().unwrap().at_ms >= now - MAX_SPAN_MS);
        let last = h.points.last().unwrap();
        assert_eq!(last.busy_permille, 500);
        let b = last.shares.iter().find(|s| s.kind == "batch").unwrap();
        assert_eq!((b.task, b.permille, b.label.as_str()), (303, 100, "#303"));
        // Garbage lines are skipped.
        let junk = CpuRing::from_text(&format!("x\ny\tz\n{text}"), now);
        assert_eq!(junk.coarse.len(), back.coarse.len());
        // And the file itself.
        let dir = std::env::temp_dir().join(format!("mason-cpuhist-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join(FILE);
        r.save(&f);
        assert_eq!(CpuRing::load(&f, now).coarse.len(), back.coarse.len());
        assert!(CpuRing::load(&dir.join("none"), now).coarse.is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
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
