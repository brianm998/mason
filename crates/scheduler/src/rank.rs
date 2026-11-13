// SPDX-License-Identifier: GPL-3.0-or-later

/// The ordering rule, as a tuple:
///
/// ```text
/// (interactive caller first, band, progress remaining, tracker priority,
///  earliest enqueue time, task id)
/// ```
///
/// The band is 0 for a landing (a batch's verification unblocks every task in
/// it, so it goes ahead of all other pipeline work), 1 for a request that has
/// waited past `order.max_wait_s` (see [`Rank::aged`]) and 2 for everything
/// else, unless the machine orders by priority first ([`Rank::priority_first`]):
/// then it is 2 + the tracker priority, so urgent work goes ahead of work
/// nearer landing.
///
/// Progress remaining is steps remaining as a share of the shortest
/// start-to-landed path of the task's own definition, in thousandths
/// ([`Rank::with_span`]). Raw steps left are not comparable across projects:
/// a project with a longer route would sit behind another's work for good.
/// Without a span, it is the steps left times 1000, which orders the same as
/// steps left.
///
/// Derived `Ord` compares the fields in declaration order, so the rule is the
/// field order and nothing else. The fields after `id` are the inputs the
/// ordering fields are computed from; they let a rank be rebuilt for a new
/// priority.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Rank {
    /// `false` sorts first: an interactive caller goes ahead of pipeline work.
    not_interactive: bool,
    /// 0 for a landing, 1 when aged, else 2 (or 2 + priority when ordering by priority first).
    band: u8,
    /// Thousandths of the definition's shortest path still to run (0 when aged).
    remaining: u32,
    /// Lower is more urgent (0 = urgent); 0 when aged, so the oldest goes first.
    priority_key: u8,
    enqueued: u64,
    id: u64,
    // Inputs.
    steps: u32,
    span: u32,
    priority: u8,
    jump: bool,
    landing: bool,
    by_priority: bool,
    aged: bool,
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
            band: 2,
            remaining: 0,
            priority_key: priority,
            enqueued,
            id,
            steps: steps_remaining,
            span: 1,
            priority,
            jump,
            landing: false,
            by_priority: false,
            aged: false,
        }
        .recompute()
    }

    fn recompute(mut self) -> Rank {
        let interactive = self.interactive();
        let steps = if self.jump && self.priority == 0 {
            0
        } else {
            self.steps
        };
        self.remaining = steps.saturating_mul(1000) / self.span.max(1);
        self.priority_key = self.priority;
        self.band = if interactive {
            2
        } else if self.landing {
            0
        } else if self.aged {
            self.remaining = 0;
            self.priority_key = 0;
            1
        } else if self.by_priority {
            2u8.saturating_add(self.priority)
        } else {
            2
        };
        self
    }

    pub fn interactive(&self) -> bool {
        !self.not_interactive
    }

    /// Count progress as a share of `span`, the steps on the shortest
    /// start-to-landed path of the task's own definition, so that tasks of
    /// projects with routes of different length compare by how far they have
    /// come.
    pub fn with_span(mut self, span: u32) -> Rank {
        self.span = span.max(1);
        self.recompute()
    }

    /// The same rank for a landing's work: ahead of every other pipeline
    /// request, and not held back by the CPU gate or the pacing of compiles.
    pub fn landing(mut self) -> Rank {
        if !self.interactive() {
            self.landing = true;
        }
        self.recompute()
    }

    /// Is this a landing's work?
    pub fn is_landing(&self) -> bool {
        !self.interactive() && self.landing
    }

    /// The same rank with priority ahead of progress: a P0 task's request goes
    /// before a P2 task's however near landing that one is. A landing and an
    /// interactive caller keep their place.
    pub fn priority_first(mut self) -> Rank {
        self.by_priority = true;
        self.recompute()
    }

    /// The same rank for a request that has waited past `order.max_wait_s`: it
    /// goes ahead of all other pipeline work that is not a landing, oldest
    /// first. A landing and an interactive caller keep their place.
    pub fn aged(mut self) -> Rank {
        self.aged = true;
        self.recompute()
    }

    /// Is this rank aged?
    pub fn is_aged(&self) -> bool {
        !self.interactive() && !self.landing && self.aged
    }

    /// The same rank for another tracker priority, keeping everything else
    /// (arrival time, progress, landing, ordering by priority).
    pub fn with_priority(mut self, priority: u8) -> Rank {
        self.priority = priority;
        self.recompute()
    }

    /// The steps remaining the rank was built with (after any jump).
    pub fn steps(&self) -> u32 {
        if self.jump && self.priority == 0 {
            0
        } else {
            self.steps
        }
    }

    /// The tracker priority the rank was built with.
    pub fn priority(&self) -> u8 {
        self.priority
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

    #[test]
    fn progress_is_a_share_of_the_definitions_own_route() {
        // A task in `test` of a 5-step route (3 left) against a task in
        // `develop` of a 3-step route (2 left): raw steps would put the first behind.
        let long_route = Rank::new(false, 3, 2, 9, 1, false).with_span(5);
        let short_route = Rank::new(false, 2, 2, 1, 2, false).with_span(3);
        assert!(long_route < short_route);
        assert!(Rank::new(false, 3, 2, 9, 1, false) > Rank::new(false, 2, 2, 1, 2, false));
    }

    #[test]
    fn an_aged_request_goes_ahead_of_pipeline_work_oldest_first_but_not_ahead_of_a_landing() {
        let far = Rank::new(false, 5, 3, 50, 1, false).aged();
        let near_urgent = Rank::new(false, 1, 0, 1, 2, false);
        assert!(far.is_aged() && far < near_urgent);
        assert!(far.priority_first() < near_urgent.priority_first());
        let older = Rank::new(false, 9, 3, 40, 3, false).aged();
        assert!(older < far, "oldest first");
        let land = Rank::new(false, 1, 4, 99, 9, false).landing().aged();
        assert!(land.is_landing() && !land.is_aged() && land < older);
        let person = Rank::new(true, 9, 9, 99, 99, false).aged();
        assert!(!person.is_aged() && person < land);
    }

    #[test]
    fn with_priority_keeps_arrival_and_the_ordering_rule() {
        let r = Rank::new(false, 2, 2, 7, 3, false)
            .with_span(4)
            .priority_first();
        let u = r.with_priority(0);
        assert!(u < r);
        assert_eq!(
            u,
            Rank::new(false, 2, 0, 7, 3, false)
                .with_span(4)
                .priority_first()
        );
        assert_eq!(u.priority(), 0);
    }
}
