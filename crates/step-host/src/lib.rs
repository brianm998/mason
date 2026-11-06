// SPDX-License-Identifier: GPL-3.0-or-later

//! Detached per-step supervisor (Part 2). The key decision of the design: a
//! running step survives the daemon's death.

pub mod client;
pub mod host;
pub mod launcher;
pub mod protocol;
pub mod registry;
