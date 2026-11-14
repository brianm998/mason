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
//! * [`adapters`]: tracker, git host and CI adapters that call a protocol 3 plugin;
//! * [`kit`]: the conformance kit an external plugin must pass.

pub mod adapters;
pub mod hash;
pub mod host;
pub mod kit;
pub mod manifest;
pub mod rpc;
pub mod schema;

pub use manifest::{Manifest, PluginType};

/// The newest protocol version this build speaks (2 adds `step.progress`; 3 adds adapters,
/// parsers and environments, and the `step.cancel` notification).
pub const PROTOCOL: u32 = 3;
/// Every version this build accepts in a manifest. The host speaks the manifest's.
pub const SUPPORTED: [u32; 3] = [1, 2, 3];
