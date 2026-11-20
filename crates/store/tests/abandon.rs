// SPDX-License-Identifier: GPL-3.0-or-later
//! `abandon_batches` names the batches it abandoned, so their step hosts can be stopped.

use mason_store::Store;

#[test]
fn abandon_batches_returns_the_ids_of_the_unfinished_ones() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(d.path().join("mason.db")).unwrap();
    let a = s.add_batch("p", "1,2", 1).unwrap();
    let b = s.add_batch("p", "3", 2).unwrap();
    let other = s.add_batch("q", "4", 3).unwrap();
    assert_eq!(s.abandon_batches("p", 10).unwrap(), vec![a, b]);
    assert!(s.abandon_batches("p", 11).unwrap().is_empty());
    assert_eq!(s.abandon_batches("q", 12).unwrap(), vec![other]);
}
