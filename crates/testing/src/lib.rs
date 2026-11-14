// SPDX-License-Identifier: GPL-3.0-or-later

//! The testing system (Part 8): suites and environments, parsers for the
//! formats real test tools print, and verdicts **proven from output**.
//!
//! Everything here is pure: it is handed text and returns answers. Running
//! commands, storing evidence and leasing resources belong to the daemon.
//!
//! * [`model`]: Suite, Environment, Level, Binding, Impact map.
//! * [`parse`]: JUnit XML, TAP, xctest, Swift Testing, cargo-nextest, pytest,
//!   Gradle, Playwright, Cucumber JSON, and a regex fallback.
//! * [`verdict`]: green requires proof; red is classified (flaky,
//!   environmental, baseline, product) before anyone is blamed; closing
//!   verdicts rank worst-first.
//! * [`gates`]: negative control, assertion-weakening scan, acceptance table,
//!   quarantine.
//! * [`env`]: the lifecycle of an environment as commands to run.

pub mod basered;
pub mod env;
pub mod gates;
pub mod glob;
pub mod live;
pub mod model;
pub mod parse;
pub mod results;
pub mod signatures;
pub mod verdict;

pub use live::{Counts, LiveCount};
pub use model::{
    Binding, DeviceType, EnvKind, Environment, ImpactMap, ParserKind, Selection, Signature, Suite,
    Testing, When,
};
pub use results::{Case, Results, Status};
pub use verdict::{
    classify, closing, judge, Closing, Evidence, Inputs, RedClass, SuiteOutcome, Verdict,
};
