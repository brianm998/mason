// SPDX-License-Identifier: GPL-3.0-or-later

//! The pure reducer: `(world, event) -> (world', effects)`.
//!
//! No IO, no clock, no randomness. Ids are allocated from counters inside the
//! world, so replaying the same events always gives the same world.

mod reducer;
mod world;

pub use reducer::{reduce, reduce_in_place, ReduceError, Touched};
pub use world::{world_clones, World};
