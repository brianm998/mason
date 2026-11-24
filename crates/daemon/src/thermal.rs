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
//!
//! `pmset` is not enough on that machine (task 693): through 31 h of 2026-11-18/19 its speed
//! limit stayed at 100 while the SMC power-limited the package and the die ran at 90 °C or more
//! for 647 minutes, up to 99.6 °C. So the sampler also reads the die temperature and the SMC's
//! power-limit state from the `AppleSMC` IOKit service (no root needed). Both are `None` when the
//! service or a key is missing, and then the gate behaves as it did with `pmset` alone.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

/// A low reading keeps counting for this long after the limit lifts.
pub const HOLD_MS: u64 = 60_000;

/// Power limiting must have lasted this long before it holds work.
pub const LIMIT_MS: u64 = 30_000;

/// No reading yet, or the reading failed.
const NONE: u32 = u32::MAX;

/// Where the numbers come from. Tests inject their own.
pub trait ThermalSource: Send + Sync + std::fmt::Debug {
    /// The CPU speed limit in percent (100: full speed), or `None` if nothing is known.
    fn speed_pct(&self) -> Option<u32>;
    /// The hottest CPU die reading in °C over the hold time, `None` if the SMC cannot be read.
    fn die_c(&self) -> Option<f32> {
        None
    }
    /// How long the SMC has been power-limiting the package, in ms (0: not limited), `None` if
    /// that cannot be read. It keeps its value for the hold time after the limit lifts.
    fn limited_ms(&self) -> Option<u64> {
        None
    }
}

/// A fixed, settable value.
#[derive(Debug)]
pub struct FakeThermal {
    speed: AtomicU32,
    /// Die temperature in tenths of a degree.
    die_dc: AtomicU32,
    limited_ms: AtomicU32,
}

impl FakeThermal {
    pub fn new(pct: u32) -> Arc<FakeThermal> {
        Arc::new(FakeThermal {
            speed: AtomicU32::new(pct),
            die_dc: AtomicU32::new(NONE),
            limited_ms: AtomicU32::new(NONE),
        })
    }
    pub fn set(&self, pct: u32) {
        self.speed.store(pct, Ordering::SeqCst);
    }
    /// Inject a die temperature in °C.
    pub fn set_die(&self, c: f32) {
        self.die_dc
            .store((c * 10.0).round() as u32, Ordering::SeqCst);
    }
    /// Inject how long power limiting has been on, in ms.
    pub fn set_limited_ms(&self, ms: u32) {
        self.limited_ms.store(ms, Ordering::SeqCst);
    }
}

impl ThermalSource for FakeThermal {
    fn speed_pct(&self) -> Option<u32> {
        Some(self.speed.load(Ordering::SeqCst))
    }
    fn die_c(&self) -> Option<f32> {
        match self.die_dc.load(Ordering::SeqCst) {
            NONE => None,
            d => Some(d as f32 / 10.0),
        }
    }
    fn limited_ms(&self) -> Option<u64> {
        match self.limited_ms.load(Ordering::SeqCst) {
            NONE => None,
            m => Some(u64::from(m)),
        }
    }
}

/// `CPU_Speed_Limit` from `pmset -g therm`'s output, `None` if it is not there.
pub fn parse_speed_limit(text: &str) -> Option<u32> {
    text.lines().find_map(|l| {
        let (k, v) = l.split_once('=')?;
        (k.trim() == "CPU_Speed_Limit").then(|| v.trim().parse().ok())?
    })
}

/// The macOS thermal pressure level from `notifyutil -g com.apple.system.thermalpressurelevel`
/// (0 nominal, 1 light, 2 moderate, 3 heavy, 4 sleeping). Apple Silicon Macs have no SMC die
/// key of the Intel layout this module reads, so a node there is judged by this level too.
pub fn parse_pressure_level(text: &str) -> Option<u32> {
    text.lines().find_map(|l| {
        let (k, v) = l.split_once(' ')?;
        (k.trim() == "com.apple.system.thermalpressurelevel").then(|| v.trim().parse().ok())?
    })
}

/// A pressure level at which a node takes no new work: "heavy" or worse.
pub const PRESSURE_HOT: u32 = 3;

/// The current thermal pressure level, `None` off macOS or if `notifyutil` says nothing.
pub fn pressure_level() -> Option<u32> {
    #[cfg(target_os = "macos")]
    {
        let o = std::process::Command::new("/usr/bin/notifyutil")
            .args(["-g", "com.apple.system.thermalpressurelevel"])
            .output()
            .ok()?;
        parse_pressure_level(&String::from_utf8_lossy(&o.stdout))
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// The lowest reading within the last `hold_ms`, from `(ms, pct)` samples, oldest first.
pub fn held_low(samples: &VecDeque<(u64, u32)>, now_ms: u64, hold_ms: u64) -> Option<u32> {
    samples
        .iter()
        .filter(|(t, _)| now_ms.saturating_sub(*t) <= hold_ms)
        .map(|(_, p)| *p)
        .min()
}

/// Why the machine counts as hot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hot {
    /// `pmset`'s speed limit, in percent, is under the minimum.
    Speed(u32),
    /// The die, in whole °C, is at or over the maximum.
    Die(u32),
    /// The SMC has power-limited the package for this many ms.
    Limited(u64),
}

impl Hot {
    /// The words for a wait: "die 97 °C", "power limited".
    pub fn why(&self) -> String {
        match self {
            Hot::Speed(p) => format!("CPU held to {p}% of its speed"),
            Hot::Die(c) => format!("die {c} °C"),
            Hot::Limited(_) => "power limited".into(),
        }
    }
}

/// The combined decision. Hot when the speed limit is under `min_pct` (0: ignore it), the die is
/// at or over `max_c` (0: ignore the die and the power limit), or power limiting has lasted
/// [`LIMIT_MS`]. A missing reading never makes it hot, so with only `pmset` this is the old gate.
pub fn hot(
    speed_pct: Option<u32>,
    die_c: Option<f32>,
    limited_ms: Option<u64>,
    min_pct: u32,
    max_c: u32,
) -> Option<Hot> {
    if let Some(p) = speed_pct.filter(|p| min_pct > 0 && *p < min_pct) {
        return Some(Hot::Speed(p));
    }
    if max_c == 0 {
        return None;
    }
    if let Some(c) = die_c.filter(|c| *c >= max_c as f32) {
        return Some(Hot::Die(c.round() as u32));
    }
    limited_ms.filter(|m| *m >= LIMIT_MS).map(Hot::Limited)
}

/// What the sampler knows now, with the hold applied.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Reading {
    pub speed_pct: Option<u32>,
    pub die_c: Option<f32>,
    pub limited_ms: Option<u64>,
}

/// The samples behind a [`Reading`]. Pure: the sampler thread feeds it the clock.
#[derive(Debug, Default)]
pub struct Tracker {
    speed: VecDeque<(u64, u32)>,
    die: VecDeque<(u64, f32)>,
    limited_since: Option<u64>,
    limited_last: Option<u64>,
    limit_known: bool,
}

impl Tracker {
    /// Add one round of readings; `None` means that reading failed.
    pub fn update(
        &mut self,
        now: u64,
        speed: Option<u32>,
        die: Option<f32>,
        limited: Option<bool>,
    ) {
        if let Some(p) = speed {
            self.speed.push_back((now, p));
        }
        if let Some(c) = die {
            self.die.push_back((now, c));
        }
        while self
            .speed
            .front()
            .is_some_and(|(t, _)| now.saturating_sub(*t) > HOLD_MS)
        {
            self.speed.pop_front();
        }
        while self
            .die
            .front()
            .is_some_and(|(t, _)| now.saturating_sub(*t) > HOLD_MS)
        {
            self.die.pop_front();
        }
        if let Some(l) = limited {
            self.limit_known = true;
            if l {
                self.limited_since.get_or_insert(now);
                self.limited_last = Some(now);
            }
        }
        // An episode ends once the limit has been off for the hold time.
        if self
            .limited_last
            .is_some_and(|t| now.saturating_sub(t) > HOLD_MS)
        {
            self.limited_since = None;
            self.limited_last = None;
        }
    }

    pub fn reading(&self, now: u64) -> Reading {
        Reading {
            speed_pct: held_low(&self.speed, now, HOLD_MS),
            die_c: self
                .die
                .iter()
                .filter(|(t, _)| now.saturating_sub(*t) <= HOLD_MS)
                .map(|(_, c)| *c)
                .fold(None, |m: Option<f32>, c| Some(m.map_or(c, |m| m.max(c)))),
            limited_ms: self
                .limit_known
                .then(|| match (self.limited_since, self.limited_last) {
                    (Some(s), Some(l)) => l.saturating_sub(s),
                    _ => 0,
                }),
        }
    }
}

/// `sp78`: signed 8.8 fixed point, big-endian (the SMC's temperatures).
pub fn decode_sp78(b: [u8; 2]) -> f32 {
    f32::from(i16::from_be_bytes(b)) / 256.0
}

/// `fpe2`: unsigned 14.2 fixed point, big-endian.
pub fn decode_fpe2(b: [u8; 2]) -> f32 {
    f32::from(u16::from_be_bytes(b)) / 4.0
}

/// `flt`: a 32-bit float in the machine's byte order (little-endian on the Intel Macs).
pub fn decode_flt(b: [u8; 4]) -> f32 {
    f32::from_le_bytes(b)
}

/// An SMC value of the given four-character type as a number, `None` for a type or length
/// not handled.
pub fn decode_smc(ty: &[u8; 4], bytes: &[u8]) -> Option<f32> {
    match (ty, bytes) {
        (b"sp78", [a, b, ..]) => Some(decode_sp78([*a, *b])),
        (b"fpe2", [a, b, ..]) => Some(decode_fpe2([*a, *b])),
        (b"flt ", [a, b, c, d, ..]) => Some(decode_flt([*a, *b, *c, *d])),
        _ => None,
    }
}

/// The package is power-limited when its actual power zone limit has been pulled below the
/// nominal one. Both from `PZ1E`/`PZ1F`; see `smc::read_power_limited` for what is unverified.
pub fn is_power_limited(limit: f32, nominal: f32) -> bool {
    nominal > 0.0 && limit < nominal * 0.99
}

/// The 80-byte `SMCKeyData_t` that `AppleSMC` method 2 takes and returns, built by offset so
/// that no struct layout is assumed: key at 0, key info (size at 28, type at 32) , `data8`
/// (the command) at 42, value bytes at 48.
pub mod wire {
    pub const SIZE: usize = 80;
    pub const READ_KEY_INFO: u8 = 9;
    pub const READ_BYTES: u8 = 5;

    pub fn key_u32(key: &[u8; 4]) -> u32 {
        u32::from_be_bytes(*key)
    }

    /// The request for `cmd` on `key`; `size` is the value's size, needed to read its bytes.
    pub fn request(key: &[u8; 4], cmd: u8, size: u32) -> [u8; SIZE] {
        let mut b = [0u8; SIZE];
        b[0..4].copy_from_slice(&key_u32(key).to_ne_bytes());
        b[28..32].copy_from_slice(&size.to_ne_bytes());
        b[42] = cmd;
        b
    }

    /// `(size, type)` from a key-info reply, `None` if the SMC reported an error or no value.
    pub fn key_info(reply: &[u8; SIZE]) -> Option<(u32, [u8; 4])> {
        let size = u32::from_ne_bytes(reply[28..32].try_into().ok()?);
        let ty = u32::from_ne_bytes(reply[32..36].try_into().ok()?).to_be_bytes();
        (reply[40] == 0 && size > 0 && size <= 32).then_some((size, ty))
    }

    /// The value bytes of a read reply, `None` on an SMC error.
    pub fn value(reply: &[u8; SIZE], size: u32) -> Option<&[u8]> {
        (reply[40] == 0 && size <= 32).then(|| &reply[48..48 + size as usize])
    }
}

#[cfg(target_os = "macos")]
mod smc {
    use super::{decode_smc, is_power_limited, wire};
    use std::ffi::{c_char, c_void};

    #[link(name = "IOKit", kind = "framework")]
    extern "C" {
        fn IOServiceMatching(name: *const c_char) -> *mut c_void;
        fn IOServiceGetMatchingService(main_port: u32, matching: *mut c_void) -> u32;
        fn IOServiceOpen(service: u32, owning_task: u32, ty: u32, conn: *mut u32) -> i32;
        fn IOServiceClose(conn: u32) -> i32;
        fn IOObjectRelease(obj: u32) -> i32;
        fn IOConnectCallStructMethod(
            conn: u32,
            selector: u32,
            input: *const c_void,
            input_size: usize,
            output: *mut c_void,
            output_size: *mut usize,
        ) -> i32;
    }

    /// An open connection to the `AppleSMC` service.
    pub struct Smc(u32);

    impl Smc {
        #[allow(unsafe_code, deprecated)]
        pub fn open() -> Option<Smc> {
            // SAFETY: the name is a NUL-terminated string; IOServiceMatching returns a dictionary
            // that IOServiceGetMatchingService consumes; `conn` is a live u32 that
            // IOServiceOpen fills in. The service object is released once opened.
            unsafe {
                let m = IOServiceMatching(c"AppleSMC".as_ptr());
                if m.is_null() {
                    return None;
                }
                let svc = IOServiceGetMatchingService(0, m);
                if svc == 0 {
                    return None;
                }
                let mut conn = 0u32;
                let kr = IOServiceOpen(svc, nix::libc::mach_task_self(), 0, &mut conn);
                IOObjectRelease(svc);
                (kr == 0 && conn != 0).then_some(Smc(conn))
            }
        }

        #[allow(unsafe_code)]
        fn call(&self, req: &[u8; wire::SIZE]) -> Option<[u8; wire::SIZE]> {
            let mut out = [0u8; wire::SIZE];
            let mut n = wire::SIZE;
            // SAFETY: both buffers are live, wire::SIZE bytes (the SMCKeyData_t size) and `n`
            // says so; selector 2 is the SMC's struct method.
            let kr = unsafe {
                IOConnectCallStructMethod(
                    self.0,
                    2,
                    req.as_ptr().cast(),
                    wire::SIZE,
                    out.as_mut_ptr().cast(),
                    &mut n,
                )
            };
            (kr == 0).then_some(out)
        }

        /// A key as a number, `None` if it is missing or of a type not handled.
        pub fn read(&self, key: &[u8; 4]) -> Option<f32> {
            let info = self.call(&wire::request(key, wire::READ_KEY_INFO, 0))?;
            let (size, ty) = wire::key_info(&info)?;
            let r = self.call(&wire::request(key, wire::READ_BYTES, size))?;
            decode_smc(&ty, wire::value(&r, size)?)
        }

        /// The CPU die in °C: the PECI die key, else the hottest core key.
        pub fn die_c(&self) -> Option<f32> {
            let ok = |c: &f32| (1.0..150.0).contains(c);
            self.read(b"TCXc").filter(ok).or_else(|| {
                (b'0'..=b'9')
                    .filter_map(|d| self.read(&[b'T', b'C', d, b'c']).filter(ok))
                    .fold(None, |m: Option<f32>, c| Some(m.map_or(c, |m| m.max(c))))
            })
        }

        /// Whether the package is power-limited. `PZ1E` and `PZ1F` are the package power zone's
        /// limit and actual values; which of them tracks powermetrics' `CPU Plimit` was not
        /// confirmed against the owner's `sudo powermetrics --samplers smc` output.
        pub fn power_limited(&self) -> Option<bool> {
            let limit = self.read(b"PZ1E")?;
            let nominal = self.read(b"PZ1F")?;
            Some(is_power_limited(limit, nominal))
        }
    }

    impl Drop for Smc {
        #[allow(unsafe_code)]
        fn drop(&mut self) {
            // SAFETY: `self.0` is the connection IOServiceOpen returned, closed once.
            unsafe {
                IOServiceClose(self.0);
            }
        }
    }
}

/// The real source: `pmset` and the SMC, on a thread of its own.
#[derive(Debug)]
pub struct Sampler(Arc<Mutex<Reading>>);

impl Sampler {
    /// Start sampling. Off macOS it never has a reading, so the gate never holds anything.
    pub fn start() -> Arc<Sampler> {
        let v = Arc::new(Mutex::new(Reading::default()));
        #[cfg(target_os = "macos")]
        {
            let out = v.clone();
            let _ = std::thread::Builder::new()
                .name("thermal-sampler".into())
                .spawn(move || {
                    let mut tracker = Tracker::default();
                    let mut smc = smc::Smc::open();
                    loop {
                        let now = crate::daemon::now_ms();
                        let speed = std::process::Command::new("/usr/bin/pmset")
                            .args(["-g", "therm"])
                            .output()
                            .ok()
                            .and_then(|o| parse_speed_limit(&String::from_utf8_lossy(&o.stdout)));
                        if smc.is_none() {
                            smc = smc::Smc::open();
                        }
                        let die = smc.as_ref().and_then(|s| s.die_c());
                        let limited = smc.as_ref().and_then(|s| s.power_limited());
                        tracker.update(now, speed, die, limited);
                        if let Ok(mut r) = out.lock() {
                            *r = tracker.reading(now);
                        }
                        std::thread::sleep(std::time::Duration::from_secs(5));
                    }
                });
        }
        Arc::new(Sampler(v))
    }

    fn get(&self) -> Reading {
        self.0.lock().map(|r| *r).unwrap_or_default()
    }
}

impl ThermalSource for Sampler {
    fn speed_pct(&self) -> Option<u32> {
        self.get().speed_pct
    }
    fn die_c(&self) -> Option<f32> {
        self.get().die_c
    }
    fn limited_ms(&self) -> Option<u64> {
        self.get().limited_ms
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
    fn the_pressure_level_is_read_from_notifyutil() {
        assert_eq!(
            parse_pressure_level("com.apple.system.thermalpressurelevel 1\n"),
            Some(1)
        );
        assert_eq!(
            parse_pressure_level("com.apple.system.thermalpressurelevel 3"),
            Some(3)
        );
        assert_eq!(parse_pressure_level("other 3\n"), None);
        assert_eq!(parse_pressure_level(""), None);
    }

    #[test]
    fn a_low_reading_holds_for_the_hold_time() {
        let s: VecDeque<(u64, u32)> = [(0, 40), (30_000, 100), (55_000, 100)].into();
        assert_eq!(held_low(&s, 59_000, HOLD_MS), Some(40));
        assert_eq!(held_low(&s, 61_000, HOLD_MS), Some(100));
        assert_eq!(held_low(&VecDeque::new(), 0, HOLD_MS), None);
    }
}
