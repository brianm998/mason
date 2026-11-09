// SPDX-License-Identifier: GPL-3.0-or-later

//! The real `masond`, started with the soft limit launchd gives a service: the
//! kernel has to accept the raise. An unknown flag makes it exit right after.

#[test]
fn a_daemon_started_with_launchds_open_file_limit_raises_it() {
    let out = std::process::Command::new("sh")
        .args(["-c", "ulimit -Sn 256 && exec \"$0\" --no-such-flag"])
        .arg(env!("CARGO_BIN_EXE_masond"))
        .env("RUST_LOG", "info")
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("raised the open-file limit from 256 to"),
        "{err}"
    );
}
