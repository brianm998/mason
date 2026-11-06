// SPDX-License-Identifier: GPL-3.0-or-later

//! Test mode: adapters refuse any non-loopback address (Part 23). A test suite
//! once filed real tickets in a live tracker; this makes that impossible.
//!
//! Enable with [`enable_test_mode`] or `MASON_TEST_MODE=1`. Real-service runs
//! are separate, scheduled or manual, and never in test mode.

use std::io;
use std::net::{IpAddr, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};

static TEST_MODE: AtomicBool = AtomicBool::new(false);

pub fn enable_test_mode() {
    TEST_MODE.store(true, Ordering::SeqCst);
}

pub fn test_mode() -> bool {
    TEST_MODE.load(Ordering::SeqCst) || std::env::var("MASON_TEST_MODE").is_ok_and(|v| v == "1")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetError {
    NonLoopback(String),
    /// A provider endpoint must be set explicitly in test mode so that the
    /// default (real) one is never used by accident.
    EndpointRequired(&'static str),
}

impl std::fmt::Display for NetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NetError::NonLoopback(a) => write!(f, "test mode refuses non-loopback address {a:?}"),
            NetError::EndpointRequired(v) => {
                write!(f, "test mode requires {v} to be set to a loopback address")
            }
        }
    }
}

impl std::error::Error for NetError {}

/// The host part of `scheme://user@host:port/path`, `host:port`, `[::1]:80`
/// or a bare host.
fn host_of(addr: &str) -> &str {
    let rest = addr.split_once("://").map_or(addr, |(_, r)| r);
    let rest = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let rest = rest.rsplit_once('@').map_or(rest, |(_, h)| h);
    if let Some(inner) = rest.strip_prefix('[') {
        return inner.split(']').next().unwrap_or(inner);
    }
    match rest.matches(':').count() {
        1 => rest.split(':').next().unwrap_or(rest),
        _ => rest, // bare IPv6 or no port
    }
}

/// Is this address loopback? Hostnames other than `localhost` are refused,
/// because resolving them would mean trusting DNS.
pub fn is_loopback(addr: &str) -> bool {
    let h = host_of(addr);
    if h.eq_ignore_ascii_case("localhost") {
        return true;
    }
    h.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// Always `Ok` outside test mode.
pub fn check(addr: &str) -> Result<(), NetError> {
    if test_mode() && !is_loopback(addr) {
        return Err(NetError::NonLoopback(addr.to_string()));
    }
    Ok(())
}

/// Like [`check`] but also fails when no address was configured at all.
pub fn check_endpoint(var: &'static str, value: Option<&str>) -> Result<(), NetError> {
    if !test_mode() {
        return Ok(());
    }
    match value {
        None => Err(NetError::EndpointRequired(var)),
        Some(v) => check(v),
    }
}

/// `TcpStream::connect` behind the guard. Adapters that talk to a service use
/// this (or call [`check`] before their own client does).
pub fn connect(addr: &str) -> io::Result<TcpStream> {
    check(addr).map_err(|e| io::Error::new(io::ErrorKind::PermissionDenied, e))?;
    TcpStream::connect(addr)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_detection_handles_the_usual_spellings() {
        for ok in [
            "127.0.0.1",
            "127.0.0.1:8080",
            "http://127.0.0.1:9/x",
            "http://localhost:3000",
            "LOCALHOST",
            "[::1]:80",
            "::1",
            "https://user:pw@127.0.0.1:1/",
            "127.1.2.3",
        ] {
            assert!(is_loopback(ok), "{ok}");
        }
        for bad in [
            "api.anthropic.com",
            "https://api.anthropic.com/v1",
            "8.8.8.8:53",
            "10.0.0.1",
            "192.168.1.5:22",
            "[2001:db8::1]:443",
            "http://localhost.evil.com",
            "http://127.0.0.1@evil.com/",
            "0.0.0.0",
            "",
        ] {
            assert!(!is_loopback(bad), "{bad}");
        }
    }
}
