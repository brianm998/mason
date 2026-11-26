// SPDX-License-Identifier: GPL-3.0-or-later

//! One ordering rule for every contended resource, across every project
//! (Part 7). This crate is pure: no clock, no IO. Time is a number the caller
//! passes in, and "free memory" is a number the caller measures.
//!
//! * [`Rank`] is the ordering tuple.
//! * [`plan`] decides who is granted what, all at once per request, with the
//!   passing rule.
//! * [`explain`] turns a wait into the sentence the owner reads.
//! * [`Ledger`] is the stateful wrapper the daemon holds.

mod cpu_hold;
mod deadlock;
mod explain;
mod ledger;
mod plan;
mod rank;

pub use cpu_hold::{CpuHold, MAX_GAP_MS};
pub use deadlock::{self_deadlocks, Deadlock};
pub use explain::{
    cause_class, describe_command, explain, label_parts, ordinal, since, thermal_sentence,
    thermal_short, Ahead, HeldBy, Wait, WaitCause, DESCRIPTION_LIMIT,
};
pub use ledger::{Ledger, Snapshot, Stall};
pub use plan::{
    is_gha_pool, is_runner_request, plan, Capacity, CompilePace, CpuGate, DiskGate, HeatCeiling,
    Holder, Kind, Needs, Plan, PolicyGate, Request, StallGate, ThermalGate, ThermalHold, World,
    COMPILE, GHA, GHA_DEBT, GHA_MACOS,
};
pub use rank::{Rank, RunnerKind};
