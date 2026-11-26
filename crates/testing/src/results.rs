// SPDX-License-Identifier: GPL-3.0-or-later

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Pass,
    Fail(String),
    /// Not executed: skipped, ignored, pending.
    Skip,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Case {
    pub name: String,
    pub status: Status,
    /// How long it took, when the parser could read that (libtest's `--report-time`).
    pub ms: Option<u64>,
}

impl Case {
    pub fn pass(name: impl Into<String>) -> Case {
        Case {
            name: name.into(),
            status: Status::Pass,
            ms: None,
        }
    }
    pub fn fail(name: impl Into<String>, why: impl Into<String>) -> Case {
        Case {
            name: name.into(),
            status: Status::Fail(why.into()),
            ms: None,
        }
    }
    pub fn skip(name: impl Into<String>) -> Case {
        Case {
            name: name.into(),
            status: Status::Skip,
            ms: None,
        }
    }
}

/// What a parser found in a run's output.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Results {
    pub cases: Vec<Case>,
}

impl Results {
    pub fn new(cases: Vec<Case>) -> Results {
        Results { cases }
    }

    /// Each test's own time, for the store's `test_times` (s634): the cases whose output carried
    /// one. A world-slot wait was already taken off by the parser, so this is the test's work and
    /// not its queueing; a test with no time in the output is left out.
    pub fn test_times(&self) -> Vec<(String, u64)> {
        self.cases
            .iter()
            .filter_map(|c| c.ms.map(|ms| (c.name.clone(), ms)))
            .collect()
    }

    /// Cases that actually ran. A skipped test proves nothing.
    pub fn executed(&self) -> usize {
        self.cases
            .iter()
            .filter(|c| c.status != Status::Skip)
            .count()
    }

    pub fn passed(&self) -> usize {
        self.cases
            .iter()
            .filter(|c| c.status == Status::Pass)
            .count()
    }

    pub fn skipped(&self) -> usize {
        self.cases
            .iter()
            .filter(|c| c.status == Status::Skip)
            .count()
    }

    pub fn failures(&self) -> Vec<&Case> {
        self.cases
            .iter()
            .filter(|c| matches!(c.status, Status::Fail(_)))
            .collect()
    }

    pub fn failed_names(&self) -> Vec<String> {
        self.failures().iter().map(|c| c.name.clone()).collect()
    }

    pub fn names(&self) -> Vec<String> {
        self.cases.iter().map(|c| c.name.clone()).collect()
    }
}
