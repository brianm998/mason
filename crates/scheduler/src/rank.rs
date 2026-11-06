// SPDX-License-Identifier: GPL-3.0-or-later

/// The ordering rule, as a tuple:
///
/// ```text
/// (interactive caller first, fewest steps remaining to landed,
///  tracker priority, earliest enqueue time, task id)
/// ```
///
/// Derived `Ord` compares the fields in declaration order, so the rule is the
/// field order and nothing else.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Rank {
    /// `false` sorts first: an interactive caller goes ahead of pipeline work.
    not_interactive: bool,
    /// Shortest graph path to landed, ignoring loops.
    steps: u32,
    /// Lower is more urgent (0 = urgent).
    priority: u8,
    enqueued: u64,
    id: u64,
}

impl Rank {
    /// `jump` is a project's opt-in to let top-priority work (priority 0) skip
    /// the progress rule. Off by default.
    pub fn new(
        interactive: bool,
        steps_remaining: u32,
        priority: u8,
        enqueued: u64,
        id: u64,
        jump: bool,
    ) -> Rank {
        Rank {
            not_interactive: !interactive,
            steps: if jump && priority == 0 {
                0
            } else {
                steps_remaining
            },
            priority,
            enqueued,
            id,
        }
    }

    pub fn interactive(&self) -> bool {
        !self.not_interactive
    }

    /// The steps remaining the rank was built with (after any jump).
    pub fn steps(&self) -> u32 {
        self.steps
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(i: bool, s: u32, p: u8, e: u64, id: u64) -> Rank {
        Rank::new(i, s, p, e, id, false)
    }

    #[test]
    fn each_term_beats_the_ones_after_it() {
        assert!(r(true, 9, 3, 99, 99) < r(false, 1, 0, 0, 0));
        assert!(r(false, 1, 3, 99, 99) < r(false, 2, 0, 0, 0));
        assert!(r(false, 2, 0, 99, 99) < r(false, 2, 1, 0, 0));
        assert!(r(false, 2, 1, 5, 99) < r(false, 2, 1, 6, 0));
        assert!(r(false, 2, 1, 5, 1) < r(false, 2, 1, 5, 2));
    }

    #[test]
    fn urgent_work_skips_the_progress_rule_only_when_the_project_opts_in() {
        let urgent_far = |jump| Rank::new(false, 5, 0, 1, 1, jump);
        let normal_near = Rank::new(false, 1, 2, 1, 2, false);
        assert!(normal_near < urgent_far(false));
        assert!(urgent_far(true) < normal_near);
        // Only priority 0 jumps.
        assert!(Rank::new(false, 5, 1, 1, 1, true) > normal_near);
    }
}
