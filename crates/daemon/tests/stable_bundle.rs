// SPDX-License-Identifier: GPL-3.0-or-later

//! A takeover installs the new runtime into the coordinator's stable app bundle and execs it from
//! the same path every time (s1118). `take_over` itself replaces the process, so this runs the
//! two calls it makes before the exec: `stable_app::stage` and `stable_app::executable`.
//! Everything is in a scratch directory with its own `app_dir`; no binary is created, only
//! ad-hoc signed copies of Apple's own, so nothing unsigned is ever written or deleted.

#![cfg(target_os = "macos")]

use mason_daemon::config::DaemonConfig;
use mason_daemon::stable_app;
use mason_daemon::update::{sign_installed, Runtimes};
use std::path::{Path, PathBuf};
use std::process::Command;

/// A fake runtime `sha`: `masond` and `step-host` copied from `binary`, signed ad hoc.
fn runtime(rt: &Runtimes, sha: &str, binary: &str) {
    let dir = rt.dir(sha);
    std::fs::create_dir_all(&dir).unwrap();
    for name in ["masond", "step-host"] {
        let to = dir.join(name);
        std::fs::copy(binary, &to).unwrap();
        sign_installed(&to, None).unwrap();
    }
    std::fs::write(dir.join("SHA"), sha).unwrap();
    std::fs::write(dir.join("READY"), "").unwrap();
}

fn verifies(app: &Path) -> bool {
    Command::new("/usr/bin/codesign")
        .args(["--verify", "--strict"])
        .arg(app)
        .status()
        .unwrap()
        .success()
}

/// What `take_over` does before it execs: stage, then take the path to exec.
fn take_over_path(cfg: &DaemonConfig, rt: &Runtimes, sha: &str) -> PathBuf {
    let staged = stable_app::stage(cfg, rt, sha);
    assert!(staged.is_some(), "the bundle for {sha} was not staged");
    stable_app::executable(staged.as_ref(), rt, sha)
}

#[test]
fn two_takeovers_run_masond_from_the_same_stable_bundle_path() {
    let d = tempfile::tempdir().unwrap();
    let (data, config, apps) = (
        d.path().join("data"),
        d.path().join("c"),
        d.path().join("apps"),
    );
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(
        config.join("update.toml"),
        format!(
            "repo = \"/nonexistent\"\napp_dir = \"{}\"\n",
            apps.display()
        ),
    )
    .unwrap();
    let cfg = DaemonConfig::new(
        &data,
        &config,
        "/nonexistent/step-host".into(),
        "/nonexistent/mcp".into(),
    );
    let rt = Runtimes::new(&data);
    runtime(&rt, "aaaa", "/usr/bin/true");
    runtime(&rt, "bbbb", "/usr/bin/false");

    let first = take_over_path(&cfg, &rt, "aaaa");
    assert!(
        first.starts_with(&apps),
        "{} is not under {}",
        first.display(),
        apps.display()
    );
    assert!(
        !first.starts_with(&data),
        "it must not run from a runtime directory"
    );
    assert!(verifies(&apps.join(stable_app::BUNDLE_NAME)));
    assert!(
        Command::new(&first).status().unwrap().success(),
        "runtime aaaa is `true`"
    );
    assert!(first.with_file_name("step-host").is_file());

    let second = take_over_path(&cfg, &rt, "bbbb");
    assert_eq!(
        first, second,
        "the path masond is exec'd from changes between takeovers"
    );
    assert!(verifies(&apps.join(stable_app::BUNDLE_NAME)));
    assert!(
        !Command::new(&second).status().unwrap().success(),
        "runtime bbbb is `false`"
    );

    // The bundle names the identifier macOS keys its rule on, and has the usage text.
    let plist = std::fs::read_to_string(
        apps.join(stable_app::BUNDLE_NAME)
            .join("Contents/Info.plist"),
    )
    .unwrap();
    assert!(plist.contains(&format!("<string>{}</string>", stable_app::IDENTIFIER)));
    assert!(plist.contains("NSLocalNetworkUsageDescription"));

    // The same runtime again changes nothing; `app_dir = "off"` means no bundle at all.
    assert_eq!(take_over_path(&cfg, &rt, "bbbb"), first);
    std::fs::write(
        config.join("update.toml"),
        "repo = \"/nonexistent\"\napp_dir = \"off\"\n",
    )
    .unwrap();
    assert!(stable_app::stage(&cfg, &rt, "aaaa").is_none());
}
