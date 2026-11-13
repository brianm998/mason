// SPDX-License-Identifier: GPL-3.0-or-later
#![cfg(unix)]
// Unix-only: signals, permission bits or unix sockets. Windows coverage comes with the later port tasks.

//! The key candidates (owner's account, passphrase) and the unlock, which must never see the
//! passphrase.

use mason_daemon::ssh_keys::{list_in, unlock};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

fn keygen(dir: &Path, name: &str, pass: &str) -> String {
    let f = dir.join(name);
    assert!(std::process::Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", pass, "-C", name, "-f"])
        .arg(&f)
        .status()
        .unwrap()
        .success());
    std::fs::read_to_string(dir.join(format!("{name}.pub"))).unwrap()
}

fn script(path: &Path, body: &str) {
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn keys_are_marked_for_the_owners_account_and_for_a_passphrase() {
    let d = tempfile::tempdir().unwrap();
    let ssh = d.path().join("ssh");
    std::fs::create_dir(&ssh).unwrap();
    let _plain = keygen(&ssh, "id_plain", "");
    let locked = keygen(&ssh, "id_locked", "s3cret");
    // A stand-in for https://github.com/<owner>.keys
    let web = d.path().join("web");
    std::fs::create_dir(&web).unwrap();
    std::fs::write(web.join("brianm998.keys"), &locked).unwrap();
    std::env::set_var("MASON_GITHUB_URL", format!("file://{}", web.display()));
    std::env::set_var("MASON_GITHUB_SSH", "nobody@127.0.0.1");
    let keys = list_in(&ssh, Some("brianm998"));
    let by = |n: &str| {
        keys.iter()
            .find(|k| k.path.as_ref().is_some_and(|p| p.ends_with(n)))
            .unwrap()
    };
    assert!(by("id_locked").on_owner_account);
    assert!(!by("id_plain").on_owner_account);
    assert!(by("id_locked").encrypted);
    assert!(!by("id_plain").encrypted);
    assert!(
        keys[0].path.as_ref().unwrap().ends_with("id_locked"),
        "the owner's account sorts first"
    );
}

#[test]
fn the_passphrase_is_asked_for_by_ssh_add_and_never_seen_here() {
    let d = tempfile::tempdir().unwrap();
    let askpass = d.path().join("askpass.sh");
    script(&askpass, "echo s3cret");
    let record = d.path().join("record");
    // A stand-in for ssh-add: asks the way ssh-add does, and fails unless the answer is right.
    let fake = d.path().join("ssh-add");
    script(
        &fake,
        &format!(
            "echo \"$SSH_ASKPASS_REQUIRE $@\" > {}\n[ \"$($SSH_ASKPASS 'Enter passphrase')\" = s3cret ] || {{ echo 'bad passphrase' >&2; exit 1; }}",
            record.display()
        ),
    );
    std::env::set_var("MASON_SSH_ADD", &fake);
    let key = d.path().join("id_x");
    std::fs::write(&key, "x").unwrap();
    unlock(&key, Some(&askpass)).unwrap();
    let rec = std::fs::read_to_string(&record).unwrap();
    assert!(rec.starts_with("force "), "{rec}");

    let wrong = d.path().join("wrong.sh");
    script(&wrong, "echo nope");
    let err = unlock(&key, Some(&wrong)).unwrap_err();
    assert!(err.contains("did not add the key"), "{err}");
    // Nothing MASON says or keeps holds the passphrase.
    for text in [&rec, &err, &format!("{:?}", unlock(&key, None))] {
        assert!(!text.contains("s3cret"), "{text}");
    }
}
