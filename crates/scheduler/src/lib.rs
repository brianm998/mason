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

mod explain;
mod ledger;
mod plan;
mod rank;

pub use explain::{explain, ordinal, since, Wait, WaitCause};
pub use ledger::{Ledger, Snapshot, Stall};
pub use plan::{
    plan, Capacity, CompilePace, CpuGate, DiskGate, Holder, Kind, Needs, Plan, Request, StallGate,
    World, COMPILE,
};
pub use rank::Rank;
