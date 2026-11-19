// SPDX-License-Identifier: GPL-3.0-or-later
#![allow(dead_code)]

use std::path::PathBuf;

/// A scratch directory. On Unix it lives under `/tmp`, because the paths must
/// stay short enough for an AF_UNIX socket; Windows has no `/tmp`, so it uses
/// the platform temp dir.
pub fn short_tempdir() -> std::io::Result<tempfile::TempDir> {
    #[cfg(unix)]
    {
        tempfile::tempdir_in("/tmp")
    }
    #[cfg(windows)]
    {
        tempfile::tempdir()
    }
}

/// The `step-host` binary. It belongs to another package, so cargo only builds
/// it for `cargo test --workspace`; build it on demand otherwise.
pub fn step_host_bin() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let target = exe.parent().unwrap().parent().unwrap(); // target/debug
    let bin = target.join("step-host");
    static BUILT: std::sync::Once = std::sync::Once::new();
    BUILT.call_once(|| {
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let ok = std::process::Command::new(cargo)
            .args(["build", "-q", "-p", "mason-step-host", "--bin", "step-host"])
            // The suite's runner gives this process RUSTC_BOOTSTRAP=1 for `--report-time`
            // (s634); build scripts rerun when it changes, so a build that kept it would
            // recompile the workspace the suite's build just compiled without it.
            .env_remove("RUSTC_BOOTSTRAP")
            .status()
            .unwrap()
            .success();
        assert!(ok, "could not build step-host");
    });
    bin
}
