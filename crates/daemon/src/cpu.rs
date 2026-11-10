// SPDX-License-Identifier: GPL-3.0-or-later

//! Overall CPU use, for the `compile` gate. The scheduler is pure and is only
//! handed a number; reading the OS happens here. The reading is an average over
//! about 15 seconds, so one spike does not hold a compile back.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Where the number comes from. Tests inject their own; nothing in them reads
/// the machine's real load.
pub trait CpuSource: Send + Sync + std::fmt::Debug {
    /// Overall CPU use, 0..=100, or `None` if it cannot be read (no gate then).
    fn busy_pct(&self) -> Option<u32>;

    /// Whether this reading alone decides when compiles start, with no count of
    /// them to set. True of the machine's own reading; a test source keeps the
    /// counts its test declared unless it asks otherwise.
    fn replaces_compile_count(&self) -> bool {
        false
    }
}

/// A fixed, settable value.
#[derive(Debug)]
pub struct FakeCpu(AtomicU32, bool);

impl FakeCpu {
    pub fn new(pct: u32) -> Arc<FakeCpu> {
        Arc::new(FakeCpu(AtomicU32::new(pct), false))
    }
    /// Behaves as the machine's own reading does: no count of compiles.
    pub fn replacing_compile_count(pct: u32) -> Arc<FakeCpu> {
        Arc::new(FakeCpu(AtomicU32::new(pct), true))
    }
    pub fn set(&self, pct: u32) {
        self.0.store(pct, Ordering::SeqCst);
    }
}

impl CpuSource for FakeCpu {
    fn busy_pct(&self) -> Option<u32> {
        Some(self.0.load(Ordering::SeqCst))
    }
    fn replaces_compile_count(&self) -> bool {
        self.1
    }
}

/// The percentage in a file, read each time (`MASON_CPU_FILE`: how the exit
/// script drives the gate against a real daemon).
#[derive(Debug)]
pub struct FileCpu(pub PathBuf);

impl CpuSource for FileCpu {
    fn busy_pct(&self) -> Option<u32> {
        std::fs::read_to_string(&self.0)
            .ok()?
            .trim()
            .parse::<u32>()
            .ok()
            .map(|n| n.min(100))
    }
}

/// Cumulative CPU time as `(busy, total)` ticks from the first line of /proc/stat.
pub fn parse_proc_stat(text: &str) -> Option<(u64, u64)> {
    let line = text.lines().next()?;
    let mut f = line.split_whitespace();
    if f.next()? != "cpu" {
        return None;
    }
    let v: Vec<u64> = f.filter_map(|x| x.parse().ok()).collect();
    if v.len() < 4 {
        return None;
    }
    let total: u64 = v.iter().take(8).sum();
    let idle = v[3] + v.get(4).copied().unwrap_or(0);
    Some((total.saturating_sub(idle), total))
}

/// Percent busy between two cumulative readings.
pub fn pct_between(a: (u64, u64), b: (u64, u64)) -> Option<u32> {
    let total = b.1.checked_sub(a.1).filter(|t| *t > 0)?;
    let busy = b.0.saturating_sub(a.0).min(total);
    Some((busy * 100 / total) as u32)
}

/// Overall busy percent from `top -l` output on macOS: `CPU usage: 5.1% user,
/// 10.2% sys, 84.7% idle`. The last such line wins (the first sample of top is
/// since boot).
pub fn parse_top_cpu(text: &str) -> Option<u32> {
    let line = text.lines().rev().find(|l| l.contains("CPU usage:"))?;
    let idle: f64 = line
        .split(',')
        .find(|p| p.contains("idle"))?
        .trim()
        .split('%')
        .next()?
        .rsplit(' ')
        .next()?
        .parse()
        .ok()?;
    Some((100.0 - idle).round().clamp(0.0, 100.0) as u32)
}

/// `top -l 2 -s 1` takes about two seconds.
pub const TOP_DEADLINE: Duration = Duration::from_secs(10);

/// A reading older than this is unknown (so no gate), not the last one for ever.
pub const MAX_AGE: Duration = Duration::from_secs(30);

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn read_instant() -> Option<u32> {
    if let Ok(t) = std::fs::read_to_string("/proc/stat") {
        // Two readings a moment apart.
        let a = parse_proc_stat(&t)?;
        std::thread::sleep(Duration::from_millis(250));
        let b = parse_proc_stat(&std::fs::read_to_string("/proc/stat").ok()?)?;
        return pct_between(a, b);
    }
    let out = mason_landing::proc::output(
        std::process::Command::new("top").args(["-l", "2", "-n", "0", "-s", "1"]),
        TOP_DEADLINE,
    )
    .ok()?;
    parse_top_cpu(&String::from_utf8_lossy(&out.stdout))
}

/// Averages the last three readings, taken every five seconds: `top` is a
/// process to start, so it is not run any more often than the gate needs.
#[derive(Debug)]
pub struct Sampler {
    pct: Arc<AtomicU32>,
    known: Arc<AtomicBool>,
    /// When the last reading was taken (unix ms).
    at: Arc<AtomicU64>,
}

/// A reading is current if it was taken no more than `max_age` before `now`.
pub fn fresh(taken_ms: u64, now_ms: u64, max_age: Duration) -> bool {
    now_ms.saturating_sub(taken_ms) <= max_age.as_millis() as u64
}

pub const NONE: u32 = u32::MAX;

pub fn average(window: &VecDeque<u32>) -> Option<u32> {
    if window.is_empty() {
        return None;
    }
    Some(window.iter().sum::<u32>() / window.len() as u32)
}

impl Sampler {
    /// Start a thread that samples for the life of the process.
    pub fn start() -> Arc<Sampler> {
        let s = Arc::new(Sampler {
            pct: Arc::new(AtomicU32::new(NONE)),
            known: Arc::new(AtomicBool::new(false)),
            at: Arc::new(AtomicU64::new(0)),
        });
        let (pct, known, at) = (s.pct.clone(), s.known.clone(), s.at.clone());
        let me = Arc::downgrade(&s);
        let _ = std::thread::Builder::new()
            .name("cpu-sampler".into())
            .spawn(move || {
                let mut window: VecDeque<u32> = VecDeque::new();
                // Ends when the daemon that owns the sampler is gone.
                while me.upgrade().is_some() {
                    let started = std::time::Instant::now();
                    if let Some(v) = read_instant() {
                        window.push_back(v);
                        while window.len() > 3 {
                            window.pop_front();
                        }
                        if let Some(a) = average(&window) {
                            pct.store(a, Ordering::SeqCst);
                            at.store(now_ms(), Ordering::SeqCst);
                            known.store(true, Ordering::SeqCst);
                        }
                    }
                    let spent = started.elapsed();
                    std::thread::sleep(Duration::from_secs(5).saturating_sub(spent));
                }
            });
        s
    }
}

impl CpuSource for Sampler {
    fn busy_pct(&self) -> Option<u32> {
        (self.known.load(Ordering::SeqCst)
            && fresh(self.at.load(Ordering::SeqCst), now_ms(), MAX_AGE))
        .then(|| self.pct.load(Ordering::SeqCst))
    }
    fn replaces_compile_count(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proc_stat_deltas() {
        let a = parse_proc_stat("cpu  100 0 100 800 0 0 0 0 0 0\ncpu0 1 2 3 4\n").unwrap();
        assert_eq!(a, (200, 1000));
        let b = parse_proc_stat("cpu  200 0 200 900 0 0 0 0 0 0\n").unwrap();
        assert_eq!(pct_between(a, b), Some(66));
        assert_eq!(pct_between(a, a), None);
        assert!(parse_proc_stat("nothing").is_none());
    }

    #[test]
    fn top_output() {
        let t = "Processes: 1\nCPU usage: 50.0% user, 10.0% sys, 40.0% idle\n\nProcesses: 2\nCPU usage: 5.1% user, 10.2% sys, 84.7% idle \n";
        assert_eq!(parse_top_cpu(t), Some(15));
        assert_eq!(parse_top_cpu("no cpu line"), None);
    }

    #[test]
    fn an_old_reading_is_unknown() {
        let now = 1_000_000;
        assert!(fresh(now - 29_000, now, MAX_AGE));
        assert!(!fresh(now - 31_000, now, MAX_AGE));
        let s = Sampler {
            pct: Arc::new(AtomicU32::new(90)),
            known: Arc::new(AtomicBool::new(true)),
            at: Arc::new(AtomicU64::new(now_ms() - 60_000)),
        };
        assert_eq!(s.busy_pct(), None);
        s.at.store(now_ms(), Ordering::SeqCst);
        assert_eq!(s.busy_pct(), Some(90));
    }

    #[test]
    fn averaging_smooths_a_spike() {
        let w: VecDeque<u32> = [10, 100, 10].into();
        assert_eq!(average(&w), Some(40));
        assert_eq!(average(&VecDeque::new()), None);
    }

    #[test]
    fn file_and_fake_sources() {
        let f = FakeCpu::new(71);
        assert_eq!(f.busy_pct(), Some(71));
        f.set(3);
        assert_eq!(f.busy_pct(), Some(3));
        let dir = std::env::temp_dir().join(format!("mason-cpu-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("cpu");
        assert_eq!(FileCpu(p.clone()).busy_pct(), None);
        std::fs::write(&p, "85\n").unwrap();
        assert_eq!(FileCpu(p).busy_pct(), Some(85));
    }
}
