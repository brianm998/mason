// SPDX-License-Identifier: GPL-3.0-or-later

//! Overall CPU use, for the `compile` gate. The scheduler is pure and is only
//! handed a number; reading the OS happens here. The reading is an average over
//! about 15 seconds, so one spike does not hold a compile back.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Where the number comes from. Tests inject their own; nothing in them reads
/// the machine's real load.
pub trait CpuSource: Send + Sync + std::fmt::Debug {
    /// Overall CPU use, 0..=100, or `None` if it cannot be read (no gate then).
    /// The machine's own sampler never answers `None`: it falls back, and
    /// at worst reports busy.
    fn busy_pct(&self) -> Option<u32>;

    /// Where the reading comes from: `mach`, `proc`, `top`, `loadavg`,
    /// `assumed-busy`, or `file`/`fake` for the test sources.
    fn source(&self) -> &'static str {
        "fake"
    }

    /// Primary reads that have failed since the daemon started.
    fn failed_reads(&self) -> u64 {
        0
    }

    /// How long the primary reading has been missing (ms), if it is.
    fn primary_missing_ms(&self) -> Option<u64> {
        None
    }

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
    fn source(&self) -> &'static str {
        "file"
    }
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

/// `top -l 2 -s 1` takes about two seconds on an idle machine, and several times
/// that under load. It is only the fallback now.
pub const TOP_DEADLINE: Duration = Duration::from_secs(10);

/// A primary reading older than this is missing. The gate then uses the
/// fallback (the load average), never "no gate".
pub const MAX_AGE: Duration = Duration::from_secs(30);

/// The primary reading missing this long raises an alert.
pub const MISSING_ALERT_MS: u64 = 120_000;

/// A failed read is logged at most this often.
pub const WARN_EVERY: Duration = Duration::from_secs(60);

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// `(busy, total)` from the macOS `host_cpu_load_info` ticks, in the order
/// user, system, idle, nice. Busy is user + system + nice.
pub fn mach_ticks(t: [u32; 4]) -> (u64, u64) {
    let [user, system, idle, nice] = t.map(u64::from);
    let busy = user + system + nice;
    (busy, busy + idle)
}

/// Cumulative ticks from `host_statistics`: a system call, no process to start.
#[cfg(target_os = "macos")]
// `mach_host_self` is marked deprecated in favour of the `mach2` crate; it is a
// system call and a new dependency is not worth it.
#[allow(unsafe_code, deprecated)]
fn read_mach_ticks() -> Result<(u64, u64), String> {
    use nix::libc;
    let mut info = libc::host_cpu_load_info { cpu_ticks: [0; 4] };
    let mut count: libc::mach_msg_type_number_t = libc::HOST_CPU_LOAD_INFO_COUNT;
    // SAFETY: `info` is a live `host_cpu_load_info` and `count` is its size in
    // integers, which is what HOST_CPU_LOAD_INFO takes and fills in.
    let kr = unsafe {
        libc::host_statistics(
            libc::mach_host_self(),
            libc::HOST_CPU_LOAD_INFO,
            (&mut info as *mut libc::host_cpu_load_info).cast(),
            &mut count,
        )
    };
    if kr != 0 {
        return Err(format!("host_statistics returned {kr}"));
    }
    let t = info.cpu_ticks;
    Ok(mach_ticks([
        t[libc::CPU_STATE_USER as usize],
        t[libc::CPU_STATE_SYSTEM as usize],
        t[libc::CPU_STATE_IDLE as usize],
        t[libc::CPU_STATE_NICE as usize],
    ]))
}

#[cfg(not(target_os = "macos"))]
fn read_mach_ticks() -> Result<(u64, u64), String> {
    Err("no mach host statistics on this system".into())
}

fn read_mach() -> Result<u32, String> {
    let a = read_mach_ticks()?;
    std::thread::sleep(Duration::from_millis(250));
    let b = read_mach_ticks()?;
    pct_between(a, b).ok_or_else(|| "mach ticks did not advance (parse error)".to_string())
}

fn read_top() -> Result<u32, String> {
    let out = mason_landing::proc::output(
        std::process::Command::new("top").args(["-l", "2", "-n", "0", "-s", "1"]),
        TOP_DEADLINE,
    )
    .map_err(|e| format!("top: {e:?}"))?;
    parse_top_cpu(&String::from_utf8_lossy(&out.stdout))
        .ok_or_else(|| "top: parse error, no CPU line".to_string())
}

/// One primary reading and where it came from, or why it failed.
pub fn read_instant() -> Result<(u32, &'static str), String> {
    if let Ok(t) = std::fs::read_to_string("/proc/stat") {
        // Two readings a moment apart.
        let a = parse_proc_stat(&t).ok_or("proc: parse error")?;
        std::thread::sleep(Duration::from_millis(250));
        let t2 = std::fs::read_to_string("/proc/stat").map_err(|e| format!("proc: {e}"))?;
        let b = parse_proc_stat(&t2).ok_or("proc: parse error")?;
        return pct_between(a, b)
            .map(|v| (v, "proc"))
            .ok_or_else(|| "proc: ticks did not advance (parse error)".to_string());
    }
    match read_mach() {
        Ok(v) => Ok((v, "mach")),
        Err(mach) => read_top()
            .map(|v| (v, "top"))
            .map_err(|top| format!("{mach}; {top}")),
    }
}

/// The load average over one minute divided by the logical CPU count, as a
/// busy percentage capped at 100. Cannot stall: two system calls.
#[cfg(windows)]
pub fn read_loadavg() -> Option<u32> {
    // Windows keeps no load average. `None` is this function's "no reading",
    // which the sampler already treats as an absent fallback.
    None
}

/// The load average over one minute divided by the logical CPU count, as a
/// busy percentage capped at 100. Cannot stall: two system calls.
#[cfg(unix)]
pub fn read_loadavg() -> Option<u32> {
    let mut l = [0f64; 1];
    // SAFETY: one element is asked for and one is provided.
    #[allow(unsafe_code)]
    let n = unsafe { nix::libc::getloadavg(l.as_mut_ptr(), 1) };
    if n < 1 {
        return None;
    }
    let cpus = std::thread::available_parallelism().ok()?.get();
    loadavg_pct(l[0], cpus)
}

/// Busy percentage from a load average and a CPU count; `None` if either is no use.
pub fn loadavg_pct(load: f64, cpus: usize) -> Option<u32> {
    if cpus == 0 || !load.is_finite() || load < 0.0 {
        return None;
    }
    Some((load * 100.0 / cpus as f64).round().min(100.0) as u32)
}

type Primary = dyn Fn() -> Result<(u32, &'static str), String> + Send + Sync;
type Fallback = dyn Fn() -> Option<u32> + Send + Sync;

const SOURCES: [&str; 3] = ["mach", "proc", "top"];

/// Averages the last three primary readings, taken every five seconds. When
/// there is no fresh one it answers from the fallback, and if that fails too it
/// says busy: a held compile costs seconds, twelve admitted at once cost an hour.
pub struct Sampler {
    pct: AtomicU32,
    known: AtomicBool,
    /// When the last primary reading was taken (unix ms).
    at: AtomicU64,
    /// Index into `SOURCES` of the last primary reading.
    primary: AtomicU32,
    failed: AtomicU64,
    started_ms: u64,
    window: Mutex<VecDeque<u32>>,
    fallback: Arc<Fallback>,
}

impl std::fmt::Debug for Sampler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sampler")
            .field("pct", &self.pct)
            .field("known", &self.known)
            .field("at", &self.at)
            .field("failed", &self.failed)
            .finish_non_exhaustive()
    }
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
    /// A sampler with this fallback and no thread: the caller feeds `ingest`.
    pub fn new(fallback: Arc<Fallback>) -> Arc<Sampler> {
        Arc::new(Sampler {
            pct: AtomicU32::new(NONE),
            known: AtomicBool::new(false),
            at: AtomicU64::new(0),
            primary: AtomicU32::new(0),
            failed: AtomicU64::new(0),
            started_ms: now_ms(),
            window: Mutex::new(VecDeque::new()),
            fallback,
        })
    }

    /// Start a thread that samples for the life of the process. The test hooks
    /// `MASON_CPU_TEST_PRIMARY_FILE` and `MASON_CPU_TEST_FALLBACK_FILE` replace
    /// the two readers with the percentage in a file (primary: a missing or
    /// unreadable file is a failed read), for the exit script.
    pub fn start() -> Arc<Sampler> {
        let hook = |name: &str| std::env::var_os(name).map(std::path::PathBuf::from);
        let fallback: Arc<Fallback> = match hook("MASON_CPU_TEST_FALLBACK_FILE") {
            Some(p) => Arc::new(move || FileCpu(p.clone()).busy_pct()),
            None => Arc::new(read_loadavg),
        };
        let primary: Arc<Primary> = match hook("MASON_CPU_TEST_PRIMARY_FILE") {
            Some(p) => Arc::new(move || {
                FileCpu(p.clone())
                    .busy_pct()
                    .map(|v| (v, "mach"))
                    .ok_or_else(|| "test hook: no primary reading".to_string())
            }),
            None => Arc::new(read_instant),
        };
        Sampler::start_with(primary, fallback)
    }

    pub fn start_with(primary: Arc<Primary>, fallback: Arc<Fallback>) -> Arc<Sampler> {
        let s = Sampler::new(fallback);
        let me = Arc::downgrade(&s);
        let _ = std::thread::Builder::new()
            .name("cpu-sampler".into())
            .spawn(move || {
                let mut last_warn: Option<std::time::Instant> = None;
                // Ends when the daemon that owns the sampler is gone.
                while let Some(s) = me.upgrade() {
                    let started = std::time::Instant::now();
                    let r = primary();
                    let elapsed = started.elapsed();
                    if let Err(cause) = s.ingest(r) {
                        if last_warn.is_none_or(|t| t.elapsed() >= WARN_EVERY) {
                            last_warn = Some(std::time::Instant::now());
                            tracing::warn!(
                                cause,
                                elapsed_ms = elapsed.as_millis() as u64,
                                failed_reads = s.failed_reads(),
                                source = s.current().1,
                                "cpu reading failed; the gate is using its fallback"
                            );
                        }
                    }
                    drop(s);
                    std::thread::sleep(Duration::from_secs(5).saturating_sub(elapsed));
                }
            });
        s
    }

    /// Take one primary reading, or count its failure and return the cause.
    pub fn ingest(&self, r: Result<(u32, &'static str), String>) -> Result<(), String> {
        match r {
            Ok((v, src)) => {
                let mut w = self.window.lock().unwrap_or_else(|e| e.into_inner());
                w.push_back(v);
                while w.len() > 3 {
                    w.pop_front();
                }
                if let Some(a) = average(&w) {
                    self.pct.store(a, Ordering::SeqCst);
                    self.at.store(now_ms(), Ordering::SeqCst);
                    let i = SOURCES.iter().position(|s| *s == src).unwrap_or(0);
                    self.primary.store(i as u32, Ordering::SeqCst);
                    self.known.store(true, Ordering::SeqCst);
                }
                Ok(())
            }
            Err(e) => {
                self.failed.fetch_add(1, Ordering::SeqCst);
                Err(e)
            }
        }
    }

    /// The busy percentage and its source: the primary reading while it is
    /// fresh, else the load average, else busy.
    pub fn current(&self) -> (u32, &'static str) {
        if self.known.load(Ordering::SeqCst)
            && fresh(self.at.load(Ordering::SeqCst), now_ms(), MAX_AGE)
        {
            let i = self.primary.load(Ordering::SeqCst) as usize;
            return (self.pct.load(Ordering::SeqCst), SOURCES[i.min(2)]);
        }
        match (self.fallback)() {
            Some(v) => (v.min(100), "loadavg"),
            None => (100, "assumed-busy"),
        }
    }

    pub fn failed_reads(&self) -> u64 {
        self.failed.load(Ordering::SeqCst)
    }
}

impl CpuSource for Sampler {
    fn busy_pct(&self) -> Option<u32> {
        Some(self.current().0)
    }
    fn replaces_compile_count(&self) -> bool {
        true
    }
    fn source(&self) -> &'static str {
        self.current().1
    }
    fn failed_reads(&self) -> u64 {
        Sampler::failed_reads(self)
    }
    fn primary_missing_ms(&self) -> Option<u64> {
        let at = self.at.load(Ordering::SeqCst);
        let since = if self.known.load(Ordering::SeqCst) {
            at
        } else {
            self.started_ms
        };
        let age = now_ms().saturating_sub(since);
        (age > MAX_AGE.as_millis() as u64).then_some(age)
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

    fn sampler(fallback: Option<u32>) -> Arc<Sampler> {
        Sampler::new(Arc::new(move || fallback))
    }

    #[test]
    fn mach_ticks_are_user_system_nice_over_idle() {
        assert_eq!(mach_ticks([10, 20, 60, 10]), (40, 100));
        let a = mach_ticks([10, 20, 60, 10]);
        let b = mach_ticks([30, 40, 100, 10]);
        assert_eq!(pct_between(a, b), Some(50));
        // No time passed, or a counter that wrapped: no reading.
        assert_eq!(pct_between(a, a), None);
        assert_eq!(
            pct_between(mach_ticks([u32::MAX, 0, 0, 0]), mach_ticks([5, 0, 0, 0])),
            None
        );
    }

    #[test]
    fn loadavg_is_a_capped_percentage() {
        assert_eq!(loadavg_pct(18.0, 36), Some(50));
        assert_eq!(loadavg_pct(72.0, 36), Some(100));
        assert_eq!(loadavg_pct(1.0, 0), None);
        assert_eq!(loadavg_pct(f64::NAN, 4), None);
    }

    #[test]
    fn no_primary_reading_uses_the_load_average_and_holds() {
        // A load of twice the core count.
        let s = sampler(loadavg_pct(72.0, 36));
        assert!(s.ingest(Err("deadline".into())).is_err());
        assert_eq!(s.busy_pct(), Some(100));
        assert_eq!(s.source(), "loadavg");
        assert_eq!(s.failed_reads(), 1);
    }

    #[test]
    fn both_readers_failing_is_busy_not_unknown() {
        let s = sampler(None);
        assert_eq!(s.busy_pct(), Some(100));
        assert_eq!(s.source(), "assumed-busy");
    }

    #[test]
    fn the_primary_wins_when_it_works_again_and_not_when_old() {
        let s = sampler(Some(100));
        assert_eq!(s.source(), "loadavg");
        s.ingest(Ok((20, "mach"))).unwrap();
        assert_eq!((s.busy_pct(), s.source()), (Some(20), "mach"));
        s.at.store(now_ms() - 60_000, Ordering::SeqCst);
        assert_eq!((s.busy_pct(), s.source()), (Some(100), "loadavg"));
        s.ingest(Ok((10, "mach"))).unwrap();
        assert_eq!((s.busy_pct(), s.source()), (Some(15), "mach"));
    }

    #[test]
    fn the_old_reading_is_old_by_max_age() {
        let now = 1_000_000;
        assert!(fresh(now - 29_000, now, MAX_AGE));
        assert!(!fresh(now - 31_000, now, MAX_AGE));
    }

    #[test]
    fn a_primary_missing_for_long_is_reported() {
        let s = sampler(Some(0));
        assert_eq!(s.primary_missing_ms(), None);
        s.ingest(Ok((1, "mach"))).unwrap();
        assert_eq!(s.primary_missing_ms(), None);
        s.at.store(now_ms() - MISSING_ALERT_MS - 1, Ordering::SeqCst);
        assert!(s.primary_missing_ms().unwrap() > MISSING_ALERT_MS);
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
