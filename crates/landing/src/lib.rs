// SPDX-License-Identifier: GPL-3.0-or-later

//! Plain-`git` operations and landing (Part 9, Part 11): the `landing.toml`
//! configuration, how batches form and which tasks are to blame for a red,
//! promotion of a commit onto the base, and the single-task `direct` lander.
//!
//! Everything goes through the `git` CLI, so any remote works. The daemon does
//! all remote git work; agents never fetch or push.

pub mod audit;
pub mod batch;
pub mod config;
pub mod direct;
pub mod edit;
pub mod git;
pub mod plan;
pub mod proc;
pub mod promote;
