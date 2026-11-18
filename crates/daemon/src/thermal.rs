// SPDX-License-Identifier: GPL-3.0-or-later

//! How far macOS holds the CPU below full speed for heat or power, for the thermal gate.
//!
//! On the owner's iMac Pro the SMC's power-limit storms (`fSMC_PLimitNotification`, thousands in
//! the kernel log) came minutes before T2 watchdog resets on 2026-11-18 at 07:29 and 12:23, with
//! a third of the machine busy, and ten minutes after the second the speed limit sat at 27% under
//! two test suites and two compiles. Starting more work then only piles it up.
//!
//! The reading is `CPU_Speed_Limit` from `/usr/bin/pmset -g therm` (100: not limited), taken on a
//! thread of its own every few seconds, so a slow `pmset` never blocks the broker. A low reading
//! keeps counting for [`HOLD_MS`], so work does not pile back in the moment the limit lifts.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

/// A low reading keeps counting for this long after the limit lifts.
pub const HOLD_MS: u64 = 60_000;

/// No reading yet, or the reading failed.
const NONE: u32 = u32::MAX;

/// Where the number comes from. Tests inject their own.
pub trait ThermalSource: Send + Sync + std::fmt::Debug {
    /// The CPU speed limit in percent (100: full speed), or `None` if nothing is known.
    fn speed_pct(&self) -> Option<u32>;
}

/// A fixed, settable value.
#[derive(Debug)]
pub struct FakeThermal(AtomicU32);

impl FakeThermal {
    pub fn new(pct: u32) -> Arc<FakeThermal> {
        Arc::new(FakeThermal(AtomicU32::new(pct)))
    }
    pub fn set(&self, pct: u32) {
        self.0.store(pct, Ordering::SeqCst);
    }
}

impl ThermalSource for FakeThermal {
    fn speed_pct(&self) -> Option<u32> {
        Some(self.0.load(Ordering::SeqCst))
    }
}

/// `CPU_Speed_Limit` from `pmset -g therm`'s output, `None` if it is not there.
pub fn parse_speed_limit(text: &str) -> Option<u32> {
    text.lines().find_map(|l| {
        let (k, v) = l.split_once('=')?;
        (k.trim() == "CPU_Speed_Limit").then(|| v.trim().parse().ok())?
    })
}

/// The lowest reading within the last `hold_ms`, from `(ms, pct)` samples, oldest first.
pub fn held_low(samples: &VecDeque<(u64, u32)>, now_ms: u64, hold_ms: u64) -> Option<u32> {
    samples
        .iter()
        .filter(|(t, _)| now_ms.saturating_sub(*t) <= hold_ms)
        .map(|(_, p)| *p)
        .min()
}

/// The real source: `pmset` on a thread of its own.
#[derive(Debug)]
pub struct Sampler(Arc<AtomicU32>);

impl Sampler {
    /// Start sampling. Off macOS it never has a reading, so the gate never holds anything.
    pub fn start() -> Arc<Sampler> {
        let v = Arc::new(AtomicU32::new(NONE));
        #[cfg(target_os = "macos")]
        {
            let out = v.clone();
            let _ = std::thread::Builder::new()
                .name("thermal-sampler".into())
                .spawn(move || {
                    let mut samples: VecDeque<(u64, u32)> = VecDeque::new();
                    loop {
                        let now = crate::daemon::now_ms();
                        let read = std::process::Command::new("/usr/bin/pmset")
                            .args(["-g", "therm"])
                            .output()
                            .ok()
                            .and_then(|o| parse_speed_limit(&String::from_utf8_lossy(&o.stdout)));
                        if let Some(p) = read {
                            samples.push_back((now, p));
                        }
                        while samples
                            .front()
                            .is_some_and(|(t, _)| now.saturating_sub(*t) > HOLD_MS)
                        {
                            samples.pop_front();
                        }
                        out.store(
                            held_low(&samples, now, HOLD_MS).unwrap_or(NONE),
                            Ordering::SeqCst,
                        );
                        std::thread::sleep(std::time::Duration::from_secs(5));
                    }
                });
        }
        Arc::new(Sampler(v))
    }
}

impl ThermalSource for Sampler {
    fn speed_pct(&self) -> Option<u32> {
        match self.0.load(Ordering::SeqCst) {
            NONE => None,
            p => Some(p),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_speed_limit_is_read_from_pmset() {
        let t = "Note: No thermal warning level has been recorded\n\
                 Note: No performance warning level has been recorded\n\
                 2026-11-18 12:27:59 -0800 CPU Power notify\n\
                 \tCPU_Scheduler_Limit \t= 100\n\
                 \tCPU_Available_CPUs \t= 36\n\
                 \tCPU_Speed_Limit \t= 27\n";
        assert_eq!(parse_speed_limit(t), Some(27));
        assert_eq!(parse_speed_limit("nothing here\n"), None);
    }

    #[test]
    fn a_low_reading_holds_for_the_hold_time() {
        let s: VecDeque<(u64, u32)> = [(0, 40), (30_000, 100), (55_000, 100)].into();
        assert_eq!(held_low(&s, 59_000, HOLD_MS), Some(40));
        assert_eq!(held_low(&s, 61_000, HOLD_MS), Some(100));
        assert_eq!(held_low(&VecDeque::new(), 0, HOLD_MS), None);
    }
}
