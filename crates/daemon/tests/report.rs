// SPDX-License-Identifier: GPL-3.0-or-later

//! `GET /report` and `mason report --json`: the report of a daemon reads in one pass and its
//! JSON text parses back, with every section present.

mod support;
use support::*;

#[test]
fn the_report_of_a_daemon_parses_and_has_every_section() {
    let s = Scratch::new("true", unique_script());
    let d = s.start();
    let v = mason_daemon::report::report(&d, None, 7);
    let back: serde_json::Value = serde_json::from_str(&format!("{v:#}")).unwrap();
    for k in [
        "landed",
        "failed",
        "lead_time_ms",
        "send_backs",
        "failures",
        "waits",
        "interventions",
        "host_losses",
        "flaky",
        "open_debts",
    ] {
        assert!(!back[k].is_null(), "{k} missing in {back:#}");
    }
    assert_eq!(back["landed"]["total"], 0);
}
