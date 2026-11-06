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
}

impl Case {
    pub fn pass(name: impl Into<String>) -> Case {
        Case {
            name: name.into(),
            status: Status::Pass,
        }
    }
    pub fn fail(name: impl Into<String>, why: impl Into<String>) -> Case {
        Case {
            name: name.into(),
            status: Status::Fail(why.into()),
        }
    }
    pub fn skip(name: impl Into<String>) -> Case {
        Case {
            name: name.into(),
            status: Status::Skip,
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
