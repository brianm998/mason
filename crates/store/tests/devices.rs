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

#[test]
fn a_push_token_belongs_to_one_live_device_and_revoking_clears_it() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(d.path().join("mason.db")).unwrap();
    let a = s.create_device("phone", "owner", "h1", 1).unwrap();
    let b = s.create_device("ipad", "viewer", "h2", 2).unwrap();
    assert!(s.push_targets().unwrap().is_empty());
    assert!(s.set_push(a, "aa11", "sandbox").unwrap());
    assert!(s.set_push(b, "bb22", "production").unwrap());
    let t = s.push_targets().unwrap();
    assert_eq!(t.len(), 2);
    assert_eq!((t[0].device, t[0].env.as_str()), (a, "sandbox"));
    assert_eq!(t[1].role, "viewer");
    // A token Apple refused is cleared, but a newer one the device registered since is kept.
    s.clear_push_token(a, "other").unwrap();
    assert!(s.push_of(a).unwrap().is_some());
    s.clear_push_token(a, "aa11").unwrap();
    assert!(s.push_of(a).unwrap().is_none());
    s.clear_push(b).unwrap();
    assert!(s.push_of(b).unwrap().is_none());
    // Revoking a device clears its token, and a revoked device cannot register one.
    assert!(s.set_push(a, "cc33", "sandbox").unwrap());
    assert!(s.revoke_device(a, 5).unwrap());
    assert!(s.push_of(a).unwrap().is_none());
    assert!(!s.set_push(a, "dd44", "sandbox").unwrap());
    assert!(s.push_targets().unwrap().is_empty());
}
