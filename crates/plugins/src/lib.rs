// SPDX-License-Identifier: GPL-3.0-or-later

//! The plugin protocol (Part 11) and project extensions (Part 12).
//!
//! A plugin is a folder with a `manifest.toml` and a program that speaks
//! versioned JSON-RPC 2.0 over stdio, one JSON object per line. Any language
//! works. The pieces:
//!
//! * [`manifest`]: reading and checking `manifest.toml` (no I/O);
//! * [`rpc`]: building and reading the messages (no I/O);
//! * [`schema`]: the small part of JSON Schema that forms and config checks use (no I/O);
//! * [`hash`]: SHA-256 over a folder's contents, which names an approval (no I/O);
//! * [`host`]: running a plugin process, with timeouts;
//! * [`kit`]: the conformance kit an external plugin must pass.

pub mod hash;
pub mod host;
pub mod kit;
pub mod manifest;
pub mod rpc;
pub mod schema;

pub use manifest::{Manifest, PluginType};

/// The protocol version this build speaks.
pub const PROTOCOL: u32 = 1;
