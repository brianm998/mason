// SPDX-License-Identifier: GPL-3.0-or-later

//! A run meant for GitHub that `fallback = "local"` sends here takes the machine slots it would
//! hold when placed here, through the broker, so this machine's limits (and with them the CPU
//! and thermal holds) apply to it. Before, such a run had skipped the local queue on its way to
//! GitHub and started here at once, beside everything else. No duration is asserted.

use crate::support;
use std::collections::BTreeMap;
use support::lw::*;
use support::wait_for;

fn compile() -> BTreeMap<String, u32> {
    BTreeMap::from([("compile".to_string(), 1)])
}

#[test]
fn a_fallback_run_waits_for_the_slot_it_would_hold_here_and_gets_it_when_freed() {
    let w = world("window = \"20m\"\n", &[]);
    let d = w.d.clone();
    d.broker.set_limit("compile", 1).unwrap();

    let first = mason_daemon::remote::fallback_lease(&d, "scratch", None, &compile(), "first")
        .unwrap()
        .expect("a lease");
    assert_eq!(d.broker.used("compile"), 1);

    // A second fallback run queues behind the first: the limit of one holds.
    let got = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let h = {
        let (d, got) = (d.clone(), got.clone());
        std::thread::spawn(move || {
            let l = mason_daemon::remote::fallback_lease(&d, "scratch", None, &compile(), "second")
                .unwrap()
                .expect("a lease");
            got.store(true, std::sync::atomic::Ordering::SeqCst);
            l
        })
    };
    // While the first holds the only slot, the second cannot have it (this can only fail if the
    // limit is ignored, never because the machine is slow).
    assert!(!got.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(d.broker.used("compile"), 1);

    drop(first);
    wait_for("the second fallback run to get the freed slot", 60, || {
        got.load(std::sync::atomic::Ordering::SeqCst)
    });
    let second = h.join().unwrap();
    assert_eq!(d.broker.used("compile"), 1);
    drop(second);
    assert_eq!(d.broker.used("compile"), 0);
}

#[test]
fn nothing_to_hold_takes_nothing() {
    let w = world("window = \"20m\"\n", &[]);
    let none =
        mason_daemon::remote::fallback_lease(&w.d, "scratch", None, &BTreeMap::new(), "x").unwrap();
    assert!(none.is_none());
    assert_eq!(w.d.broker.used("compile"), 0);
}
