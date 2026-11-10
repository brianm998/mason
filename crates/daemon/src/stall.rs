// SPDX-License-Identifier: GPL-3.0-or-later

//! How slowly the build disk answers, for the `compile` gate. Creating a file is
//! the call that blocks when a volume is overloaded: on an external SSD it has been
//! measured stuck for 20 to 220 seconds while reads and other disks stayed quick.
//! CPU use cannot see this, because everything waiting on the disk is idle.
//!
//! The scheduler is pure and is only handed a number; the file work happens here,
//! on its own thread per path, so a stuck disk never blocks the broker.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A slow answer keeps counting for this long, so compiles do not pile back in
/// the moment a stall ends.
pub const HOLD: Duration = Duration::from_secs(60);

const EVERY: Duration = Duration::from_secs(2);

/// Where the number comes from. Tests inject their own.
pub trait StallSource: Send + Sync + std::fmt::Debug {
    /// The slowest recent answer in milliseconds, or `None` if nothing is known.
    fn slow_ms(&self) -> Option<u64>;
}

/// A fixed, settable value.
#[derive(Debug)]
pub struct FakeStall(AtomicU64);

impl FakeStall {
    pub fn new(ms: u64) -> Arc<FakeStall> {
        Arc::new(FakeStall(AtomicU64::new(ms)))
    }
    pub fn set(&self, ms: u64) {
        self.0.store(ms, Ordering::SeqCst);
    }
}

impl StallSource for FakeStall {
    fn slow_ms(&self) -> Option<u64> {
        Some(self.0.load(Ordering::SeqCst))
    }
}

/// One path's recent answers. Pure: the times are passed in.
#[derive(Debug, Default)]
struct Window {
    in_flight: Option<Instant>,
    slowest: Option<(Instant, u64)>,
}

impl Window {
    fn begin(&mut self, now: Instant) {
        self.in_flight = Some(now);
    }

    fn end(&mut self, now: Instant) {
        let Some(began) = self.in_flight.take() else {
            return;
        };
        let ms = now.saturating_duration_since(began).as_millis() as u64;
        let keep_old = self
            .slowest
            .is_some_and(|(at, old)| now.saturating_duration_since(at) < HOLD && old > ms);
        if !keep_old {
            self.slowest = Some((now, ms));
        }
    }

    /// A call still stuck counts for as long as it has been stuck.
    fn reading(&self, now: Instant) -> Option<u64> {
        let stuck = self
            .in_flight
            .map(|began| now.saturating_duration_since(began).as_millis() as u64);
        let recent = self
            .slowest
            .filter(|(at, _)| now.saturating_duration_since(*at) < HOLD)
            .map(|(_, ms)| ms);
        stuck.max(recent)
    }
}

/// Creates and removes a small file in each path, every two seconds.
#[derive(Debug)]
pub struct Prober {
    windows: Vec<Arc<Mutex<Window>>>,
}

fn lock(w: &Mutex<Window>) -> std::sync::MutexGuard<'_, Window> {
    w.lock().unwrap_or_else(|e| e.into_inner())
}

/// A failure (a read-only or missing directory) is as quick as a success, and is
/// not a stall.
fn touch(path: &std::path::Path) {
    if std::fs::write(path, b"x").is_ok() {
        let _ = std::fs::remove_file(path);
    }
}

impl Prober {
    /// Start one thread per path, for the life of the returned value.
    pub fn start(mut paths: Vec<PathBuf>) -> Arc<Prober> {
        paths.sort();
        paths.dedup();
        let windows: Vec<_> = paths
            .iter()
            .map(|_| Arc::new(Mutex::new(Window::default())))
            .collect();
        let p = Arc::new(Prober {
            windows: windows.clone(),
        });
        for (dir, window) in paths.into_iter().zip(windows) {
            let me = Arc::downgrade(&p);
            let file = dir.join(format!(".mason-stall-probe-{}", std::process::id()));
            let _ = std::thread::Builder::new()
                .name("stall-probe".into())
                .spawn(move || {
                    // Ends when the daemon that owns the prober is gone.
                    while me.upgrade().is_some() {
                        lock(&window).begin(Instant::now());
                        touch(&file);
                        lock(&window).end(Instant::now());
                        std::thread::sleep(EVERY);
                    }
                });
        }
        p
    }
}

impl StallSource for Prober {
    fn slow_ms(&self) -> Option<u64> {
        let now = Instant::now();
        self.windows
            .iter()
            .filter_map(|w| lock(w).reading(now))
            .max()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slow_answer_counts_until_the_hold_ends() {
        let t = Instant::now();
        let mut w = Window::default();
        assert_eq!(w.reading(t), None);
        w.begin(t);
        w.end(t + Duration::from_secs(21));
        let done = t + Duration::from_secs(21);
        assert_eq!(w.reading(done + Duration::from_secs(59)), Some(21_000));
        assert_eq!(w.reading(done + HOLD), None);
    }

    #[test]
    fn a_call_still_stuck_counts_while_it_is_stuck() {
        let t = Instant::now();
        let mut w = Window::default();
        w.begin(t);
        assert_eq!(w.reading(t + Duration::from_secs(5)), Some(5_000));
    }

    #[test]
    fn a_quick_answer_does_not_hide_a_recent_slow_one() {
        let t = Instant::now();
        let mut w = Window::default();
        w.begin(t);
        w.end(t + Duration::from_secs(30));
        let later = t + Duration::from_secs(40);
        w.begin(later);
        w.end(later);
        assert_eq!(w.reading(later), Some(30_000));
    }

    #[test]
    fn the_prober_reads_a_real_directory() {
        let dir = std::env::temp_dir().join(format!("mason-stall-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = Prober::start(vec![dir.clone(), dir.clone()]);
        assert_eq!(p.windows.len(), 1, "the same path is probed once");
        let end = Instant::now() + Duration::from_secs(10);
        while p.slow_ms().is_none() {
            assert!(Instant::now() < end, "no reading");
            std::thread::sleep(Duration::from_millis(10));
        }
        drop(p);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
