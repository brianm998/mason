// SPDX-License-Identifier: GPL-3.0-or-later
#![allow(dead_code)]

use std::path::PathBuf;

/// The `step-host` binary. It belongs to another package, so cargo only builds
/// it for `cargo test --workspace`; build it on demand otherwise.
pub fn step_host_bin() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let target = exe.parent().unwrap().parent().unwrap(); // target/debug
    let bin = target.join("step-host");
    if !bin.exists() {
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let ok = std::process::Command::new(cargo)
            .args(["build", "-q", "-p", "mason-step-host", "--bin", "step-host"])
            .status()
            .unwrap()
            .success();
        assert!(ok, "could not build step-host");
    }
    bin
}
