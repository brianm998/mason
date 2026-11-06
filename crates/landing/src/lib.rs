// SPDX-License-Identifier: GPL-3.0-or-later

//! Plain-`git` operations and the `direct` landing mode (Part 9, Part 11).
//!
//! Everything goes through the `git` CLI, so any remote works. The daemon does
//! all remote git work; agents never fetch or push.

pub mod direct;
pub mod git;
