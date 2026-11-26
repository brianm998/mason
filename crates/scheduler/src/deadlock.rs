// SPDX-License-Identifier: GPL-3.0-or-later

//! The self-deadlock (#1149): every unit of a resource is held by a caller that cannot release
//! it until a second request of its own for the same resource is granted.

use crate::ledger::Snapshot;
use std::collections::BTreeMap;

/// A resource that cannot move: nothing that holds it can end.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Deadlock {
    pub resource: String,
    /// The labels of the holders, each waiting for itself.
    pub holders: Vec<String>,
    /// The smallest limit that would let one of the waiting requests in.
    pub unblocking_limit: u32,
}

/// The resources every unit of which is held by holders that each wait for more of the same.
/// `blocked` maps a waiting request's id to the holder that is kept until that request ends.
/// One holder that is not waiting on itself means the resource can come free: no deadlock.
pub fn self_deadlocks(snap: &Snapshot, blocked: &BTreeMap<u64, u64>) -> Vec<Deadlock> {
    let mut out = vec![];
    for (resource, cap) in &snap.capacity {
        let units = |amounts: &BTreeMap<String, u32>| amounts.get(resource).copied().unwrap_or(0);
        let held: Vec<_> = snap
            .holders
            .iter()
            .filter(|h| units(&h.needs.amounts) > 0)
            .collect();
        let used: u32 = held.iter().map(|h| units(&h.needs.amounts)).sum();
        if held.is_empty() || used < cap.units {
            continue;
        }
        // For each holder, what its own waiting request asks for.
        let wants: Option<Vec<u32>> = held
            .iter()
            .map(|h| {
                snap.waiting
                    .iter()
                    .filter(|w| blocked.get(&w.id) == Some(&h.id))
                    .map(|w| units(&w.needs.amounts))
                    .find(|n| *n > 0)
            })
            .collect();
        if let Some(least) = wants.and_then(|w| w.into_iter().min()) {
            out.push(Deadlock {
                resource: resource.clone(),
                holders: held.iter().map(|h| h.label.clone()).collect(),
                unblocking_limit: used + least,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Kind, Ledger, Needs, Rank};

    fn compile() -> Needs {
        Needs::of(&[("compile", 1)])
    }

    fn ledger(limit: u32) -> Ledger {
        let mut l = Ledger::new();
        l.set_capacity("compile", limit, Kind::Machine);
        l
    }

    fn rank(n: u64) -> Rank {
        Rank::new(false, 1, 0, n, n, false)
    }

    #[test]
    fn every_holder_waiting_only_on_itself_is_a_deadlock() {
        let mut l = ledger(2);
        let a = l.adopt(compile(), "#1", 0);
        let b = l.adopt(compile(), "#2", 0);
        let wa = l.submit(rank(1), compile(), "#1 check", 0);
        let wb = l.submit(rank(2), compile(), "#2 check", 0);
        let blocked = BTreeMap::from([(wa, a), (wb, b)]);
        let found = self_deadlocks(&l.snapshot(), &blocked);
        assert_eq!(
            found,
            vec![Deadlock {
                resource: "compile".into(),
                holders: vec!["#1".into(), "#2".into()],
                unblocking_limit: 3,
            }]
        );
    }

    #[test]
    fn one_holder_that_is_building_means_no_deadlock() {
        let mut l = ledger(2);
        let a = l.adopt(compile(), "#1", 0);
        l.adopt(compile(), "#2 building", 0);
        let wa = l.submit(rank(1), compile(), "#1 check", 0);
        let blocked = BTreeMap::from([(wa, a)]);
        assert_eq!(self_deadlocks(&l.snapshot(), &blocked), vec![]);
    }

    #[test]
    fn a_free_unit_or_an_ordinary_wait_is_no_deadlock() {
        let mut l = ledger(2);
        let a = l.adopt(compile(), "#1", 0);
        let wa = l.submit(rank(1), compile(), "#1 check", 0);
        assert_eq!(
            self_deadlocks(&l.snapshot(), &BTreeMap::from([(wa, a)])),
            vec![],
            "a unit is free"
        );
        l.adopt(compile(), "#2", 0);
        assert_eq!(
            self_deadlocks(&l.snapshot(), &BTreeMap::new()),
            vec![],
            "nobody waits on a slot of their own"
        );
    }
}
