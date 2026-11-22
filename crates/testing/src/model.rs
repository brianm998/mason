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
    /// Run first, holding one `compile` slot, which it gives back before `command` runs
    /// (decision 0130, per suite). Suites with the same build command share one build.
    pub build: Option<String>,
    pub parser: ParserKind,
    /// `parser = "plugin:<name>"`: the output is read by the approved extension that provides
    /// `parser:<name>` instead of a built-in parser (`parser` above is then unused). The
    /// verdict is judged from the cases it returns exactly as for a built-in parser.
    pub plugin_parser: Option<String>,
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
    /// Run the suite without the known-red tests: `{tests}` becomes their quoted names and
    /// `{skips}` becomes `--skip 'a' --skip 'b'` (for cargo, `{project_test} -- --exact {skips}`).
    /// Without it a known-red test runs and its failure is excused instead.
    pub skip: Option<String>,
    /// What `run_check` runs while a task is developed and the agent asks for what its change
    /// touches: `{packages}` becomes the `-p` list of the packages the diff touches and their
    /// dependents. The test step never runs it; it runs `command` whole.
    pub dev_command: Option<String>,
    /// The build of `dev_command` (also with `{packages}`); without it the suite's `build` runs.
    pub dev_build: Option<String>,
    /// The suite's fast unit tier: a command (no placeholders) that runs only the quick tests, in
    /// seconds to a minute. A landing batch whose every task had its testing skipped runs it after
    /// the build floor, so a bad merge of two green tasks is caught without the full run, which
    /// stays owed. Its failing tests are named by `fail_regex` (or the output's tail).
    pub fast_command: Option<String>,
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
    /// How to run only the tests a change needs, in a task's test step (s386). The landing
    /// batch always runs `command` whole.
    pub narrow: Option<Narrow>,
    /// Set by the test step on the copy it runs narrowed: such a run is no proof the suite is
    /// green for the tree, so it records no evidence (never read from a definition).
    pub narrowed: bool,
    /// In a task's test step, run `dev_command` for the changed crates and their dependents
    /// (s396), as `run_check` does. The landing batch runs `command` whole.
    pub narrow_packages: bool,
    /// The suite runs whole again after the test step (`whole_later`): the step then narrows
    /// harder, running only `narrow_smoke` plus the picked units where it would run whole.
    pub whole_later: Option<WholeLater>,
    /// After a lost step host, rerun only the cargo test executables whose `test result:` line
    /// the lost run did not reach (#621). A suite whose `dev_command` takes `{tests_for_change}`
    /// does it without this key.
    pub resume_after_loss: bool,
    /// `remote = false` keeps the suite on this machine whatever the global switch says (#698).
    pub remote: bool,
    /// `runs_on = "macos-15"`: the runner kind when the suite runs on GitHub Actions; without
    /// it the project's `[remote_runs] runs_on`.
    pub runs_on: Option<String>,
    /// `shards = 8`: when the suite runs on GitHub Actions, split its whole `command` run across
    /// this many runners, balanced by recorded per-test times (#701). A local run ignores it.
    /// 0 and 1 mean one runner.
    pub shards: u32,
    /// What each shard's runner runs instead of `command`: it reads `.mason/run.json` and the
    /// `MASON_SHARD` variable, as `scripts/test-shard.py` does. Needed by `shards`.
    pub shard_command: Option<String>,
    /// Like `shard_command`, for a narrowed run: what each runner runs for its share of the tests
    /// `{tests_for_change}` picked, in the form of `dev_command` (#880). Without it, or when the
    /// pick is more than one whole test target, a narrowed run stays on one runner.
    pub dev_shard_command: Option<String>,
    /// `ci_jobs = ["test (ubuntu-latest, *)"]`: the CI jobs that together cover this suite
    /// (`*` matches any run of characters). When every one of them succeeded on one run of the
    /// base branch, the suite is recorded green at that run's commit (#774).
    pub ci_jobs: Vec<String>,
}

/// The `narrow_*` keys of a suite (s386): where its tests live, how one is named, and the
/// command that runs only some of them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Narrow {
    /// The test files (a glob, relative to the worktree).
    pub tests: String,
    /// For a test file that is one unit: the prefix taken off its path before the path becomes
    /// its name (`/` to `.`, extension dropped).
    pub root: Option<String>,
    /// For a file holding many units: a regex whose matches start one each, named by its first
    /// capture group (an XCTest file's `class (\w+): XCTestCase`).
    pub split: Option<String>,
    /// The command with `{tests}`, which becomes `each` once per picked unit.
    pub command: String,
    /// One unit's part of `{tests}`, with `{}` its name (`--tests {}`).
    pub each: String,
    /// A changed path matching one of these runs the suite whole (build files, shared code).
    pub never: Vec<String>,
    /// Units added to every narrowed pick of a suite with `whole_later` (`narrow_smoke`), by
    /// name, as `each` takes them.
    pub smoke: Vec<String>,
}

/// Where a suite runs whole after a task's test step (`whole_later`), so the step need not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WholeLater {
    /// In a landing tier (`batch`) of the project.
    Batch,
    /// On the base's CI, before the change lands.
    Ci,
}

impl WholeLater {
    pub fn name(self) -> &'static str {
        match self {
            WholeLater::Batch => "batch",
            WholeLater::Ci => "ci",
        }
    }

    pub fn from_name(s: &str) -> Option<WholeLater> {
        match s {
            "batch" => Some(WholeLater::Batch),
            "ci" => Some(WholeLater::Ci),
            _ => None,
        }
    }
}

impl Suite {
    pub fn new(name: &str, command: &str, parser: ParserKind) -> Suite {
        Suite {
            name: name.into(),
            command: command.into(),
            build: None,
            parser,
            plugin_parser: None,
            min_tests: 1,
            level: String::new(),
            uses: BTreeMap::new(),
            env: String::new(),
            results_file: None,
            rerun: None,
            skip: None,
            dev_command: None,
            dev_build: None,
            fast_command: None,
            pass_regex: None,
            fail_regex: None,
            timeout: None,
            artifacts: vec![],
            progress: None,
            test_paths: vec![],
            pty: false,
            narrow: None,
            narrowed: false,
            narrow_packages: false,
            whole_later: None,
            resume_after_loss: false,
            remote: true,
            runs_on: None,
            shards: 0,
            shard_command: None,
            dev_shard_command: None,
            ci_jobs: vec![],
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
    /// `type = "plugin:<name>"`: the approved extension that provides `environment:<name>`
    /// sets it up, checks its health and tears it down, instead of `setup`, `health` and
    /// `teardown` commands (`kind` is then `Local`).
    pub plugin: Option<String>,
    pub setup: Option<String>,
    pub health: Option<String>,
    pub teardown: Option<String>,
    /// Compose file (for `Compose`).
    pub file: Option<String>,
    /// For a device: the OS version it must start with ("17", "14").
    pub os: Option<String>,
    /// For a device: a glob its name must match (`*` any run, `?` one
    /// character), so a project leases only the devices meant for it. The
    /// `name` key in `[environment.*]`.
    pub device_name: Option<String>,
    /// For a device: globs on name or id it must not match, so a project
    /// keeps off devices another pipeline on the same machine uses.
    pub exclude: Vec<String>,
    /// Pool resources leased while it is up.
    pub uses: BTreeMap<String, u32>,
    pub health_timeout: Duration,
}

impl Environment {
    pub fn local(name: &str) -> Environment {
        Environment {
            name: name.into(),
            kind: EnvKind::Local,
            plugin: None,
            setup: None,
            health: None,
            teardown: None,
            file: None,
            os: None,
            device_name: None,
            exclude: vec![],
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
    /// Plain entries: each adds its suites to the union.
    pub entries: Vec<(String, Vec<String>)>,
    /// Override entries (`{ suites = [...], override = true }`): for a path
    /// they match, the most specific ones replace every less specific entry.
    pub overrides: Vec<(String, Vec<String>)>,
}

/// How specific a glob is: the number of segments with no wildcard, then the
/// length of the literal prefix before the first wildcard character.
pub fn glob_specificity(glob: &str) -> (usize, usize) {
    let g = glob.trim_start_matches("./");
    let literal = g.split('/').filter(|s| !s.contains(['*', '?'])).count();
    let prefix = g.find(['*', '?']).unwrap_or(g.len());
    (literal, prefix)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Selection {
    /// Exactly these suites, in the order given.
    Some(Vec<String>),
    /// An unmapped path, so everything.
    All,
}

impl ImpactMap {
    /// The first of `paths` no glob names: the one that makes `select` run everything.
    pub fn first_unmapped<'a>(&self, paths: &'a [String]) -> Option<&'a str> {
        if self.entries.is_empty() {
            return paths.first().map(String::as_str);
        }
        paths
            .iter()
            .find(|p| {
                !(self.entries.iter().chain(&self.overrides)).any(|(g, _)| glob::matches(g, p))
            })
            .map(String::as_str)
    }

    /// The files of `files` no glob names, grouped by top-level directory
    /// (`.` for files at the root), in order.
    pub fn unmapped_by_dir(&self, files: &[String]) -> Vec<(String, Vec<String>)> {
        let mut out: std::collections::BTreeMap<String, Vec<String>> = Default::default();
        for f in files {
            if self.first_unmapped(std::slice::from_ref(f)).is_some() {
                let top = f.split_once('/').map_or(".", |(d, _)| d).to_string();
                out.entry(top).or_default().push(f.clone());
            }
        }
        out.into_iter().collect()
    }

    /// The suites a change to `paths` needs, out of `available` (in order).
    /// No changed paths selects nothing; any unmapped path selects all.
    pub fn select(&self, paths: &[String], available: &[String]) -> Selection {
        if self.entries.is_empty() && self.overrides.is_empty() && !paths.is_empty() {
            return Selection::All;
        }
        let mut wanted: BTreeSet<&str> = BTreeSet::new();
        for p in paths {
            let mut mapped = false;
            let best = self
                .overrides
                .iter()
                .filter(|(g, _)| glob::matches(g, p))
                .map(|(g, _)| glob_specificity(g))
                .max();
            if let Some(best) = best {
                mapped = true;
                for (g, suites) in &self.overrides {
                    if glob::matches(g, p) && glob_specificity(g) == best {
                        wanted.extend(suites.iter().map(String::as_str));
                    }
                }
            }
            for (g, suites) in &self.entries {
                if glob::matches(g, p) && best.is_none_or(|b| glob_specificity(g) >= b) {
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
            overrides: vec![],
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

    fn ov(entries: &[(&str, &[&str])], overrides: &[(&str, &[&str])]) -> ImpactMap {
        let conv = |v: &[(&str, &[&str])]| {
            v.iter()
                .map(|(g, s)| (g.to_string(), names(s)))
                .collect::<Vec<_>>()
        };
        ImpactMap {
            entries: conv(entries),
            overrides: conv(overrides),
        }
    }

    #[test]
    fn an_empty_override_selects_nothing_under_a_broader_list() {
        let avail = names(&["a", "b"]);
        let m = ov(&[("scripts/**", &["a"])], &[("scripts/exit/**", &[])]);
        assert_eq!(
            m.select(&names(&["scripts/exit/s1.sh"]), &avail),
            Selection::Some(vec![])
        );
        assert_eq!(
            m.select(&names(&["scripts/x.sh"]), &avail),
            Selection::Some(names(&["a"]))
        );
    }

    #[test]
    fn an_override_list_replaces_a_broader_entry() {
        let avail = names(&["a", "b"]);
        let m = ov(&[("scripts/**", &["a"])], &[("scripts/exit/**", &["b"])]);
        assert_eq!(
            m.select(&names(&["scripts/exit/s1.sh"]), &avail),
            Selection::Some(names(&["b"]))
        );
    }

    #[test]
    fn a_plain_entry_more_specific_than_the_override_still_adds() {
        let avail = names(&["a", "b", "c"]);
        let m = ov(
            &[("scripts/**", &["a"]), ("scripts/exit/s1.sh", &["c"])],
            &[("scripts/exit/**", &["b"])],
        );
        assert_eq!(
            m.select(&names(&["scripts/exit/s1.sh"]), &avail),
            Selection::Some(names(&["b", "c"]))
        );
        assert_eq!(
            m.select(&names(&["scripts/exit/s2.sh"]), &avail),
            Selection::Some(names(&["b"]))
        );
    }

    #[test]
    fn overrides_of_equal_specificity_union() {
        let avail = names(&["a", "b"]);
        let m = ov(&[], &[("a/*.rs", &["a"]), ("a/?.rs", &["b"])]);
        assert_eq!(
            m.select(&names(&["a/x.rs"]), &avail),
            Selection::Some(names(&["a", "b"]))
        );
    }

    #[test]
    fn a_map_without_overrides_selects_as_the_union_did() {
        // Star's map, copied as a fixture.
        let m = ov(
            &[
                ("scripts/**", &["a", "b"]),
                ("scripts/exit/**", &[]),
                ("pipeline/**", &["a"]),
                ("docs/**", &[]),
            ],
            &[],
        );
        let avail = names(&["a", "b"]);
        for (path, want) in [
            ("scripts/exit/s1.sh", Selection::Some(names(&["a", "b"]))),
            ("scripts/x.sh", Selection::Some(names(&["a", "b"]))),
            ("pipeline/x.py", Selection::Some(names(&["a"]))),
            ("docs/x.md", Selection::Some(vec![])),
            ("new/x.txt", Selection::All),
        ] {
            assert_eq!(m.select(&names(&[path]), &avail), want, "{path}");
        }
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
