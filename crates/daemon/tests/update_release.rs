// SPDX-License-Identifier: GPL-3.0-or-later
#![cfg(unix)]

//! The `release` update channel against a fake release served from a directory (a
//! `file://` base URL): a good update, a checksum that does not match, a release with no
//! build for this platform, and a downgrade. The tarball holds the real binaries, copied
//! and ad-hoc signed (an unsigned copy deleted while macOS scans it crashes syspolicyd).

mod support;
use mason_daemon::release::{asset_name, sha256_hex};
use serde_json::{json, Value};
use std::path::Path;
use std::process::Command;
use support::*;

fn run(dir: &Path, prog: &str, args: &[&str]) {
    let st = Command::new(prog)
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap();
    assert!(st.success(), "{prog} {args:?}");
}

/// Publish release `ver`. `for_this_platform` false lists only another platform's tarball;
/// `lie` writes a SHA256SUMS that does not match the tarball.
fn publish(s: &Scratch, ver: &str, for_this_platform: bool, lie: bool) {
    let root = s.root.path().join("fake-release");
    let assets = root.join("assets");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("api/releases")).unwrap();
    let top = format!("mason-{ver}-pkg");
    let stage = root.join("stage").join(&top);
    std::fs::create_dir_all(&stage).unwrap();
    std::fs::create_dir_all(&assets).unwrap();
    for (pkg, name) in [
        ("mason-daemon", "masond"),
        ("mason-step-host", "step-host"),
        ("mason-adapters", "mason-mcp"),
    ] {
        let from = bin(pkg, name);
        let to = stage.join(name);
        std::fs::copy(&from, &to).unwrap();
        if cfg!(target_os = "macos") {
            run(&stage, "codesign", &["-s", "-", "-f", to.to_str().unwrap()]);
        }
    }
    std::fs::write(stage.join("README.md"), "fake").unwrap();
    let name = if for_this_platform {
        asset_name(ver, std::env::consts::OS, std::env::consts::ARCH).unwrap()
    } else {
        format!("mason-{ver}-riscv64-unknown-linux-gnu.tar.gz")
    };
    run(
        &root.join("stage"),
        "tar",
        &["-czf", assets.join(&name).to_str().unwrap(), &top],
    );
    let hash = if lie {
        "0".repeat(64)
    } else {
        sha256_hex(&assets.join(&name)).unwrap()
    };
    std::fs::write(assets.join("SHA256SUMS"), format!("{hash}  {name}\n")).unwrap();
    let url = |n: &str| format!("file://{}/{n}", assets.display());
    let body = json!({
        "tag_name": format!("v{ver}"),
        "assets": [
            {"name": name, "browser_download_url": url(&name)},
            {"name": "SHA256SUMS", "browser_download_url": url("SHA256SUMS")},
        ],
    });
    std::fs::write(root.join("api/releases/latest"), body.to_string()).unwrap();
    std::fs::write(
        s.config_dir.join("update.toml"),
        format!(
            "channel = \"release\"\nrelease_api = \"file://{}/api\"\ncheck_interval_s = 0\n",
            root.display()
        ),
    )
    .unwrap();
}

fn start(s: &Scratch) -> Masond {
    let mut m = s.masond(&["--managed", "--healthy-after-ms", "1500"], &[]);
    s.wait_daemon(&mut m);
    m
}

fn said(o: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

fn status(s: &Scratch) -> Value {
    serde_json::from_str(&s.mason_ok(&["update", "status", "--json"])).unwrap()
}

fn ready_runtimes(s: &Scratch) -> Vec<String> {
    std::fs::read_dir(s.data_dir.join("runtimes"))
        .map(|d| {
            d.flatten()
                .filter(|e| e.path().join("READY").is_file())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

fn wait_for(what: &str, f: impl Fn() -> bool) {
    let end = std::time::Instant::now() + std::time::Duration::from_secs(120);
    while !f() {
        assert!(
            std::time::Instant::now() < end,
            "timed out waiting for {what}"
        );
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

#[test]
fn a_newer_release_is_downloaded_verified_tested_and_then_taken_over() {
    let s = Scratch::new("true", json!({"steps": {}}));
    publish(&s, "99.0.0", true, false);
    let _m = start(&s);
    // --check stops after the self-test: nothing takes over.
    let o = s.mason(&["update", "--check"]);
    let text = said(&o);
    assert!(o.status.success(), "{text}");
    assert!(
        text.contains("downloaded, verified and passes its self-test"),
        "{text}"
    );
    assert_eq!(ready_runtimes(&s), vec!["99.0.0".to_string()]);
    let v = status(&s);
    assert!(v["running"].is_null() && v["current"].is_null(), "{v:#}");
    // The real thing takes over, and the daemon comes back as the release.
    s.mason_ok(&["update"]);
    wait_for("the release to run", || {
        let o = s.mason(&["update", "status", "--json"]);
        serde_json::from_slice::<Value>(&o.stdout).is_ok_and(|v| v["running"] == "99.0.0")
    });
}

#[test]
fn a_checksum_that_does_not_match_is_refused_and_nothing_is_installed() {
    let s = Scratch::new("true", json!({"steps": {}}));
    publish(&s, "99.0.0", true, true);
    let _m = start(&s);
    let o = s.mason(&["update", "--check"]);
    let text = said(&o);
    assert!(!o.status.success(), "{text}");
    assert!(
        text.contains("checksum") && text.contains("refusing"),
        "{text}"
    );
    assert!(ready_runtimes(&s).is_empty());
    assert!(!s.data_dir.join("runtimes/99.0.0").exists());
    assert!(!s.data_dir.join("runtimes/99.0.0.partial").exists());
    assert!(text.contains("the running daemon is unchanged"), "{text}");
}

#[test]
fn a_release_with_no_build_for_this_platform_is_refused() {
    let s = Scratch::new("true", json!({"steps": {}}));
    publish(&s, "99.0.0", false, false);
    let _m = start(&s);
    let o = s.mason(&["update", "--check"]);
    let text = said(&o);
    assert!(!o.status.success(), "{text}");
    assert!(text.contains("no mason-99.0.0-"), "{text}");
    assert!(ready_runtimes(&s).is_empty());
}

#[test]
fn an_older_release_is_a_downgrade_and_is_refused() {
    let s = Scratch::new("true", json!({"steps": {}}));
    publish(&s, "0.0.1", true, false);
    let _m = start(&s);
    let o = s.mason(&["update", "--check"]);
    let text = said(&o);
    assert!(!o.status.success(), "{text}");
    assert!(
        text.contains("downgrade") && text.contains("--rollback"),
        "{text}"
    );
    assert!(ready_runtimes(&s).is_empty());
}

#[test]
fn versions_are_compared_as_numbers() {
    use mason_daemon::update::{check_release_move, Move};
    assert_eq!(
        check_release_move("0.9.0", "0.10.0", false, &[]),
        Ok(Move::Forward)
    );
    assert!(check_release_move("0.10.0", "0.9.0", false, &[]).is_err());
    assert_eq!(
        check_release_move("0.10.0", "0.9.0", true, &[]),
        Ok(Move::Rollback)
    );
    assert_eq!(
        check_release_move("1.0.0", "1.0.0", false, &[]),
        Ok(Move::Same)
    );
    // A source commit is not a release version.
    assert!(check_release_move("deadbeef", "1.0.0", false, &[]).is_err());
}
