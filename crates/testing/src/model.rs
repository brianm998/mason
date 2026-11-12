// SPDX-License-Identifier: GPL-3.0-or-later

use crate::glob;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParserKind {
    Junit,
    Tap,
    Xctest,
    SwiftTesting,
    Nextest,
    Pytest,
    Gradle,
    Playwright,
    Cucumber,
    Regex,
}

impl ParserKind {
    pub const ALL: [ParserKind; 10] = [
        ParserKind::Junit,
        ParserKind::Tap,
        ParserKind::Xctest,
        ParserKind::SwiftTesting,
        ParserKind::Nextest,
        ParserKind::Pytest,
        ParserKind::Gradle,
        ParserKind::Playwright,
        ParserKind::Cucumber,
        ParserKind::Regex,
    ];

    pub fn name(self) -> &'static str {
        match self {
            ParserKind::Junit => "junit",
            ParserKind::Tap => "tap",
            ParserKind::Xctest => "xctest",
            ParserKind::SwiftTesting => "swift-testing",
            ParserKind::Nextest => "nextest",
            ParserKind::Pytest => "pytest",
            ParserKind::Gradle => "gradle",
            ParserKind::Playwright => "playwright",
            ParserKind::Cucumber => "cucumber",
            ParserKind::Regex => "regex",
        }
    }

    pub fn from_name(s: &str) -> Option<ParserKind> {
        ParserKind::ALL.into_iter().find(|p| p.name() == s)
    }
}

/// A command, a result parser, a minimum test count, resources, artifacts and
/// a progress source (Part 8).
#[derive(Clone, Debug, PartialEq)]
pub struct Suite {
    pub name: String,
    pub command: String,
    pub parser: ParserKind,
    /// At least this many tests must execute (and never fewer than one).
    pub min_tests: u32,
    pub level: String,
    /// Resources it holds while it runs, like `compile = 1`.
    pub uses: BTreeMap<String, u32>,
    /// The environment it runs against; empty means the local machine.
    pub env: String,
    /// Read results from this file (relative to the worktree) instead of the
    /// command's output. JUnit and Playwright usually write one.
    pub results_file: Option<String>,
    /// Re-run only the failing tests: `{tests}` becomes their names. Without
    /// it a failing suite is re-run whole when checking for flakiness.
    pub rerun: Option<String>,
    /// For the `regex` parser: lines matching `pass_regex` and `fail_regex`
    /// each count one test, named by the first capture group.
    pub pass_regex: Option<String>,
    pub fail_regex: Option<String>,
    pub timeout: Option<Duration>,
    /// Files and directories worth keeping after a run.
    pub artifacts: Vec<String>,
    /// A file the suite appends progress to, for a monitor to show.
    pub progress: Option<String>,
    /// Where its tests live (globs). Used by the negative control.
    pub test_paths: Vec<String>,
    /// Run the command with stdout and stderr on a pseudo-terminal, so a test runner
    /// that block-buffers to a pipe writes line by line. Only how output is captured.
    pub pty: bool,
}

impl Suite {
    pub fn new(name: &str, command: &str, parser: ParserKind) -> Suite {
        Suite {
            name: name.into(),
            command: command.into(),
            parser,
            min_tests: 1,
            level: String::new(),
            uses: BTreeMap::new(),
            env: String::new(),
            results_file: None,
            rerun: None,
            pass_regex: None,
            fail_regex: None,
            timeout: None,
            artifacts: vec![],
            progress: None,
            test_paths: vec![],
            pty: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnvKind {
    /// The machine itself, with optional setup, health check and teardown.
    Local,
    /// A Docker Compose stack.
    Compose,
    /// One leased simulator, emulator or phone (Stage 25).
    Device(DeviceType),
}

/// Which sort of device an environment leases. The daemon turns this into the
/// pool capability `device:<name>`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceType {
    IosSimulator,
    AndroidEmulator,
    IosDevice,
    AndroidDevice,
}

impl DeviceType {
    pub const ALL: [DeviceType; 4] = [
        DeviceType::IosSimulator,
        DeviceType::AndroidEmulator,
        DeviceType::IosDevice,
        DeviceType::AndroidDevice,
    ];

    pub fn name(self) -> &'static str {
        match self {
            DeviceType::IosSimulator => "ios-simulator",
            DeviceType::AndroidEmulator => "android-emulator",
            DeviceType::IosDevice => "ios-device",
            DeviceType::AndroidDevice => "android-device",
        }
    }

    pub fn from_name(s: &str) -> Option<DeviceType> {
        Self::ALL.into_iter().find(|t| t.name() == s)
    }
}

/// What a suite runs against, with lease, setup, health check and teardown.
/// Devices (Stage 25) lease one device per suite; the provider readies it, and
/// the commands here run on top with `MASON_DEVICE` set.
#[derive(Clone, Debug, PartialEq)]
pub struct Environment {
    pub name: String,
    pub kind: EnvKind,
    pub setup: Option<String>,
    pub health: Option<String>,
    pub teardown: Option<String>,
    /// Compose file (for `Compose`).
    pub file: Option<String>,
    /// For a device: the OS version it must start with ("17", "14").
    pub os: Option<String>,
    /// Pool resources leased while it is up.
    pub uses: BTreeMap<String, u32>,
    pub health_timeout: Duration,
}

impl Environment {
    pub fn local(name: &str) -> Environment {
        Environment {
            name: name.into(),
            kind: EnvKind::Local,
            setup: None,
            health: None,
            teardown: None,
            file: None,
            os: None,
            uses: BTreeMap::new(),
            health_timeout: Duration::from_secs(60),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum When {
    /// While the agent works (`run_check`).
    Development,
    /// In the pipeline's test step.
    TestStep,
    /// Once per landing batch.
    Batch,
    /// On the base branch after landing.
    AfterLanding,
    /// On a timer.
    Schedule,
}

impl When {
    pub fn name(self) -> &'static str {
        match self {
            When::Development => "development",
            When::TestStep => "test_step",
            When::Batch => "batch",
            When::AfterLanding => "after_landing",
            When::Schedule => "schedule",
        }
    }
    pub fn from_name(s: &str) -> Option<When> {
        [
            When::Development,
            When::TestStep,
            When::Batch,
            When::AfterLanding,
            When::Schedule,
        ]
        .into_iter()
        .find(|w| w.name() == s)
    }
}

/// When a level runs. This grid is what the monitor edits (Part 18).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding {
    pub level: String,
    pub when: BTreeSet<When>,
}

/// A pattern that marks a red as the machine's fault (no device, an expired
/// certificate), never the task's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signature {
    pub name: String,
    pub pattern: String,
    pub advice: String,
}

/// Path globs to suites. A path no glob names runs everything: the safe
/// outcome is the default.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImpactMap {
    pub entries: Vec<(String, Vec<String>)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Selection {
    /// Exactly these suites, in the order given.
    Some(Vec<String>),
    /// An unmapped path, so everything.
    All,
}

impl ImpactMap {
    /// The suites a change to `paths` needs, out of `available` (in order).
    /// No changed paths selects nothing; any unmapped path selects all.
    pub fn select(&self, paths: &[String], available: &[String]) -> Selection {
        if self.entries.is_empty() && !paths.is_empty() {
            return Selection::All;
        }
        let mut wanted: BTreeSet<&str> = BTreeSet::new();
        for p in paths {
            let mut mapped = false;
            for (g, suites) in &self.entries {
                if glob::matches(g, p) {
                    mapped = true;
                    wanted.extend(suites.iter().map(String::as_str));
                }
            }
            if !mapped {
                return Selection::All;
            }
        }
        Selection::Some(
            available
                .iter()
                .filter(|s| wanted.contains(s.as_str()))
                .cloned()
                .collect(),
        )
    }
}

/// The testing part of a definition.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Testing {
    pub suites: Vec<Suite>,
    pub environments: Vec<Environment>,
    pub bindings: Vec<Binding>,
    pub impact: ImpactMap,
    pub signatures: Vec<Signature>,
}

impl Testing {
    pub fn suite(&self, name: &str) -> Option<&Suite> {
        self.suites.iter().find(|s| s.name == name)
    }

    pub fn environment(&self, name: &str) -> Option<&Environment> {
        self.environments.iter().find(|e| e.name == name)
    }

    /// Suites whose level is bound to `when`. With no bindings at all every
    /// suite runs in the test step and nowhere else.
    pub fn bound(&self, when: When) -> Vec<&Suite> {
        if self.bindings.is_empty() {
            return if when == When::TestStep {
                self.suites.iter().collect()
            } else {
                vec![]
            };
        }
        self.suites
            .iter()
            .filter(|s| {
                self.bindings
                    .iter()
                    .any(|b| b.level == s.level && b.when.contains(&when))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map() -> ImpactMap {
        ImpactMap {
            entries: vec![
                ("src/**".into(), vec!["unit".into()]),
                ("api/**".into(), vec!["unit".into(), "contract".into()]),
                ("docs/**".into(), vec![]),
            ],
        }
    }

    fn names(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn mapped_paths_select_their_suites_in_order() {
        let avail = names(&["unit", "contract", "e2e"]);
        assert_eq!(
            map().select(&names(&["src/a.rs", "api/x.rs"]), &avail),
            Selection::Some(names(&["unit", "contract"]))
        );
        assert_eq!(
            map().select(&names(&["docs/a.md"]), &avail),
            Selection::Some(vec![]),
            "mapped to nothing on purpose"
        );
    }

    #[test]
    fn an_unmapped_path_runs_everything() {
        let avail = names(&["unit", "contract", "e2e"]);
        assert_eq!(
            map().select(&names(&["src/a.rs", "build.gradle"]), &avail),
            Selection::All
        );
        assert_eq!(
            ImpactMap::default().select(&names(&["x"]), &avail),
            Selection::All
        );
        // Nothing changed, nothing to test.
        assert_eq!(map().select(&[], &avail), Selection::Some(vec![]));
    }

    #[test]
    fn bindings_say_when_a_level_runs() {
        let mut t = Testing::default();
        let mut a = Suite::new("a", "x", ParserKind::Tap);
        a.level = "fast".into();
        let mut b = Suite::new("b", "x", ParserKind::Tap);
        b.level = "e2e".into();
        t.suites = vec![a, b];
        // No bindings: everything in the test step only.
        assert_eq!(t.bound(When::TestStep).len(), 2);
        assert!(t.bound(When::Batch).is_empty());
        t.bindings = vec![
            Binding {
                level: "fast".into(),
                when: [When::Development, When::TestStep].into(),
            },
            Binding {
                level: "e2e".into(),
                when: [When::Batch].into(),
            },
        ];
        let n = |w| {
            t.bound(w)
                .iter()
                .map(|s| s.name.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(n(When::Development), vec!["a"]);
        assert_eq!(n(When::TestStep), vec!["a"]);
        assert_eq!(n(When::Batch), vec!["b"]);
    }
}
