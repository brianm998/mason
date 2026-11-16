// SPDX-License-Identifier: GPL-3.0-or-later

//! How busy macOS's `syspolicyd` is, for the `compile` gate (s502, decision 0495).
//!
//! `syspolicyd` scans every new executable the first time it runs, signed or not. Measured on
//! 2026-11-16 on the owner's Intel Mac: about 0.3 to 0.8 CPU s for each fresh 40 MB test binary,
//! about 0.03 s for a 0.5 MB one, nothing for a repeat launch and nothing for a file written but
//! not run; ad-hoc signing made no difference. A workspace build links about 180 such binaries,
//! and ten tasks building and testing at once kept `syspolicyd` at 7 cores until WindowServer's
//! watchdog killed the owner's session. CPU use cannot hold that back: the scans are Apple's
//! process, and the compiles that cause them are mostly waiting on it.
//!
//! The scheduler is pure and is only handed a number. The reading comes from `/bin/ps`, which is
//! set-uid root and can read a root process's CPU time (`proc_pid_rusage` cannot); it is an Apple
//! binary that never changes, so running it adds nothing to `syspolicyd`'s own work. It runs on
//! a thread of its own, so a slow `ps` never blocks the broker.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// A busy reading keeps counting for this long, so compiles do not pile back in the moment the
/// scans pause between two test binaries.
pub const HOLD_MS: u64 = 30_000;

#[cfg(target_os = "macos")]
const EVERY: Duration = Duration::from_secs(5);

/// No reading yet, or the reading failed.
const NONE: u32 = u32::MAX;

/// Where the number comes from. Tests inject their own.
pub trait PolicySource: Send + Sync + std::fmt::Debug {
    /// `syspolicyd`'s recent CPU use in percent of one core (700 is seven cores), or `None`
    /// if nothing is known (another OS, or `ps` failed).
    fn busy_pct(&self) -> Option<u32>;
}

/// A fixed, settable value.
#[derive(Debug)]
pub struct FakePolicy(AtomicU32);

impl FakePolicy {
    pub fn new(pct: u32) -> Arc<FakePolicy> {
        Arc::new(FakePolicy(AtomicU32::new(pct)))
    }
    pub fn set(&self, pct: u32) {
        self.0.store(pct, Ordering::SeqCst);
    }
}

impl PolicySource for FakePolicy {
    fn busy_pct(&self) -> Option<u32> {
        Some(self.0.load(Ordering::SeqCst))
    }
}

/// `ps -o time=` as seconds: `MM:SS.ss` (minutes may run past 59), `HH:MM:SS` or
/// `D-HH:MM:SS`, with surrounding spaces.
pub fn parse_cputime(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let (days, rest) = match s.split_once('-') {
        Some((d, r)) => (d.parse::<f64>().ok()?, r),
        None => (0.0, s),
    };
    let parts: Vec<&str> = rest.split(':').collect();
    let nums: Vec<f64> = parts
        .iter()
        .map(|p| p.parse::<f64>().ok())
        .collect::<Option<_>>()?;
    let secs = match nums.as_slice() {
        [m, s] => m * 60.0 + s,
        [h, m, s] => h * 3600.0 + m * 60.0 + s,
        _ => return None,
    };
    Some(days * 86_400.0 + secs)
}

/// The pid of `syspolicyd` in `ps -A -o pid=,comm=` output.
pub fn find_pid(ps_out: &str) -> Option<u32> {
    ps_out.lines().find_map(|l| {
        let (pid, comm) = l.trim().split_once(char::is_whitespace)?;
        let comm = comm.trim();
        (comm == "syspolicyd" || comm.ends_with("/syspolicyd")).then(|| pid.parse().ok())?
    })
}

/// CPU time samples turned into a held reading. Pure: the times are passed in.
#[derive(Debug, Default)]
pub struct Meter {
    last: Option<(u64, f64)>,
    /// (when, percent) for each interval, newest last, kept for [`HOLD_MS`].
    recent: VecDeque<(u64, u32)>,
}

impl Meter {
    /// A CPU-time sample (`cpu_s`, the process's total) taken at `now_ms`.
    pub fn add(&mut self, now_ms: u64, cpu_s: f64) {
        if let Some((at, before)) = self.last {
            let wall_ms = now_ms.saturating_sub(at);
            // A restarted syspolicyd starts its CPU time again from 0: skip that interval.
            if wall_ms > 0 && cpu_s >= before {
                let pct = ((cpu_s - before) * 100_000.0 / wall_ms as f64).round();
                self.recent
                    .push_back((now_ms, pct.clamp(0.0, f64::from(NONE - 1)) as u32));
            }
        }
        self.last = Some((now_ms, cpu_s));
        while self
            .recent
            .front()
            .is_some_and(|(at, _)| now_ms.saturating_sub(*at) > HOLD_MS)
        {
            self.recent.pop_front();
        }
    }

    /// The process was not found or `ps` failed: the next sample starts a new interval.
    pub fn lost(&mut self) {
        self.last = None;
    }

    /// The busiest interval within [`HOLD_MS`] of `now_ms`.
    pub fn reading(&self, now_ms: u64) -> Option<u32> {
        self.recent
            .iter()
            .filter(|(at, _)| now_ms.saturating_sub(*at) <= HOLD_MS)
            .map(|(_, p)| *p)
            .max()
    }
}

/// The real reading: a thread samples `syspolicyd` every few seconds.
#[derive(Debug)]
pub struct Sampler {
    pct: AtomicU32,
}

impl PolicySource for Sampler {
    fn busy_pct(&self) -> Option<u32> {
        let v = self.pct.load(Ordering::SeqCst);
        (v != NONE).then_some(v)
    }
}

impl Sampler {
    /// Start sampling. On anything but macOS the reading is always `None` and no thread starts.
    pub fn start() -> Arc<Sampler> {
        let s = Arc::new(Sampler {
            pct: AtomicU32::new(NONE),
        });
        #[cfg(target_os = "macos")]
        {
            let me = s.clone();
            let _ = std::thread::Builder::new()
                .name("syspolicyd-sampler".into())
                .spawn(move || me.run());
        }
        s
    }

    #[cfg(target_os = "macos")]
    fn run(&self) {
        let mut meter = Meter::default();
        let mut pid: Option<u32> = None;
        loop {
            if pid.is_none() {
                pid = ps(&["-A", "-o", "pid=,comm="])
                    .as_deref()
                    .and_then(find_pid);
            }
            let sample = pid.and_then(|p| {
                ps(&["-o", "time=", "-p", &p.to_string()])
                    .as_deref()
                    .and_then(parse_cputime)
            });
            let now = crate::daemon::now_ms();
            match sample {
                Some(cpu_s) => meter.add(now, cpu_s),
                None => {
                    // Gone (it crashed and launchd has not restarted it) or ps failed: look it
                    // up again next time, and say nothing rather than something stale.
                    pid = None;
                    meter.lost();
                }
            }
            let v = if sample.is_some() {
                meter.reading(now).unwrap_or(NONE)
            } else {
                NONE
            };
            self.pct.store(v, Ordering::SeqCst);
            std::thread::sleep(EVERY);
        }
    }
}

/// `/bin/ps` with `args`, its stdout if it exited 0.
#[cfg(target_os = "macos")]
fn ps(args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("/bin/ps")
        .args(args)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ps_cpu_times_parse_in_each_shape() {
        assert_eq!(parse_cputime("   2:25.56"), Some(145.56));
        assert_eq!(parse_cputime("2530:40.50"), Some(2530.0 * 60.0 + 40.5));
        assert_eq!(parse_cputime("1:02:03"), Some(3723.0));
        assert_eq!(parse_cputime("2-01:00:00"), Some(2.0 * 86_400.0 + 3600.0));
        assert_eq!(parse_cputime(""), None);
        assert_eq!(parse_cputime("abc"), None);
        assert_eq!(parse_cputime("1:2:3:4"), None);
    }

    #[test]
    fn the_pid_is_found_by_its_command_name_only() {
        let out = "    1 /sbin/launchd\n  325 /usr/libexec/syspolicyd\n  400 /usr/bin/syspolicyd-helper\n";
        assert_eq!(find_pid(out), Some(325));
        assert_eq!(find_pid("  9 syspolicyd\n"), Some(9));
        assert_eq!(find_pid("  9 /usr/libexec/notsyspolicyd\n"), None);
        assert_eq!(find_pid(""), None);
    }

    #[test]
    fn the_reading_is_cores_in_percent_and_held_for_a_while() {
        let mut m = Meter::default();
        m.add(0, 100.0);
        assert_eq!(m.reading(0), None, "one sample is no interval");
        m.add(5_000, 135.0); // 35 CPU s in 5 s: seven cores
        assert_eq!(m.reading(5_000), Some(700));
        m.add(10_000, 135.5); // quiet now
        assert_eq!(
            m.reading(10_000),
            Some(700),
            "a busy interval keeps counting for HOLD_MS"
        );
        // The seven-core interval has aged out; the quiet one (0.5 s in 5 s) is the busiest left.
        m.add(5_000 + HOLD_MS + 1, 136.0);
        assert_eq!(m.reading(5_000 + HOLD_MS + 1), Some(10));
    }

    #[test]
    fn a_restarted_or_lost_process_starts_a_new_interval() {
        let mut m = Meter::default();
        m.add(0, 500.0);
        m.add(5_000, 2.0); // restarted: its CPU time went back to near 0
        assert_eq!(m.reading(5_000), None);
        m.add(10_000, 7.0);
        assert_eq!(m.reading(10_000), Some(100));
        m.lost();
        m.add(15_000, 1_000.0);
        assert_eq!(
            m.reading(15_000),
            Some(100),
            "no interval spans the gap after `lost`"
        );
    }
}
