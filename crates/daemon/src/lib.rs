// SPDX-License-Identifier: GPL-3.0-or-later

//! The MASON daemon: event store, reducer, effects, jobs, landing, HTTP.
//!
//! The reducer decides what happens next; this crate does the work (git,
//! agent sessions, test runs) on bounded worker pools and reports back as new
//! events. Nothing blocks the async event loop.

pub mod attention;
pub mod config;
pub mod crash;
pub mod daemon;
pub mod defs;
pub mod http;
pub mod jobs;
pub mod pool;
pub mod prompts;
pub mod sched;
pub mod suites;
pub mod sweep;
pub mod watchdog;

pub use daemon::Daemon;
