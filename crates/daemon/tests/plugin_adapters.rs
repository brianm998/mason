// SPDX-License-Identifier: GPL-3.0-or-later
//! Plugin adapters, parsers and environments on the daemon's side: built only from an
//! approved hash, and through the same conformance kits as the built-in adapters.

use mason_adapter_api::kit::{build_kit, host_kit, tracker_kit, HostScenario};
use mason_adapter_api::service::*;
use mason_daemon::config::{parse_project, ProjectCfg};
use mason_daemon::plugins::*;
use std::path::{Path, PathBuf};
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

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../plugins/tests/fixtures").join(name)
}

/// A repo whose base branch holds the fixture extensions, a config dir, and a project
/// whose `[host]`, `[ci]` and `[tracker]` are the adapters fixture.
fn setup() -> (tempfile::TempDir, tempfile::TempDir, ProjectCfg) {
    let repo = tempfile::tempdir().unwrap();
    git(repo.path(), &["init", "-q", "-b", "main"]);
    for (from, to) in [
        ("adapters", "fixture-adapters"),
        ("parser", "fixture-parser"),
        ("canceler", "canceler"),
    ] {
        let dest = repo.path().join(".mason/extensions").join(to);
        std::fs::create_dir_all(&dest).unwrap();
        for f in std::fs::read_dir(fixture(from)).unwrap() {
            let f = f.unwrap().path();
            if f.is_file() {
                std::fs::copy(&f, dest.join(f.file_name().unwrap())).unwrap();
            }
        }
    }
    git(repo.path(), &["add", "-A"]);
    git(repo.path(), &["commit", "-qm", "extensions"]);
    let cfg = tempfile::tempdir().unwrap();
    let text = format!(
        "path = \"{}\"\nbase = \"main\"\ntest = \"t\"\n[host]\nkind = \"plugin:fixture-adapters\"\n[ci]\nkind = \"plugin:fixture-adapters\"\n[tracker]\nkind = \"plugin:fixture-adapters\"\n",
        repo.path().display()
    );
    let mut p = parse_project("web", &text).unwrap();
    p.config_dir = cfg.path().to_path_buf();
    (repo, cfg, p)
}

fn approve(repo: &Path, cfg: &Path, name: &str) {
    let hash = scan_commit(repo, "HEAD").get(name).unwrap().hash.clone();
    record_approval(
        cfg,
        "web",
        &Approval {
            name: name.into(),
            hash,
            by: "owner".into(),
            at_ms: 1,
        },
    )
    .unwrap();
}

#[test]
fn an_unapproved_or_changed_plugin_adapter_is_not_built() {
    let (repo, cfg, p) = setup();
    let e = mason_daemon::services::tracker(&p).err().expect("unapproved");
    assert!(e.contains("needs approval"), "{e}");
    assert!(mason_daemon::services::host(&p).err().unwrap().contains("needs approval"));
    assert!(mason_daemon::services::ci(&p).err().unwrap().contains("needs approval"));

    approve(repo.path(), cfg.path(), "fixture-adapters");
    assert!(mason_daemon::services::tracker(&p).is_ok());

    // Any change to the folder is a different hash, and needs a new approval.
    std::fs::write(
        repo.path().join(".mason/extensions/fixture-adapters/adapters.py"),
        "# changed\n",
    )
    .unwrap();
    git(repo.path(), &["commit", "-qam", "edit"]);
    let e = mason_daemon::services::tracker(&p).err().expect("changed");
    assert!(e.contains("needs approval") && e.contains("changed since"), "{e}");
}

#[test]
fn an_extension_that_provides_nothing_of_the_kind_is_not_found() {
    let (repo, cfg, mut p) = setup();
    approve(repo.path(), cfg.path(), "fixture-adapters");
    p.services.host = Some(mason_daemon::config::HostCfg::Plugin {
        name: "nobody".into(),
    });
    let e = mason_daemon::services::host(&p).err().unwrap();
    assert!(e.contains("no extension") && e.contains("adapter:host:nobody"), "{e}");
}

#[test]
fn approved_plugin_adapters_pass_the_conformance_kits() {
    let (repo, cfg, p) = setup();
    approve(repo.path(), cfg.path(), "fixture-adapters");

    let (tracker, tcfg) = mason_daemon::services::tracker(&p).unwrap().unwrap();
    assert_eq!(tcfg.plugin.as_deref(), Some("fixture-adapters"));
    assert_eq!(tracker_kit(tracker.as_ref()), Vec::<String>::new());
    // Importing an issue through it: the listing the poller reads.
    let Ok(Listing::Items { items, .. }) = tracker.list(None) else {
        panic!("a listing")
    };
    assert_eq!(items[0].title, "First issue");

    let host = mason_daemon::services::host(&p).unwrap().unwrap();
    let s = HostScenario {
        base: "main".into(),
        good_head: "feature".into(),
        missing_head: "missing-branch".into(),
        reviewer: None,
    };
    assert_eq!(host_kit(host.as_ref(), &s), Vec::<String>::new());

    let (ci, _) = mason_daemon::services::ci(&p).unwrap().unwrap();
    assert_eq!(build_kit(ci.as_ref(), "refs/mason/x", "abc"), Vec::<String>::new());
}

#[test]
fn a_parser_plugin_reads_results_and_an_environment_plugin_runs_its_lifecycle() {
    let (repo, cfg, p) = setup();
    let e = parse_with_plugin(&p, "fixture-parser", "PASS a\n").unwrap_err();
    assert!(e.contains("needs approval"), "{e}");
    approve(repo.path(), cfg.path(), "fixture-parser");

    let r = parse_with_plugin(&p, "fixture-parser", "PASS a\nFAIL b: broke\n").unwrap();
    assert_eq!((r.passed(), r.failed_names()), (1, vec!["b".to_string()]));
    assert!(parse_with_plugin(&p, "no-such-parser", "PASS a\n").is_err());

    let c = serde_json::json!({});
    for phase in ["setup", "health", "teardown"] {
        environment_call(&p, "fixture-parser", phase, &c).unwrap();
    }
    let e = environment_call(&p, "fixture-parser", "health", &serde_json::json!({"broken": true}))
        .unwrap_err();
    assert!(e.contains("broken"), "{e}");
}

#[test]
fn only_approved_step_types_are_known_to_definition_check() {
    let (repo, cfg, p) = setup();
    assert!(approved_types(&p).is_empty());
    approve(repo.path(), cfg.path(), "canceler");
    let names: Vec<String> = approved_types(&p).into_iter().map(|t| t.name).collect();
    assert_eq!(names, ["wait-for-cancel"]);
}
