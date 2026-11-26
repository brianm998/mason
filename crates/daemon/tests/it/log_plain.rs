// SPDX-License-Identifier: GPL-3.0-or-later

//! The real `masond` with stderr in a file: the log is plain text, so a
//! `grep` for `task=1` in it can match.

use crate::support;
use support::*;

#[test]
fn a_daemon_logging_to_a_file_writes_no_colour_codes() {
    let s = Scratch::new("true", happy_script());
    let log = s.root.path().join("masond.log");
    let log_s = log.display().to_string();
    let mut m = s.masond(&[], &[("MASON_TEST_STDERR", &log_s), ("RUST_LOG", "info")]);
    s.wait_daemon(&mut m);
    wait_for("masond to log that it is listening", 60, || {
        std::fs::read_to_string(&log).is_ok_and(|t| t.contains("masond listening"))
    });
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(!text.contains("\x1b["), "{text:?}");
    let _ = m.child.kill();
    let _ = m.child.wait();
}
