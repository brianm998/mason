// SPDX-License-Identifier: GPL-3.0-or-later

/// The ordering rule, as a tuple:
///
/// ```text
/// (interactive caller first, band, fewest steps remaining to landed,
///  tracker priority, earliest enqueue time, task id)
/// ```
///
/// The band is 0 for a landing (a batch's verification unblocks every task in
/// it, so it goes ahead of all other pipeline work) and 1 for everything else,
/// unless the machine orders by priority first ([`Rank::priority_first`]): then
/// it is 1 + the tracker priority, so urgent work goes ahead of work nearer
/// landing.
///
/// Derived `Ord` compares the fields in declaration order, so the rule is the
/// field order and nothing else.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Rank {
    /// `false` sorts first: an interactive caller goes ahead of pipeline work.
    not_interactive: bool,
    /// 0 for a landing, else 1 (or 1 + priority when ordering by priority first).
    band: u8,
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
            band: 1,
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

    /// The same rank for a landing's work: ahead of every other pipeline
    /// request, and not held back by the CPU gate or the pacing of compiles.
    pub fn landing(mut self) -> Rank {
        if !self.interactive() {
            self.band = 0;
        }
        self
    }

    /// Is this a landing's work?
    pub fn is_landing(&self) -> bool {
        !self.interactive() && self.band == 0
    }

    /// The same rank with priority ahead of progress: a P0 task's request goes
    /// before a P2 task's however near landing that one is. A landing and an
    /// interactive caller keep their place.
    pub fn priority_first(mut self) -> Rank {
        if !self.interactive() && self.band != 0 {
            self.band = 1u8.saturating_add(self.priority);
        }
        self
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
    fn a_landing_goes_ahead_of_all_pipeline_work_and_not_ahead_of_a_person() {
        let land = Rank::new(false, 1, 4, 99, 9, false).landing();
        let urgent_jump = Rank::new(false, 5, 0, 1, 1, true);
        assert!(land.is_landing());
        assert!(land < urgent_jump, "a landing unblocks the whole batch");
        assert!(land < Rank::new(false, 1, 0, 0, 0, false));
        let person = Rank::new(true, 9, 9, 99, 99, false);
        assert!(person < land);
        assert!(
            !person.landing().is_landing(),
            "a person's request is never a landing"
        );
    }

    #[test]
    fn priority_first_puts_urgent_work_ahead_of_progress_but_not_ahead_of_a_landing() {
        let near_p2 = Rank::new(false, 2, 2, 1, 1, false);
        let far_p0 = Rank::new(false, 4, 0, 9, 2, false);
        assert!(near_p2 < far_p0, "by default progress decides");
        assert!(far_p0.priority_first() < near_p2.priority_first());
        // Within a priority, progress still decides.
        let near_p0 = Rank::new(false, 2, 0, 50, 3, false).priority_first();
        assert!(near_p0 < far_p0.priority_first());
        let land = Rank::new(false, 1, 4, 99, 9, false)
            .landing()
            .priority_first();
        assert!(land.is_landing() && land < far_p0.priority_first());
        assert!(Rank::new(true, 9, 9, 9, 9, false).priority_first() < land);
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

    #[test]
    fn a_task_linked_run_ranks_as_pipeline_work_and_a_person_still_goes_first() {
        let person = Rank::new(true, 0, 0, 50, 9, false);
        let test_step = Rank::new(false, 2, 1, 10, 1, false); // task in `test`
        let linked_from_develop = Rank::new(false, 3, 1, 99, 2, false);
        assert!(person < test_step);
        assert!(test_step < linked_from_develop);
        assert!(!linked_from_develop.interactive());
        assert_eq!(linked_from_develop.steps(), 3);
    }
}
