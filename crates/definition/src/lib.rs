// SPDX-License-Identifier: GPL-3.0-or-later

//! Pipeline definitions: the typed, versioned graph in `.mason/mason.toml`
//! (Part 5), and everything that can be said about one without running it.
//!
//! * [`parse`] reads it (every mistake is reported, with the key it is about),
//!   and [`edit`] writes it with `toml_edit` so comments and ordering survive.
//! * [`validate`] checks the rules: every path reaches a terminal state, every
//!   loop has a bound, fan-out and joins behave, every reference exists.
//! * [`layers`] merges the five configuration layers, remembering where each
//!   value came from.
//! * [`editor`] is the graphical editor's engine: operations, refusal, layout.
//! * [`dryrun`] walks scripted tasks through a graph, through the real reducer,
//!   with no model and no network.
//! * [`clarity`] and [`filing`] decide whether a task may skip exploring.

pub mod builtin;
pub mod clarity;
pub mod dryrun;
pub mod edit;
pub mod editor;
pub mod filing;
pub mod graph;
pub mod layers;
pub mod model;
pub mod parse;
mod testing_parse;
pub mod validate;

pub use model::{AgentProfile, Definition, Join, Problem, Step, StepType};
pub use parse::{parse, parse_with, Plugins};
pub use validate::{validate, Capabilities};
