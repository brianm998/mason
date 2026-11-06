// SPDX-License-Identifier: GPL-3.0-or-later

use std::fmt;

macro_rules! id {
    ($($name:ident),* $(,)?) => {$(
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(pub u64);
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }
    )*};
}

id!(
    ProjectId, TaskId, StepRunId, DecisionId, VerdictId, BatchId, ProposalId, AlertId, OverrideId,
    DebtId
);

/// Milliseconds since the Unix epoch, supplied by whoever creates an event.
/// The core never reads a clock.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(pub u64);
