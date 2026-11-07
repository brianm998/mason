// SPDX-License-Identifier: GPL-3.0-or-later
//! Extensions on the daemon's side: read from a commit only, hashed, approved by hash,
//! and run from an exact copy.

use mason_daemon::plugins::*;
use std::path::Path;
use std::process::Command;

fn git(dir: &Path, args: &[&str]) {
    let o = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        o.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&o.stderr)
    );
}

fn write(dir: &Path, file: &str, text: &str) {
    let p = dir.join(file);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

const MANIFEST: &str = "name = \"check\"\nprotocol = 1\nprovides = [\"step-type:my-check\"]\nrun = \"./go.sh\"\nconfig_schema = \"schema.json\"\n";
const SCHEMA: &str = r#"{"type":"object","properties":{"dir":{"type":"string"}}}"#;

fn repo() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    git(t.path(), &["init", "-q", "-b", "main"]);
    write(t.path(), ".mason/extensions/check/manifest.toml", MANIFEST);
    write(t.path(), ".mason/extensions/check/schema.json", SCHEMA);
    write(
        t.path(),
        ".mason/extensions/check/go.sh",
        "#!/bin/sh\nexit 0\n",
    );
    git(t.path(), &["add", "-A"]);
    git(
        t.path(),
        &[
            "update-index",
            "--chmod=+x",
            ".mason/extensions/check/go.sh",
        ],
    );
    git(t.path(), &["commit", "-qm", "ext"]);
    t
}

#[test]
fn extensions_are_read_from_the_commit_and_hashed() {
    let t = repo();
    let scan = scan_commit(t.path(), "HEAD");
    let e = scan.get("check").unwrap();
    assert!(e.refused().is_none(), "{:?}", e.manifest);
    assert_eq!(scan.types()[0].name, "my-check");
    assert_eq!(scan.providing("my-check").unwrap().dir, "check");
    assert!(e.entries.iter().any(|f| f.path == "go.sh" && f.exec));

    // The worktree is not the commit: an edit or an extra extension there changes nothing.
    write(
        t.path(),
        ".mason/extensions/check/go.sh",
        "#!/bin/sh\ntouch /tmp/evil\n",
    );
    write(t.path(), ".mason/extensions/other/manifest.toml", MANIFEST);
    let again = scan_commit(t.path(), "HEAD");
    assert_eq!(again.get("check").unwrap().hash, e.hash);
    assert!(again.get("other").is_none());

    // Committing the edit is a different hash.
    git(t.path(), &["commit", "-qam", "edit"]);
    assert_ne!(
        scan_commit(t.path(), "HEAD").get("check").unwrap().hash,
        e.hash
    );
}

#[test]
fn a_manifest_that_does_not_match_its_folder_is_refused() {
    let t = repo();
    write(
        t.path(),
        ".mason/extensions/check/manifest.toml",
        &MANIFEST.replace("name = \"check\"", "name = \"other\""),
    );
    git(t.path(), &["commit", "-qam", "rename"]);
    let scan = scan_commit(t.path(), "HEAD");
    assert!(scan
        .get("check")
        .unwrap()
        .refused()
        .unwrap()
        .contains("must be the folder's name"));
    assert!(scan.types().is_empty());
}

#[test]
fn approval_is_by_hash_and_lives_outside_the_repo() {
    let t = repo();
    let cfg = tempfile::tempdir().unwrap();
    let hash = scan_commit(t.path(), "HEAD")
        .get("check")
        .unwrap()
        .hash
        .clone();
    assert!(approved(cfg.path(), "web", "check", &hash).is_none());
    let a = Approval {
        name: "check".into(),
        hash: hash.clone(),
        by: "brian".into(),
        at_ms: 5,
    };
    record_approval(cfg.path(), "web", &a).unwrap();
    assert_eq!(approved(cfg.path(), "web", "check", &hash), Some(a));
    assert!(
        approved(cfg.path(), "web", "check", "0000").is_none(),
        "another hash"
    );
    assert!(
        approved(cfg.path(), "web", "other", &hash).is_none(),
        "another name"
    );
    assert!(
        approved(cfg.path(), "api", "check", &hash).is_none(),
        "another project"
    );
    assert!(!t.path().join("trust").exists());
}

#[test]
fn the_copy_that_runs_is_the_hashed_bytes_and_a_tampered_copy_is_replaced() {
    let t = repo();
    let cfg = tempfile::tempdir().unwrap();
    let scan = scan_commit(t.path(), "HEAD");
    let e = scan.get("check").unwrap();
    let dir = materialize(cfg.path(), e).unwrap();
    assert_eq!(dir, cfg.path().join("extensions").join(&e.hash));
    assert_eq!(
        std::fs::read_to_string(dir.join("go.sh")).unwrap(),
        "#!/bin/sh\nexit 0\n"
    );
    use std::os::unix::fs::PermissionsExt;
    assert_ne!(
        std::fs::metadata(dir.join("go.sh"))
            .unwrap()
            .permissions()
            .mode()
            & 0o111,
        0
    );
    // Someone edits the approved copy on disk: the next run puts the exact bytes back.
    std::fs::write(dir.join("go.sh"), "#!/bin/sh\ntouch /tmp/evil\n").unwrap();
    let dir = materialize(cfg.path(), e).unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.join("go.sh")).unwrap(),
        "#!/bin/sh\nexit 0\n"
    );
}
