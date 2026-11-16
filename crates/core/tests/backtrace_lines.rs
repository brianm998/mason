// SPDX-License-Identifier: GPL-3.0-or-later
//! The dev/test profile sets `split-debuginfo` so no per-unit `.o` files pile up (decision 0482).
//! This checks that a panic in a test still names file and line, in the panic message and in the
//! backtrace frame of the panicking function.

use std::process::Command;

#[test]
#[ignore = "run by the parent test, with MASON_BT_CHILD set"]
fn child_panics() {
    if std::env::var_os("MASON_BT_CHILD").is_none() {
        return;
    }
    #[inline(never)]
    fn boom() {
        panic!("boom");
    }
    boom();
}

#[test]
fn a_panicking_test_names_file_and_line() {
    let exe = std::env::current_exe().unwrap();
    let out = Command::new(exe)
        .args(["--ignored", "--exact", "child_panics", "--nocapture"])
        .env("MASON_BT_CHILD", "1")
        .env("RUST_BACKTRACE", "1")
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "child should panic: {err}");
    let has_line = |needle: &str| {
        err.lines().any(|l| {
            l.find(needle).is_some_and(|i| {
                l[i + needle.len()..]
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_digit())
            })
        })
    };
    assert!(
        has_line("backtrace_lines.rs:"),
        "no file:line in output:\n{err}"
    );
    // A frame line "at ...backtrace_lines.rs:N" comes from debug info, not the panic message.
    assert!(
        err.lines()
            .any(|l| l.trim_start().starts_with("at ") && l.contains("backtrace_lines.rs:")),
        "backtrace has no source frame:\n{err}"
    );
}
