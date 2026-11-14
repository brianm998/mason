// SPDX-License-Identifier: GPL-3.0-or-later

//! The MASON daemon: event store, reducer, effects, jobs, landing, HTTP.
//!
//! The reducer decides what happens next; this crate does the work (git,
//! agent sessions, test runs) on bounded worker pools and reports back as new
//! events. Nothing blocks the async event loop.

pub mod attention;
pub mod basecheck;
pub mod basefold;
pub mod batch_ci;
pub mod bg;
pub mod blockers;
pub mod ci_schedule;
pub mod ci_watch;
pub mod cmdreuse;
pub mod config;
pub mod cpu;
pub mod cputasks;
pub mod crash;
pub mod daemon;
pub mod defs;
pub mod demo;
pub mod disk;
pub mod editor;
pub mod exhausted;
pub mod fdlimit;
pub mod holds;
pub mod hostarchive;
pub mod hostedcheck;
pub mod hostloss;
pub mod http;
pub mod hung;
pub mod improve;
pub mod introspect;
pub mod jobs;
pub mod knownred;
pub mod lan;
pub mod landing;
pub mod launchview;
pub mod notify;
pub mod numbers;
pub mod outside;
pub mod plan_view;
pub mod plugins;
pub mod pool;
pub mod progress;
pub mod prompts;
pub mod proposals;
pub mod protect;
pub mod providers;
pub mod rerun;
pub mod sched;
pub mod schedule;
pub mod scope;
pub mod selfguard;
pub mod selftest;
pub mod services;
pub mod ssh_keys;
pub mod stall;
pub mod stopsig;
pub mod suites;
pub mod sweep;
pub mod targets;
pub mod taskcache;
pub mod tiermap;
pub mod tracker;
pub mod update;
pub mod usage;
pub mod version;
pub mod views;
pub mod watch;
pub mod watchdog;
pub mod ws;

pub use daemon::Daemon;
