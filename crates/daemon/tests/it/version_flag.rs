// SPDX-License-Identifier: GPL-3.0-or-later

//! `masond --version` prints `mason <version> (<commit>)`.

#[test]
fn masond_version_has_the_agreed_shape() {
    let o = std::process::Command::new(env!("CARGO_BIN_EXE_masond"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(o.status.success());
    let out = String::from_utf8(o.stdout).unwrap();
    let want = format!("mason {} (", env!("CARGO_PKG_VERSION"));
    assert!(
        out.starts_with(&want) && out.trim_end().ends_with(')'),
        "{out:?}"
    );
    assert_eq!(out.lines().count(), 1);
}
