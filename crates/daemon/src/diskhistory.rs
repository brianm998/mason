// SPDX-License-Identifier: GPL-3.0-or-later

//! Free disk space per volume over time, for the monitors' graph (the pattern of
//! `cpuhistory`). Recent readings are kept as sampled (`FINE_CAP` of them); every reading
//! is also folded into one-minute buckets (`COARSE_CAP`, 24 h). Free space is a level, so a
//! bucket keeps each volume's *last* reading in the minute, not a mean. The minute buckets
//! are saved to `disk-history.txt` under the data dir and loaded on start; the fine ring is
//! memory only. A volume that could not be read in a reading is absent from its point.

pub use crate::cpuhistory::parse_span;
use crate::disk::{DiskSource, VolumeSample, DEFAULT_EVERY};
use crate::Daemon;
use mason_protocol::{DiskHistory, DiskPoint, DiskReading, DiskVolume};
use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Weak};

/// Raw readings kept (the same as the CPU's).
pub const FINE_CAP: usize = crate::cpuhistory::FINE_CAP;
pub const COARSE_CAP: usize = crate::cpuhistory::COARSE_CAP;
pub const MINUTE_MS: u64 = crate::cpuhistory::MINUTE_MS;
/// The sampling period the fine buffer is sized for.
pub const FINE_STEP_MS: u64 = DEFAULT_EVERY.as_millis() as u64;
/// The longest span served.
pub const MAX_SPAN_MS: u64 = COARSE_CAP as u64 * MINUTE_MS;
/// The file under the data dir.
pub const FILE: &str = "disk-history.txt";
/// Save the file every this many readings (five minutes).
const SAVE_EVERY: u64 = 10;

/// One volume at one time: `(free, total)` bytes by volume id.
type Levels = BTreeMap<String, (u64, u64)>;

#[derive(Default)]
pub struct DiskRing {
    fine: VecDeque<(u64, Levels)>,
    coarse: VecDeque<(u64, Levels)>,
    /// The open minute: its number and each volume's last reading in it.
    acc: Option<(u64, Levels)>,
    /// What each volume seen is called and holds (the latest).
    volumes: BTreeMap<String, DiskVolume>,
}

fn clean(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

impl DiskRing {
    /// Record one sampling taken at `at_ms`. An empty one (nothing readable) is not recorded.
    pub fn push(&mut self, at_ms: u64, samples: &[VolumeSample]) {
        if samples.is_empty() || self.fine.back().is_some_and(|p| p.0 >= at_ms) {
            return;
        }
        let mut lv = Levels::new();
        for s in samples {
            let v = &s.volume;
            lv.insert(v.id.clone(), (v.free_bytes, v.total_bytes));
            self.volumes.insert(
                v.id.clone(),
                DiskVolume {
                    id: v.id.clone(),
                    name: v.name.clone(),
                    mount: v.mount.display().to_string(),
                    roles: s.roles.iter().map(|r| (*r).to_string()).collect(),
                },
            );
        }
        self.fine.push_back((at_ms, lv.clone()));
        while self.fine.len() > FINE_CAP {
            self.fine.pop_front();
        }
        let minute = at_ms / MINUTE_MS;
        if self.acc.as_ref().is_some_and(|a| a.0 != minute) {
            if let Some((m, levels)) = self.acc.take() {
                let at = m * MINUTE_MS;
                // A bucket restored from the file for the same minute is replaced.
                if self.coarse.back().is_some_and(|b| b.0 >= at) {
                    self.coarse.pop_back();
                }
                self.coarse.push_back((at, levels));
                while self.coarse.len() > COARSE_CAP {
                    self.coarse.pop_front();
                }
            }
        }
        let a = self.acc.get_or_insert_with(|| (minute, Levels::new()));
        a.1.extend(lv);
    }

    /// The minute buckets (and the open minute's so far). `V` lines name the volumes
    /// (`V TAB id TAB name TAB mount TAB roles`), `P` lines the points
    /// (`P TAB at_ms TAB free:total:id ...`).
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        for v in self.volumes.values() {
            out.push_str(&format!(
                "V\t{}\t{}\t{}\t{}\n",
                clean(&v.id),
                clean(&v.name),
                clean(&v.mount),
                clean(&v.roles.join(","))
            ));
        }
        let open = self.acc.as_ref().map(|(m, l)| (m * MINUTE_MS, l.clone()));
        for (at, lv) in self.coarse.iter().cloned().chain(open) {
            out.push_str(&format!("P\t{at}"));
            for (id, (free, total)) in &lv {
                out.push_str(&format!("\t{free}:{total}:{}", clean(id)));
            }
            out.push('\n');
        }
        out
    }

    /// Buckets from [`DiskRing::to_text`], dropping those older than 24 h before `now_ms`,
    /// those out of order and any line that does not parse.
    pub fn from_text(text: &str, now_ms: u64) -> DiskRing {
        let from = now_ms.saturating_sub(MAX_SPAN_MS);
        let mut r = DiskRing::default();
        for l in text.lines() {
            let f: Vec<&str> = l.split('\t').collect();
            match f.as_slice() {
                ["V", id, name, mount, roles] => {
                    r.volumes.insert(
                        (*id).into(),
                        DiskVolume {
                            id: (*id).into(),
                            name: (*name).into(),
                            mount: (*mount).into(),
                            roles: roles
                                .split(',')
                                .filter(|x| !x.is_empty())
                                .map(String::from)
                                .collect(),
                        },
                    );
                }
                ["P", at, rest @ ..] => {
                    let Ok(at_ms) = at.parse::<u64>() else {
                        continue;
                    };
                    if at_ms < from
                        || at_ms > now_ms
                        || r.coarse.back().is_some_and(|b| b.0 >= at_ms)
                    {
                        continue;
                    }
                    let mut lv = Levels::new();
                    for s in rest {
                        let mut q = s.splitn(3, ':');
                        let (Some(fr), Some(t), Some(id)) = (q.next(), q.next(), q.next()) else {
                            continue;
                        };
                        let (Ok(fr), Ok(t)) = (fr.parse(), t.parse()) else {
                            continue;
                        };
                        lv.insert(id.to_string(), (fr, t));
                    }
                    r.coarse.push_back((at_ms, lv));
                    while r.coarse.len() > COARSE_CAP {
                        r.coarse.pop_front();
                    }
                }
                _ => {}
            }
        }
        r
    }

    /// Read the file; an absent or unreadable one gives an empty ring.
    pub fn load(path: &std::path::Path, now_ms: u64) -> DiskRing {
        std::fs::read_to_string(path)
            .map(|t| DiskRing::from_text(&t, now_ms))
            .unwrap_or_default()
    }

    /// Write the file (through a temporary one, so a crash leaves the old). A failure is
    /// logged and the memory copy kept.
    pub fn save(&self, path: &std::path::Path) {
        let tmp = path.with_extension("tmp");
        let r = std::fs::write(&tmp, self.to_text()).and_then(|()| std::fs::rename(&tmp, path));
        if let Err(e) = r {
            tracing::warn!("cannot save the disk history to {}: {e}", path.display());
        }
    }

    /// The readings in `[now_ms - span_ms, now_ms]`, oldest first. A span the raw readings
    /// cover is served from them; a longer one from the minute buckets, then the raw
    /// readings newer than the last of them.
    pub fn query(&self, now_ms: u64, span_ms: u64) -> DiskHistory {
        let span_ms = span_ms.clamp(FINE_STEP_MS, MAX_SPAN_MS);
        let from = now_ms.saturating_sub(span_ms);
        let fine_span = FINE_CAP as u64 * FINE_STEP_MS;
        let mut points: Vec<&(u64, Levels)> = vec![];
        let step_ms = if span_ms <= fine_span {
            FINE_STEP_MS
        } else {
            points.extend(self.coarse.iter());
            MINUTE_MS
        };
        let last_coarse = points.last().map_or(0, |p| p.0 + MINUTE_MS);
        points.extend(
            self.fine
                .iter()
                .filter(|p| span_ms <= fine_span || p.0 >= last_coarse),
        );
        points.retain(|p| p.0 >= from && p.0 <= now_ms);
        let mut seen: Vec<&String> = points.iter().flat_map(|p| p.1.keys()).collect();
        seen.sort();
        seen.dedup();
        DiskHistory {
            // Filled by `Daemon::disk_history`.
            worktrees: vec![],
            span_ms,
            step_ms,
            volumes: seen
                .iter()
                .filter_map(|id| self.volumes.get(*id).cloned())
                .collect(),
            points: points
                .iter()
                .map(|(at, lv)| DiskPoint {
                    at_ms: *at,
                    readings: lv
                        .iter()
                        .map(|(id, (free, total))| DiskReading {
                            volume: id.clone(),
                            free_bytes: *free,
                            total_bytes: *total,
                        })
                        .collect(),
                })
                .collect(),
        }
    }
}

/// Read every relevant volume now and record it. The paths are worked out afresh, as projects
/// come and go.
pub fn take(d: &Daemon, src: &dyn DiskSource, at_ms: u64) {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let projects = d.projects();
    let paths = crate::disk::relevant_paths(&d.cfg, projects.values().copied(), &home);
    let samples = crate::disk::sample_volumes(src, &paths);
    d.record_disk(at_ms, &samples);
}

/// Start the sampling thread (`disk-history`). It reads on a thread of its own, never on the
/// event loop. `MASON_DISK_HISTORY_EVERY_MS` shortens the period for the exit scripts.
pub fn spawn(d: &Arc<Daemon>, src: Arc<dyn DiskSource>) {
    let every = std::env::var("MASON_DISK_HISTORY_EVERY_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map_or(DEFAULT_EVERY, std::time::Duration::from_millis);
    let mut first = true;
    let mut n = 0u64;
    crate::bg::spawn_loop(d, "disk-history", move |weak: &Weak<Daemon>| {
        if !std::mem::take(&mut first) {
            std::thread::sleep(every);
        }
        let Some(d) = weak.upgrade() else {
            return false;
        };
        take(&d, &*src, crate::daemon::now_ms());
        n += 1;
        if n.is_multiple_of(SAVE_EVERY) {
            d.save_disk_history();
        }
        true
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disk::VolumeReading;

    fn s(id: &str, free: u64, total: u64) -> VolumeSample {
        VolumeSample {
            volume: VolumeReading {
                id: id.into(),
                name: format!("n{id}"),
                mount: format!("/m/{id}").into(),
                free_bytes: free,
                total_bytes: total,
            },
            roles: vec!["data"],
        }
    }

    #[test]
    fn a_fine_span_returns_readings_inside_it_in_order() {
        let mut r = DiskRing::default();
        for i in 1..=100u64 {
            r.push(i * FINE_STEP_MS, &[s("a", i, 1000), s("b", 2 * i, 5000)]);
        }
        let now = 100 * FINE_STEP_MS;
        let h = r.query(now, 300_000);
        assert_eq!(h.step_ms, FINE_STEP_MS);
        assert_eq!(h.points.len(), 11);
        assert!(h.points.windows(2).all(|w| w[0].at_ms < w[1].at_ms));
        assert_eq!(
            h.volumes.iter().map(|v| v.id.as_str()).collect::<Vec<_>>(),
            ["a", "b"]
        );
        let last = h.points.last().unwrap();
        assert_eq!(last.readings[1].free_bytes, 200);
        assert_eq!(last.readings[1].total_bytes, 5000);
    }

    #[test]
    fn past_capacity_the_newest_are_kept() {
        let mut r = DiskRing::default();
        let n = FINE_CAP as u64 + 10;
        for i in 1..=n {
            r.push(i * FINE_STEP_MS, &[s("a", i, 1000)]);
        }
        let h = r.query(n * FINE_STEP_MS, FINE_CAP as u64 * FINE_STEP_MS);
        assert_eq!(h.points.len(), FINE_CAP);
        assert_eq!(h.points[0].at_ms, 11 * FINE_STEP_MS);
    }

    #[test]
    fn a_long_span_is_minute_buckets_that_keep_the_last_reading() {
        let mut r = DiskRing::default();
        // Seven hours of readings every 30 s, free space equal to the reading's index.
        let total = 7 * 3600 / 30;
        for i in 0..total as u64 {
            r.push(i * FINE_STEP_MS, &[s("a", i, 1_000_000)]);
        }
        let now = total as u64 * FINE_STEP_MS;
        let h = r.query(now, 24 * 3_600_000);
        assert_eq!(h.step_ms, MINUTE_MS);
        let b = h.points.iter().find(|p| p.at_ms == 10 * MINUTE_MS).unwrap();
        // Minute 10 holds readings 20 and 21: the bucket is the later one.
        assert_eq!(b.readings[0].free_bytes, 21);
        assert!(h.points.windows(2).all(|w| w[0].at_ms < w[1].at_ms));
    }

    #[test]
    fn a_volume_missing_from_a_point_is_absent_but_still_listed() {
        let mut r = DiskRing::default();
        r.push(30_000, &[s("a", 1, 10), s("b", 2, 20)]);
        r.push(60_000, &[s("a", 3, 10)]);
        let h = r.query(60_000, 300_000);
        assert_eq!(h.volumes.len(), 2);
        assert_eq!(h.points[0].readings.len(), 2);
        assert_eq!(h.points[1].readings.len(), 1);
        assert_eq!(h.points[1].readings[0].volume, "a");
        // Nothing readable: no point.
        r.push(90_000, &[]);
        assert_eq!(r.query(90_000, 300_000).points.len(), 2);
    }

    #[test]
    fn the_buckets_round_trip_through_the_file_and_junk_is_ignored() {
        let mut r = DiskRing::default();
        let now = 30 * 3_600_000u64;
        let mut at = 2 * 3_600_000u64;
        while at <= now {
            r.push(at, &[s("a:b", at / 1000, 99), s("c", 5, 50)]);
            at += 20_000;
        }
        let text = r.to_text();
        let back = DiskRing::from_text(&text, now);
        let h = back.query(now, MAX_SPAN_MS);
        assert!(!h.points.is_empty());
        assert!(h.points.iter().all(|p| p.at_ms >= now - MAX_SPAN_MS));
        assert_eq!(h.volumes.len(), 2);
        assert_eq!(h.volumes[0].id, "a:b");
        assert_eq!(h.volumes[0].roles, vec!["data"]);
        assert_eq!(h.volumes[0].name, "na:b");
        let junk = DiskRing::from_text(&format!("x\nP\tz\nP\t5\tq\n{text}"), now);
        assert_eq!(junk.coarse.len(), back.coarse.len());
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join(FILE);
        r.save(&f);
        assert_eq!(DiskRing::load(&f, now).coarse.len(), back.coarse.len());
        assert!(DiskRing::load(&dir.path().join("none"), now)
            .coarse
            .is_empty());
        std::fs::write(&f, "\u{0}\u{1}garbage\n\t\t\n").unwrap();
        assert!(DiskRing::load(&f, now).query(now, 60_000).points.is_empty());
    }

    #[test]
    fn spans_parse_as_the_cpu_ones_do() {
        assert_eq!(parse_span("6h"), Some(6 * 3_600_000));
        assert_eq!(parse_span("x"), None);
    }
}
