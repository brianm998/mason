// SPDX-License-Identifier: GPL-3.0-or-later

use mason_core::State;
use std::collections::BTreeMap;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum StepType {
    Agent,
    Command,
    Test,
    Gate,
    Approval,
    Ci,
    Pr,
    Landing,
    /// A step type a plugin provides (Stage 19). Which one is `Step::plugin`.
    /// Not in [`StepType::ALL`]: that lists the built-in types.
    Plugin,
}

impl StepType {
    pub const ALL: [StepType; 8] = [
        StepType::Agent,
        StepType::Command,
        StepType::Test,
        StepType::Gate,
        StepType::Approval,
        StepType::Ci,
        StepType::Pr,
        StepType::Landing,
    ];

    pub fn name(self) -> &'static str {
        match self {
            StepType::Agent => "agent",
            StepType::Command => "command",
            StepType::Test => "test",
            StepType::Gate => "gate",
            StepType::Approval => "approval",
            StepType::Ci => "ci",
            StepType::Pr => "pr",
            StepType::Landing => "landing",
            StepType::Plugin => "plugin",
        }
    }

    pub fn from_name(s: &str) -> Option<StepType> {
        StepType::ALL.into_iter().find(|t| t.name() == s)
    }

    /// The categories a step of this type may have.
    pub fn categories(self) -> &'static [State] {
        use State::*;
        match self {
            StepType::Agent => &[Exploring, Developing, Review],
            StepType::Command => &[Exploring, Developing, Testing, Review],
            StepType::Test => &[Testing],
            StepType::Gate => &[Testing, Review, Landing],
            StepType::Approval => &[Exploring, Developing, Review, Landing],
            StepType::Ci => &[Testing, Landing],
            StepType::Pr => &[Review, Landing],
            StepType::Landing => &[Landing],
            // The plugin's own list narrows this (see `validate`).
            StepType::Plugin => &[Exploring, Developing, Testing, Review, Landing],
        }
    }
}

/// The category names a definition may use.
pub fn category_name(s: State) -> &'static str {
    match s {
        State::Exploring => "exploring",
        State::Developing => "developing",
        State::Testing => "testing",
        State::Review => "review",
        State::Landing => "landing",
        _ => "?",
    }
}

pub fn category_from_name(s: &str) -> Option<State> {
    [
        State::Exploring,
        State::Developing,
        State::Testing,
        State::Review,
        State::Landing,
    ]
    .into_iter()
    .find(|c| category_name(*c) == s)
}

/// How far along the graph a category is (matches the reducer).
pub fn category_order(s: State) -> u8 {
    match s {
        State::Exploring => 0,
        State::Developing => 1,
        State::Testing => 2,
        State::Review => 3,
        State::Landing => 4,
        _ => 99,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Join {
    /// Wait for every branch; any red sends it back.
    AllGreen,
    /// Decide as soon as one branch is red.
    FirstRed,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    pub name: String,
    pub kind: StepType,
    pub category: State,
    /// Empty: the task ends here (landing steps). More than one: fan-out.
    pub next: Vec<String>,
    pub on_red: Option<String>,
    pub max_rounds: Option<u32>,
    pub agent: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub budget: Option<Duration>,
    pub tools: Option<Vec<String>>,
    pub read_only: Option<bool>,
    pub command: Option<String>,
    /// For a test step's command: what builds the tests, run first while the
    /// step holds one `compile` slot. The tests then run holding the step's
    /// `uses` without `compile`, so a long test run does not keep a build slot.
    pub build: Option<String>,
    /// Run the command on a pseudo-terminal (see the suite key of the same name).
    pub pty: bool,
    pub suites: Vec<String>,
    pub when: Option<String>,
    pub uses: BTreeMap<String, u32>,
    pub join: Option<Join>,
    pub policy: Option<String>,
    /// Optional gates on a test step: `negative_control`, `assertion_scan`,
    /// `acceptance` (Part 8).
    pub gates: Vec<String>,
    /// For a plugin step: the step type's name, as written in `type = "..."`.
    pub plugin: Option<String>,
    /// For a plugin step: its settings (every key the built-in steps do not use).
    pub config: BTreeMap<String, serde_json::Value>,
}

impl Step {
    /// The type as written in the definition: a plugin's own name for its steps.
    pub fn type_name(&self) -> &str {
        self.plugin.as_deref().unwrap_or(self.kind.name())
    }

    pub fn new(name: &str, kind: StepType, category: State) -> Step {
        Step {
            name: name.to_string(),
            kind,
            category,
            next: vec![],
            on_red: None,
            max_rounds: None,
            agent: None,
            model: None,
            effort: None,
            budget: None,
            tools: None,
            read_only: None,
            command: None,
            build: None,
            pty: false,
            suites: vec![],
            when: None,
            uses: BTreeMap::new(),
            join: None,
            policy: None,
            gates: vec![],
            plugin: None,
            config: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AgentProfile {
    /// `deep` or `light` (`fast` too): which tier this profile asks for.
    pub tier: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Definition {
    pub schema: u32,
    pub start: String,
    /// Where tasks that skip exploring start. Without it, tasks always explore.
    pub explored_entry: Option<String>,
    /// In file order.
    pub steps: Vec<Step>,
    pub agents: BTreeMap<String, AgentProfile>,
    pub resources: BTreeMap<String, u32>,
    /// Suites, environments, bindings, the impact map and signatures.
    pub testing: mason_testing::Testing,
    /// `[watch.ci]` when it is switched on (decision 0149); `None` is off.
    pub watch_ci: Option<mason_core::ci_watch::Rules>,
    /// Identity of the definition text it was parsed from.
    pub hash: String,
}

impl Definition {
    /// A definition with nothing in it (for a schema that needs none of its names).
    pub fn empty() -> Definition {
        Definition {
            schema: 1,
            start: String::new(),
            explored_entry: None,
            steps: vec![],
            agents: BTreeMap::new(),
            resources: BTreeMap::new(),
            testing: mason_testing::Testing::default(),
            watch_ci: None,
            hash: String::new(),
        }
    }

    pub fn step(&self, name: &str) -> Option<&Step> {
        self.steps.iter().find(|s| s.name == name)
    }

    /// The first step, in file order, with this category.
    pub fn first_in(&self, c: State) -> Option<&Step> {
        self.steps.iter().find(|s| s.category == c)
    }

    /// Where a task starts: `explored_entry` when exploring is skipped.
    pub fn entry(&self, skip_exploring: bool) -> Option<&Step> {
        match (&self.explored_entry, skip_exploring) {
            (Some(e), true) => self.step(e),
            _ => self.step(&self.start),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    /// The key it is about, like `step.test.on_red`, or empty for the whole file.
    pub path: String,
    pub message: String,
}

impl Problem {
    pub fn new(path: impl Into<String>, message: impl Into<String>) -> Problem {
        Problem {
            path: path.into(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.path.is_empty() {
            write!(f, "{}", self.message)
        } else {
            write!(f, "{}: {}", self.path, self.message)
        }
    }
}

/// A stable identity for a piece of text (FNV-1a, twice, 32 hex characters).
/// It names a definition; it is not a security hash.
pub fn hash(text: &str) -> String {
    let fnv = |seed: u64| {
        text.bytes().fold(seed, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
        })
    };
    format!(
        "{:016x}{:016x}",
        fnv(0xcbf2_9ce4_8422_2325),
        fnv(0x84222325_cbf29ce4)
    )
}

/// Parse "4h", "30m", "90s", "1h30m". `None` if it is not a duration.
pub fn parse_duration(s: &str) -> Option<Duration> {
    let mut total = 0u64;
    let mut num = String::new();
    let mut any = false;
    for c in s.trim().chars() {
        if c.is_ascii_digit() {
            num.push(c);
        } else {
            let n: u64 = num.parse().ok()?;
            num.clear();
            total += n * match c {
                'h' => 3600,
                'm' => 60,
                's' => 1,
                'd' => 86400,
                _ => return None,
            };
            any = true;
        }
    }
    (num.is_empty() && any && total > 0).then(|| Duration::from_secs(total))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_parse() {
        assert_eq!(parse_duration("4h"), Some(Duration::from_secs(14400)));
        assert_eq!(parse_duration("1h30m"), Some(Duration::from_secs(5400)));
        assert_eq!(parse_duration("90s"), Some(Duration::from_secs(90)));
        for bad in ["", "4", "h", "4x", "0s", "-1h", "1h 30"] {
            assert_eq!(parse_duration(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn the_hash_is_stable_and_sensitive() {
        assert_eq!(hash("a"), hash("a"));
        assert_ne!(hash("a"), hash("b"));
        assert_ne!(hash("a "), hash("a"));
        assert_eq!(hash("").len(), 32);
    }

    #[test]
    fn every_type_has_categories_and_round_trips_its_name() {
        for t in StepType::ALL {
            assert_eq!(StepType::from_name(t.name()), Some(t));
            assert!(!t.categories().is_empty());
        }
        assert!(StepType::from_name("magic").is_none());
        assert!(
            StepType::from_name("plugin").is_none(),
            "a plugin is never named `plugin`"
        );
        assert!(!StepType::Plugin.categories().is_empty());
    }
}
