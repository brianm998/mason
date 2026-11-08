// SPDX-License-Identifier: GPL-3.0-or-later

//! The definition every project starts from (layer 1 of Part 5): the graph the
//! walking skeleton ran, now as data. A project's own `.mason/mason.toml`
//! replaces it, as a whole, for the tasks admitted after it lands.

use crate::model::Definition;

pub const BUILTIN: &str = r#"schema = 1
start = "explore"
explored_entry = "develop"

[step.explore]
type = "agent"
category = "exploring"
agent = "light"
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
max_rounds = 3
on_red = "develop"
"#;

pub fn builtin() -> Definition {
    crate::parse(BUILTIN).expect("the built-in definition parses")
}

/// Profiles a definition can name without declaring them: the two tiers, and
/// `fast`, another name for `light`.
pub fn builtin_profile_names() -> &'static [&'static str] {
    &["deep", "light", "fast"]
}

/// The profile an agent step uses when it names none.
pub fn default_profile(category: mason_core::State) -> &'static str {
    match category {
        mason_core::State::Developing => "deep",
        _ => "light",
    }
}

/// The next model up a retry ladder (models listed weakest to strongest) from
/// `current`. `None` when the ladder is empty, `current` is not on it, or it is
/// already the top: then nothing changes.
pub fn next_up<'a>(ladder: &'a [String], current: Option<&str>) -> Option<&'a str> {
    let at = ladder.iter().position(|m| Some(m.as_str()) == current)?;
    ladder.get(at + 1).map(String::as_str)
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

    #[test]
    fn a_ladder_moves_up_one_rung_and_stops_at_the_top() {
        let l: Vec<String> = ["small", "mid", "big"].map(String::from).to_vec();
        assert_eq!(next_up(&l, Some("small")), Some("mid"));
        assert_eq!(next_up(&l, Some("mid")), Some("big"));
        assert_eq!(next_up(&l, Some("big")), None);
        assert_eq!(next_up(&l, Some("elsewhere")), None);
        assert_eq!(next_up(&l, None), None);
        assert_eq!(
            next_up(&[], Some("small")),
            None,
            "an empty ladder changes nothing"
        );
    }
}
