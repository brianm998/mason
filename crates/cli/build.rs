// SPDX-License-Identifier: GPL-3.0-or-later

//! Records the commit this binary is built from as `MASON_COMMIT`, for `--version`:
//! `MASON_BUILD_SHA` when given (a build from a source archive), else `git rev-parse`,
//! else `unknown`.

fn main() {
    println!("cargo:rerun-if-env-changed=MASON_BUILD_SHA");
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    let given = std::env::var("MASON_BUILD_SHA")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let sha = given.or_else(|| {
        let out = std::process::Command::new("git")
            .args(["rev-parse", "--short=12", "HEAD"])
            .output()
            .ok()?;
        let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
        (out.status.success() && !s.is_empty()).then_some(s)
    });
    println!(
        "cargo:rustc-env=MASON_COMMIT={}",
        sha.unwrap_or_else(|| "unknown".into())
    );
}
