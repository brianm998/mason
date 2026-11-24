// SPDX-License-Identifier: GPL-3.0-or-later

use mason_store::{NodeRow, Store};

fn node(nid: &str, name: &str, fp: &str) -> NodeRow {
    NodeRow {
        nid: nid.into(),
        name: name.into(),
        host: format!("{name}.local"),
        port: 7425,
        fingerprint: fp.into(),
        paired_at: 5,
    }
}

#[test]
fn paired_nodes_are_kept_by_name_and_a_node_paired_again_replaces_its_row() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(d.path().join("mason.db")).unwrap();
    assert!(s.list_nodes().unwrap().is_empty());
    s.put_node(&node("n2", "studio", "aa")).unwrap();
    s.put_node(&node("n1", "mini", "bb")).unwrap();
    let names: Vec<String> = s
        .list_nodes()
        .unwrap()
        .into_iter()
        .map(|n| n.name)
        .collect();
    assert_eq!(names, ["mini", "studio"]);
    // The same node paired again (its certificate was replaced): one row, the new fingerprint.
    s.put_node(&node("n2", "studio", "cc")).unwrap();
    let all = s.list_nodes().unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[1].fingerprint, "cc");
    // A different node taking a name frees it from the old one.
    s.put_node(&node("n9", "studio", "dd")).unwrap();
    assert_eq!(s.list_nodes().unwrap().len(), 2);
    assert!(s.remove_node("studio").unwrap());
    assert!(!s.remove_node("studio").unwrap());
    assert_eq!(s.list_nodes().unwrap().len(), 1);
}
