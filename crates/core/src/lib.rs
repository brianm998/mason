// SPDX-License-Identifier: GPL-3.0-or-later

//! Domain types and the vocabulary of the pure state machine (no IO).
//!
//! Nothing in this crate reads the clock, touches the filesystem or the
//! network. Time arrives inside events as a [`Timestamp`].

mod entities;
mod event;
mod ids;
pub mod introspect;
pub mod notify;
pub mod progress;
pub mod progress_text;
pub mod proposal;
mod task;
pub mod usage_text;

pub use entities::*;
pub use event::*;
pub use ids::*;
pub use task::*;
