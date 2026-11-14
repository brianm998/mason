// SPDX-License-Identifier: GPL-3.0-or-later
//! What a results-parser plugin answers becomes the same `Results` a built-in parser gives,
//! and the verdict is judged from it unchanged.

use mason_testing::parse::from_plugin;
use mason_testing::{judge, Status, Verdict};
use serde_json::json;

#[test]
fn a_plugins_tests_become_cases() {
    let r = from_plugin(&json!({"tests": [
        {"name": "a", "passed": true},
        {"name": "b", "passed": false, "detail": "broke"},
        {"name": "c", "status": "skip"},
        {"name": "d", "status": "pass"}
    ]}))
    .unwrap();
    assert_eq!(r.names(), ["a", "b", "c", "d"]);
    assert_eq!(r.cases[1].status, Status::Fail("broke".into()));
    assert_eq!((r.passed(), r.executed(), r.skipped()), (2, 3, 1));
}

#[test]
fn a_malformed_answer_is_an_error_not_no_tests() {
    for bad in [
        json!({}),
        json!({"tests": "x"}),
        json!({"tests": [{"passed": true}]}),
        json!({"tests": [{"name": "a"}]}),
        json!({"tests": [{"name": "a", "status": "maybe"}]}),
    ] {
        assert!(from_plugin(&bad).is_err(), "{bad}");
    }
}

#[test]
fn the_verdict_comes_from_the_plugins_results() {
    let green = from_plugin(&json!({"tests": [{"name": "a", "passed": true}]})).unwrap();
    assert!(matches!(judge(&green, 1, Some(0)), Verdict::Green));
    let red = from_plugin(&json!({"tests": [
        {"name": "a", "passed": true}, {"name": "b", "passed": false, "detail": "x"}
    ]}))
    .unwrap();
    assert!(matches!(judge(&red, 1, Some(0)), Verdict::Red { .. }));
    // Nothing found proves nothing, whoever parsed it.
    let none = from_plugin(&json!({"tests": []})).unwrap();
    assert!(!matches!(judge(&none, 1, Some(0)), Verdict::Green));
}
