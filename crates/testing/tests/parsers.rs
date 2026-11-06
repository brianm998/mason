// SPDX-License-Identifier: GPL-3.0-or-later

//! A fixture for every parser, including a vacuous pass for each, and the
//! verdict each one produces.

use mason_testing::parse::parse;
use mason_testing::verdict::{judge, Verdict};
use mason_testing::{ParserKind, Suite};

fn fixture(dir: &str, file: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/tests/fixtures/{dir}/{file}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap_or_else(|e| panic!("{dir}/{file}: {e}"))
}

fn suite(kind: ParserKind) -> Suite {
    let mut s = Suite::new("s", "x", kind);
    if kind == ParserKind::Regex {
        s.pass_regex = Some(r"^\[ok\] (\S+)".into());
        s.fail_regex = Some(r"^\[FAIL\] ([^:]+)".into());
    }
    s
}

struct Row {
    kind: ParserKind,
    dir: &'static str,
    ext: &'static str,
    /// executed, passed, skipped in the green fixture
    green: [usize; 3],
    failing: &'static [&'static str],
}

const TABLE: &[Row] = &[
    Row {
        kind: ParserKind::Junit,
        dir: "junit",
        ext: "xml",
        green: [2, 2, 1],
        failing: &["calc.Adder.divides", "calc.Adder.parses"],
    },
    Row {
        kind: ParserKind::Tap,
        dir: "tap",
        ext: "tap",
        green: [2, 2, 1],
        failing: &["subtracts"],
    },
    Row {
        kind: ParserKind::Xctest,
        dir: "xctest",
        ext: "txt",
        green: [2, 2, 1],
        failing: &["-[CalcTests.CalcTests testSub]"],
    },
    Row {
        kind: ParserKind::SwiftTesting,
        dir: "swift-testing",
        ext: "txt",
        green: [2, 2, 0],
        failing: &["divide()"],
    },
    Row {
        kind: ParserKind::Nextest,
        dir: "nextest",
        ext: "txt",
        green: [2, 2, 1],
        failing: &["mason-core ids::tests::timestamps"],
    },
    Row {
        kind: ParserKind::Pytest,
        dir: "pytest",
        ext: "txt",
        green: [2, 2, 1],
        failing: &[
            "tests/test_calc.py::test_div",
            "tests/test_calc.py::test_parse",
        ],
    },
    Row {
        kind: ParserKind::Gradle,
        dir: "gradle",
        ext: "txt",
        green: [2, 2, 1],
        failing: &["com.example.CalcTest > divides()"],
    },
    Row {
        kind: ParserKind::Playwright,
        dir: "playwright",
        ext: "json",
        green: [2, 2, 1],
        failing: &["login.spec.ts > signs out"],
    },
    Row {
        kind: ParserKind::Cucumber,
        dir: "cucumber",
        ext: "json",
        green: [1, 1, 1],
        failing: &["Login > bad password", "Login > undefined step"],
    },
    Row {
        kind: ParserKind::Regex,
        dir: "regex",
        ext: "txt",
        green: [2, 2, 0],
        failing: &["checks/beta"],
    },
];

#[test]
fn every_parser_reads_its_green_fixture() {
    for r in TABLE {
        let res = parse(
            r.kind,
            &fixture(r.dir, &format!("green.{}", r.ext)),
            &suite(r.kind),
        )
        .unwrap();
        assert_eq!(
            [res.executed(), res.passed(), res.skipped()],
            r.green,
            "{} green: {:?}",
            r.kind.name(),
            res.cases
        );
        assert!(res.failures().is_empty(), "{}", r.kind.name());
        assert_eq!(
            judge(&res, r.green[0] as u32, Some(0)),
            Verdict::Green,
            "{} is green with proof",
            r.kind.name()
        );
    }
}

#[test]
fn every_parser_reads_its_red_fixture() {
    for r in TABLE {
        let res = parse(
            r.kind,
            &fixture(r.dir, &format!("red.{}", r.ext)),
            &suite(r.kind),
        )
        .unwrap();
        let mut got = res.failed_names();
        got.sort();
        let mut want: Vec<String> = r.failing.iter().map(|s| s.to_string()).collect();
        want.sort();
        assert_eq!(got, want, "{} red: {:?}", r.kind.name(), res.cases);
        assert!(
            matches!(judge(&res, 1, Some(1)), Verdict::Red { .. }),
            "{}",
            r.kind.name()
        );
    }
}

#[test]
fn every_parser_has_a_vacuous_pass_and_it_is_red() {
    for r in TABLE {
        let res = parse(
            r.kind,
            &fixture(r.dir, &format!("vacuous.{}", r.ext)),
            &suite(r.kind),
        )
        .unwrap();
        assert_eq!(
            res.executed(),
            0,
            "{} vacuous: {:?}",
            r.kind.name(),
            res.cases
        );
        // The tool exited 0 and nothing failed, and it is still not green.
        assert!(
            matches!(
                judge(&res, 1, Some(0)),
                Verdict::Vacuous { executed: 0, .. }
            ),
            "{}",
            r.kind.name()
        );
        // Even with a minimum of zero, at least one test must run.
        assert!(
            matches!(judge(&res, 0, Some(0)), Verdict::Vacuous { .. }),
            "{}",
            r.kind.name()
        );
    }
}

#[test]
fn the_table_covers_every_parser() {
    for k in ParserKind::ALL {
        assert!(
            TABLE.iter().any(|r| r.kind == k),
            "{} has no fixtures",
            k.name()
        );
    }
}

#[test]
fn junit_reads_attribute_quirks() {
    let s = suite(ParserKind::Junit);
    let r = parse(
        ParserKind::Junit,
        "<testsuite><testcase name='single quoted' classname='c'/><testcase name=\"x\" id=\"1\" classname=\"a\" timename=\"no\"></testcase><testcases/></testsuite>",
        &s,
    )
    .unwrap();
    assert_eq!(r.names(), vec!["c.single quoted", "a.x"]);
}

#[test]
fn structured_formats_that_are_not_what_they_claim_are_errors_not_vacuous() {
    for k in [ParserKind::Playwright, ParserKind::Cucumber] {
        assert!(
            parse(k, "this is not json", &suite(k)).is_err(),
            "{}",
            k.name()
        );
    }
    let mut bad = Suite::new("s", "x", ParserKind::Regex);
    assert!(parse(ParserKind::Regex, "x", &bad).is_err(), "no patterns");
    bad.pass_regex = Some("(".into());
    assert!(parse(ParserKind::Regex, "x", &bad).is_err(), "bad pattern");
}

#[test]
fn a_nextest_retry_is_one_case_and_the_failure_wins() {
    let r = parse(
        ParserKind::Nextest,
        "        PASS [ 0.1s] a b\n        FAIL [ 0.1s] a b\n        PASS [ 0.1s] c d\n",
        &suite(ParserKind::Nextest),
    )
    .unwrap();
    assert_eq!(r.executed(), 2);
    assert_eq!(r.failed_names(), vec!["a b"]);
}
