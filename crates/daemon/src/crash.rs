// SPDX-License-Identifier: GPL-3.0-or-later

//! Crash injection (Part 23): kill the daemon, uncleanly, at a named boundary.
//!
//! `MASON_CRASH_AT=<point>` or `<point>@<n>` (the n-th time it is reached).
//! The point names are listed where they are used; the crash-injection test
//! walks every one of them. With the variable unset this costs one comparison.
//!
//! `MASON_PANIC_AT=<point>` or `<point>@<n>` is the same for panics: the thread that
//! reaches the point panics (it does not die the way `kill -9` does), so the tests can
//! check that a panic is contained. Points: `coordinator` (a landing coordinator's pass),
//! `batch` (a landing batch, before it runs), `admit` (inside `admit`, after it took the
//! flag), `loop:<name>` (one pass of the background loop of that name).

use nix::sys::signal::{kill, Signal};
use nix::unistd::Pid;
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Default)]
pub struct Crash {
    target: Option<(String, u32)>,
    panic: Option<(String, u32)>,
    hits: Mutex<HashMap<String, u32>>,
}

impl Crash {
    pub fn from_env() -> Crash {
        let mut c = Crash::parse(std::env::var("MASON_CRASH_AT").ok().as_deref());
        c.panic = Self::spec(std::env::var("MASON_PANIC_AT").ok().as_deref());
        c
    }

    fn spec(spec: Option<&str>) -> Option<(String, u32)> {
        spec.filter(|s| !s.is_empty())
            .map(|s| match s.rsplit_once('@') {
                Some((name, n)) => (name.to_string(), n.parse().unwrap_or(1)),
                None => (s.to_string(), 1),
            })
    }

    pub fn parse(spec: Option<&str>) -> Crash {
        Crash {
            target: Self::spec(spec),
            panic: None,
            hits: Mutex::default(),
        }
    }

    /// A `Crash` that panics (rather than kills) at the given spec.
    pub fn parse_panic(spec: Option<&str>) -> Crash {
        Crash {
            target: None,
            panic: Self::spec(spec),
            hits: Mutex::default(),
        }
    }

    /// Would the panic point fire now? Counts hits of the targeted name only.
    pub fn should_panic(&self, point: &str) -> bool {
        let Some((name, n)) = &self.panic else {
            return false;
        };
        if name != point {
            return false;
        }
        let mut h = self.hits.lock().unwrap_or_else(|e| e.into_inner());
        let c = h.entry(format!("panic:{point}")).or_insert(0);
        *c += 1;
        *c == *n
    }

    /// Panic here, if this is the injected point.
    pub fn panic_point(&self, point: &str) {
        if self.should_panic(point) {
            panic!("PANIC INJECTED at {point}");
        }
    }

    pub fn enabled(&self) -> bool {
        self.target.is_some()
    }

    /// Would `point` fire now? Counts hits of the targeted name only.
    pub fn should_fire(&self, point: &str) -> bool {
        let Some((name, n)) = &self.target else {
            return false;
        };
        if name != point {
            return false;
        }
        let mut h = self.hits.lock().unwrap_or_else(|e| e.into_inner());
        let c = h.entry(point.to_string()).or_insert(0);
        *c += 1;
        *c == *n
    }

    /// Die here, like `kill -9`: no destructors, no flushing.
    pub fn point(&self, point: &str) {
        if self.should_fire(point) {
            eprintln!("CRASH INJECTED at {point}");
            let _ = kill(Pid::this(), Signal::SIGKILL);
            std::thread::sleep(std::time::Duration::from_secs(10)); // SIGKILL is delivered; never returns
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_fires_only_at_the_named_point_and_count() {
        let c = Crash::parse(Some("a@2"));
        assert!(!c.should_fire("b"));
        assert!(!c.should_fire("a"));
        assert!(c.should_fire("a"));
        assert!(!c.should_fire("a"), "only the n-th hit");
        assert!(!Crash::parse(None).enabled());
        assert!(!Crash::parse(Some("")).enabled());
        let d = Crash::parse(Some("p"));
        assert!(d.should_fire("p"), "default is the first hit");
        let p = Crash::parse_panic(Some("q@2"));
        assert!(!p.should_panic("q"));
        assert!(p.should_panic("q"));
        assert!(!p.should_panic("q"));
        assert!(!p.should_fire("q"), "a panic spec is not a kill spec");
    }
}
