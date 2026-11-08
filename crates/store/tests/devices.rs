// SPDX-License-Identifier: GPL-3.0-or-later

use mason_store::Store;

#[test]
fn devices_are_found_by_hash_until_revoked() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(d.path().join("mason.db")).unwrap();
    let id = s.create_device("phone", "viewer", "abc", 10).unwrap();
    let found = s.find_device("abc").unwrap().unwrap();
    assert_eq!((found.id, found.role.as_str()), (id, "viewer"));
    assert!(s.find_device("nope").unwrap().is_none());
    s.touch_device(id, 20).unwrap();
    assert_eq!(s.list_devices().unwrap()[0].last_seen, Some(20));
    assert!(s.revoke_device(id, 30).unwrap());
    assert!(!s.revoke_device(id, 31).unwrap());
    assert!(s.find_device("abc").unwrap().is_none());
    assert_eq!(s.list_devices().unwrap()[0].revoked_at, Some(30));
}

#[test]
fn a_token_hash_is_unique() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(d.path().join("mason.db")).unwrap();
    s.create_device("a", "owner", "h", 1).unwrap();
    assert!(s.create_device("b", "owner", "h", 2).is_err());
}
