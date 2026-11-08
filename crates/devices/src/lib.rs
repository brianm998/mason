// SPDX-License-Identifier: GPL-3.0-or-later

//! Devices for test environments (Part 7, Part 8): turning what a provider
//! lists into pool capacity, leasing one device at a time with a kernel lock,
//! the sentence for a wait on a person, and a provider that reads a fixture.

pub mod fake;
pub mod inventory;
pub mod lease;
pub mod wait;
