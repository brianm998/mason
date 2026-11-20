// SPDX-License-Identifier: GPL-3.0-or-later

//! `mason remote` routes `setup` to the workflow writer and on/off/status to the daemon.

use std::process::Command;

fn run(args: &[&str]) -> (bool, String) {
    let dir = std::env::temp_dir().join(format!("mason-remote-dispatch-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_mason"))
        .args(args)
        .env("MASON_CONFIG_DIR", &dir)
        .env("MASON_DATA_DIR", &dir)
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    (o.status.success(), text)
}

#[test]
fn bogus_remote_command_names_all_four() {
    let (ok, out) = run(&["remote", "setup", "--bogus"]);
    assert!(!ok, "{out}");
    let (ok, out) = run(&["remote", "bogus"]);
    assert!(!ok, "{out}");
    for w in ["setup", "on", "off", "status"] {
        assert!(out.contains(&format!("`{w}`")), "{w} missing: {out}");
    }
}

#[test]
fn status_on_off_reach_the_daemon_handler() {
    for sub in ["status", "on", "off"] {
        let (_, out) = run(&["remote", sub]);
        assert!(!out.contains("the only remote command"), "{sub}: {out}");
        assert!(!out.contains("unknown remote command"), "{sub}: {out}");
    }
}
