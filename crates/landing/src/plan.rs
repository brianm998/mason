// SPDX-License-Identifier: GPL-3.0-or-later

//! The pure decisions of batched landing (Part 9): which ready tasks form a
//! batch, which batches may run side by side, which tasks a red points at, and
//! how bisection narrows it. No git, no clock: everything is passed in.

use std::collections::BTreeSet;
use std::time::Duration;

/// A task waiting to land.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub id: u64,
    /// Smaller lands first (priority, then age).
    pub rank: u64,
    /// When it joined the queue, in seconds on any steady clock.
    pub ready_at: u64,
    pub footprint: Footprint,
}

/// What a task (or batch) touches: the paths it changes and the suites those
/// changes impact.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Footprint {
    pub files: BTreeSet<String>,
    pub suites: BTreeSet<String>,
}

impl Footprint {
    pub fn new<'a>(
        files: impl IntoIterator<Item = &'a str>,
        suites: impl IntoIterator<Item = &'a str>,
    ) -> Self {
        Footprint {
            files: files.into_iter().map(String::from).collect(),
            suites: suites.into_iter().map(String::from).collect(),
        }
    }

    pub fn overlaps(&self, other: &Footprint) -> bool {
        !self.files.is_disjoint(&other.files) || !self.suites.is_disjoint(&other.suites)
    }

    pub fn union(&self, other: &Footprint) -> Footprint {
        Footprint {
            files: self.files.union(&other.files).cloned().collect(),
            suites: self.suites.union(&other.suites).cloned().collect(),
        }
    }
}

/// What the queue should do next.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Formation {
    /// Nothing is waiting.
    Empty,
    /// Collecting: look again in this long (or when another task joins).
    Wait(Duration),
    /// Land these, in order, as one batch.
    Form(Vec<u64>),
}

/// Form a batch from the waiting tasks. The window runs from the oldest
/// waiting task; a full batch (`max_size`, 0 for no cap) goes at once. Tasks
/// are taken by rank, then id.
pub fn form_batch(waiting: &[Candidate], now: u64, window: Duration, max_size: usize) -> Formation {
    if waiting.is_empty() {
        return Formation::Empty;
    }
    let oldest = waiting.iter().map(|c| c.ready_at).min().unwrap_or(now);
    let due = oldest.saturating_add(window.as_secs());
    let full = max_size > 0 && waiting.len() >= max_size;
    if !full && now < due {
        return Formation::Wait(Duration::from_secs(due - now));
    }
    let mut v: Vec<&Candidate> = waiting.iter().collect();
    v.sort_by_key(|c| (c.rank, c.id));
    let take = if max_size == 0 {
        v.len()
    } else {
        max_size.min(v.len())
    };
    Formation::Form(v[..take].iter().map(|c| c.id).collect())
}

/// May a batch with this footprint run while these are running? Only if it
/// shares no file and no suite with any of them, and the cap allows it.
pub fn may_run_alongside(running: &[Footprint], f: &Footprint, parallel: usize) -> bool {
    running.len() < parallel && running.iter().all(|r| !r.overlaps(f))
}

/// The tasks a red points at: those whose changes impact a suite that failed.
/// Empty if the failure cannot be pinned to anyone by impact.
pub fn blame_by_impact(failed: &BTreeSet<String>, tasks: &[(u64, &Footprint)]) -> Vec<u64> {
    tasks
        .iter()
        .filter(|(_, f)| !f.suites.is_disjoint(failed))
        .map(|(id, _)| *id)
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Bisect {
    /// Red on its own.
    Culprit(u64),
    /// Each half is green on its own but the whole is red, or the whole is not
    /// red any more: no single task to blame. Ask.
    Unclear,
}

/// Narrow a red batch by halves. `red(ids)` runs the suites over just those
/// tasks (in order) and says whether they fail. The first half is tried
/// first; if it is green the second half is tried; if both are green the red
/// comes from the two together and bisection stops.
pub fn bisect(ids: &[u64], red: &mut dyn FnMut(&[u64]) -> bool) -> Bisect {
    match ids {
        [] => Bisect::Unclear,
        [one] => {
            if red(&[*one]) {
                Bisect::Culprit(*one)
            } else {
                Bisect::Unclear
            }
        }
        _ => {
            let (a, b) = ids.split_at(ids.len() / 2);
            if red(a) {
                return bisect(a, red);
            }
            if red(b) {
                return bisect(b, red);
            }
            Bisect::Unclear
        }
    }
}

/// After the base moved under a verified batch, must the suites run again?
/// Yes if a moved path is one the batch touches, or impacts a suite the batch
/// selected. `impacts` maps a path to the suites it impacts.
pub fn needs_reverify(
    moved: &[String],
    batch: &Footprint,
    impacts: &dyn Fn(&str) -> Vec<String>,
) -> bool {
    moved
        .iter()
        .any(|p| batch.files.contains(p) || impacts(p).iter().any(|s| batch.suites.contains(s)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn cand(id: u64, rank: u64, ready_at: u64) -> Candidate {
        Candidate {
            id,
            rank,
            ready_at,
            footprint: Footprint::default(),
        }
    }

    #[test]
    fn nothing_waiting_forms_nothing() {
        assert_eq!(form_batch(&[], 100, Duration::ZERO, 0), Formation::Empty);
    }

    #[test]
    fn a_zero_window_goes_at_once_in_rank_order() {
        let w = [cand(3, 2, 10), cand(1, 5, 10), cand(2, 2, 10)];
        assert_eq!(
            form_batch(&w, 10, Duration::ZERO, 0),
            Formation::Form(vec![2, 3, 1])
        );
    }

    #[test]
    fn the_window_runs_from_the_oldest_task() {
        let w = [cand(1, 1, 100), cand(2, 1, 130)];
        let win = Duration::from_secs(60);
        assert_eq!(
            form_batch(&w, 120, win, 0),
            Formation::Wait(Duration::from_secs(40))
        );
        assert_eq!(form_batch(&w, 160, win, 0), Formation::Form(vec![1, 2]));
    }

    #[test]
    fn a_full_batch_does_not_wait_and_takes_the_best_ranked() {
        let w = [cand(1, 9, 100), cand(2, 1, 100), cand(3, 5, 100)];
        let win = Duration::from_secs(600);
        assert_eq!(form_batch(&w, 100, win, 2), Formation::Form(vec![2, 3]));
        // Not full yet: wait.
        assert!(matches!(
            form_batch(&w[..1], 100, win, 2),
            Formation::Wait(_)
        ));
    }

    #[test]
    fn overlapping_footprints_do_not_run_together() {
        let a = Footprint::new(["src/a.rs"], ["unit"]);
        let same_file = Footprint::new(["src/a.rs"], []);
        let same_suite = Footprint::new(["b.rs"], ["unit"]);
        let apart = Footprint::new(["docs/x.md"], ["docs"]);
        assert!(!may_run_alongside(std::slice::from_ref(&a), &same_file, 4));
        assert!(!may_run_alongside(std::slice::from_ref(&a), &same_suite, 4));
        assert!(may_run_alongside(std::slice::from_ref(&a), &apart, 4));
        // The cap holds even for disjoint batches.
        assert!(!may_run_alongside(&[a], &apart, 1));
    }

    #[test]
    fn blame_follows_impact() {
        let (a, b, c) = (
            Footprint::new([], ["unit"]),
            Footprint::new([], ["docs"]),
            Footprint::new([], ["unit", "docs"]),
        );
        let failed: BTreeSet<String> = ["unit".to_string()].into();
        assert_eq!(
            blame_by_impact(&failed, &[(1, &a), (2, &b), (3, &c)]),
            vec![1, 3]
        );
        let none: BTreeSet<String> = ["e2e".to_string()].into();
        assert!(blame_by_impact(&none, &[(1, &a)]).is_empty());
    }

    #[test]
    fn bisect_finds_a_single_culprit() {
        let ids = [1, 2, 3, 4, 5];
        for bad in ids {
            let mut calls = 0;
            let r = bisect(&ids, &mut |s| {
                calls += 1;
                s.contains(&bad)
            });
            assert_eq!(r, Bisect::Culprit(bad));
            assert!(calls <= 2 * 3 + 1, "{calls} runs for 5 tasks");
        }
    }

    #[test]
    fn bisect_gives_up_on_an_interaction() {
        // Red only when 1 and 4 are together.
        let r = bisect(&[1, 2, 3, 4], &mut |s| s.contains(&1) && s.contains(&4));
        assert_eq!(r, Bisect::Unclear);
    }

    #[test]
    fn a_moved_base_forces_a_rerun_only_when_it_matters() {
        let batch = Footprint::new(["src/a.rs"], ["unit"]);
        let impacts = |p: &str| -> Vec<String> {
            if p.starts_with("src/") {
                vec!["unit".into()]
            } else {
                vec!["docs".into()]
            }
        };
        assert!(needs_reverify(&["src/other.rs".into()], &batch, &impacts));
        assert!(needs_reverify(&["src/a.rs".into()], &batch, &impacts));
        assert!(!needs_reverify(&["README.md".into()], &batch, &impacts));
        assert!(!needs_reverify(&[], &batch, &impacts));
    }

    proptest! {
        // A formed batch never repeats a task, never exceeds the cap, and
        // is in rank order.
        #[test]
        fn batches_are_ordered_unique_and_capped(
            ranks in proptest::collection::vec(0u64..10, 1..12),
            cap in 0usize..6,
        ) {
            let w: Vec<Candidate> = ranks.iter().enumerate().map(|(i, r)| cand(i as u64, *r, 0)).collect();
            if let Formation::Form(ids) = form_batch(&w, 0, Duration::ZERO, cap) {
                let set: BTreeSet<_> = ids.iter().collect();
                prop_assert_eq!(set.len(), ids.len());
                if cap > 0 { prop_assert!(ids.len() <= cap); }
                let rs: Vec<u64> = ids.iter().map(|i| ranks[*i as usize]).collect();
                prop_assert!(rs.windows(2).all(|p| p[0] <= p[1]));
            } else {
                prop_assert!(false, "a zero window must form");
            }
        }

        // With exactly one bad task, bisection always names it.
        #[test]
        fn bisect_always_finds_the_one_bad_task(n in 1usize..20, bad in 0usize..20) {
            let bad = (bad % n) as u64;
            let ids: Vec<u64> = (0..n as u64).collect();
            prop_assert_eq!(bisect(&ids, &mut |s| s.contains(&bad)), Bisect::Culprit(bad));
        }

        // Footprint overlap is symmetric.
        #[test]
        fn overlap_is_symmetric(a in proptest::collection::btree_set("[a-c]", 0..3), b in proptest::collection::btree_set("[a-c]", 0..3)) {
            let fa = Footprint { files: a.clone(), suites: BTreeSet::new() };
            let fb = Footprint { files: b, suites: a };
            prop_assert_eq!(fa.overlaps(&fb), fb.overlaps(&fa));
        }
    }
}
