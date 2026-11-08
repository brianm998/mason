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
pub mod demo;
pub mod editor;
pub mod http;
pub mod improve;
pub mod introspect;
pub mod jobs;
pub mod landing;
pub mod launchview;
pub mod numbers;
pub mod plan_view;
pub mod plugins;
pub mod pool;
pub mod progress;
pub mod prompts;
pub mod proposals;
pub mod protect;
pub mod providers;
pub mod sched;
pub mod schedule;
pub mod selfguard;
pub mod selftest;
pub mod services;
pub mod suites;
pub mod sweep;
pub mod tiermap;
pub mod tracker;
pub mod update;
pub mod usage;
pub mod views;
pub mod watchdog;
pub mod ws;

pub use daemon::Daemon;
