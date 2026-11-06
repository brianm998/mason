// SPDX-License-Identifier: GPL-3.0-or-later

//! The definition every project starts from (layer 1 of Part 5): the graph the
//! walking skeleton ran, now as data. A project's own `.mason/mason.toml`
//! replaces it, as a whole, for the tasks admitted after it lands.

use crate::model::{AgentProfile, Definition};

pub const BUILTIN: &str = r#"schema = 1
start = "explore"
explored_entry = "develop"

[step.explore]
type = "agent"
category = "exploring"
agent = "fast"
next = "develop"

[step.develop]
type = "agent"
category = "developing"
agent = "deep"
budget = "4h"
next = "test"

[step.test]
type = "test"
category = "testing"
max_rounds = 3
on_red = "develop"
next = "land"

[step.land]
type = "landing"
category = "landing"
"#;

pub fn builtin() -> Definition {
    crate::parse(BUILTIN).expect("the built-in definition parses")
}

/// Profiles a definition can name without declaring them.
pub fn builtin_profile_names() -> &'static [&'static str] {
    &["fast", "deep"]
}

pub fn builtin_profile(name: &str) -> Option<AgentProfile> {
    match name {
        "fast" => Some(AgentProfile {
            model: Some("haiku".into()),
            effort: None,
        }),
        "deep" => Some(AgentProfile {
            model: Some("sonnet".into()),
            effort: None,
        }),
        _ => None,
    }
}

/// The profile an agent step uses when it names none.
pub fn default_profile(category: mason_core::State) -> &'static str {
    match category {
        mason_core::State::Developing => "deep",
        _ => "fast",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validate::{validate, Capabilities};

    #[test]
    fn the_builtin_definition_is_valid_and_runnable_now() {
        let d = builtin();
        let problems = validate(&d, &Capabilities::now());
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(
            d.steps.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            ["explore", "develop", "test", "land"]
        );
    }
}
