// SPDX-License-Identifier: GPL-3.0-or-later

//! Parsers for what test tools print. Each returns the cases it found; a run
//! that found none is not an error here (the verdict calls it vacuous).
//! Malformed structured output (broken JSON) is an error, because "nothing
//! found" would be a lie about a file that could not be read.

use crate::model::{ParserKind, Suite};
use crate::results::{Case, Results, Status};
use regex::Regex;
use serde_json::Value;

pub fn parse(kind: ParserKind, text: &str, suite: &Suite) -> Result<Results, String> {
    Ok(Results::new(match kind {
        ParserKind::Junit => junit(text),
        ParserKind::Tap => tap(text),
        ParserKind::Xctest => xctest(text),
        ParserKind::SwiftTesting => swift_testing(text),
        ParserKind::Nextest => nextest(text),
        ParserKind::Pytest => pytest(text),
        ParserKind::Gradle => gradle(text),
        ParserKind::Playwright => playwright(text)?,
        ParserKind::Cucumber => cucumber(text)?,
        ParserKind::Regex => subtract_world_waits(regex_lines(text, suite)?, text),
    }))
}

/// A test that waited for a scratch-world slot prints `mason-world-wait <ms> <test name>` to
/// stderr (#778). Its reported time includes that wait, so the recorded time is the reported
/// time minus the wait, for that test only. A wait longer than the time leaves 0.
fn subtract_world_waits(mut cases: Vec<Case>, text: &str) -> Vec<Case> {
    let mut waits: std::collections::BTreeMap<&str, u64> = Default::default();
    for l in text.lines() {
        let Some(rest) = l.trim().strip_prefix("mason-world-wait ") else {
            continue;
        };
        let Some((ms, name)) = rest.split_once(' ') else {
            continue;
        };
        if let Ok(ms) = ms.parse::<u64>() {
            *waits.entry(name.trim()).or_default() += ms;
        }
    }
    if waits.is_empty() {
        return cases;
    }
    for c in &mut cases {
        // libtest names a test `module::name`; the thread name is the same string.
        if let (Some(ms), Some(w)) = (c.ms, waits.get(c.name.as_str())) {
            c.ms = Some(ms.saturating_sub(*w));
        }
    }
    cases
}

/// The cases a results-parser plugin answered with: `{"tests": [{"name", "passed"?, "status"?,
/// "detail"?}]}`. A test is `status` `pass`, `fail` or `skip`, or else `passed` true or false.
/// A malformed answer is an error, never "no tests found".
pub fn from_plugin(v: &Value) -> Result<Results, String> {
    let tests = v["tests"]
        .as_array()
        .ok_or("the parser plugin's answer has no `tests` list")?;
    let mut cases = vec![];
    for (i, t) in tests.iter().enumerate() {
        let name = t["name"]
            .as_str()
            .filter(|n| !n.is_empty())
            .ok_or_else(|| format!("test {i} of the parser plugin's answer has no `name`"))?;
        let detail = t["detail"].as_str().unwrap_or("failed");
        cases.push(match (t["status"].as_str(), t["passed"].as_bool()) {
            (Some("pass"), _) | (None, Some(true)) => Case::pass(name),
            (Some("fail"), _) | (None, Some(false)) => Case::fail(name, detail),
            (Some("skip"), _) => Case::skip(name),
            _ => {
                return Err(format!(
                    "test `{name}` of the parser plugin's answer needs `status` (pass, fail or skip) or `passed`"
                ))
            }
        });
    }
    Ok(Results::new(cases))
}

/// Is this a pattern the regex engine accepts?
pub fn check_regex(p: &str) -> Result<(), String> {
    Regex::new(p)
        .map(|_| ())
        .map_err(|e| format!("not a valid pattern: {e}"))
}

fn re(p: &str) -> Regex {
    Regex::new(p).expect("a built-in pattern")
}

// ---- JUnit XML -------------------------------------------------------------

fn attr(tag: &str, name: &str) -> Option<String> {
    for q in ['"', '\''] {
        let key = format!("{name}={q}");
        let mut from = 0;
        while let Some(i) = tag[from..].find(&key) {
            let at = from + i;
            let boundary = at == 0 || tag.as_bytes()[at - 1].is_ascii_whitespace();
            if boundary {
                let rest = &tag[at + key.len()..];
                if let Some(end) = rest.find(q) {
                    return Some(unescape(&rest[..end]));
                }
            }
            from = at + key.len();
        }
    }
    None
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

fn junit(text: &str) -> Vec<Case> {
    let mut out = vec![];
    let mut rest = text;
    while let Some(i) = rest.find("<testcase") {
        let after = &rest[i + "<testcase".len()..];
        // The tag must be `<testcase` followed by whitespace or `>` or `/`.
        if !after.starts_with(|c: char| c.is_whitespace() || c == '>' || c == '/') {
            rest = after;
            continue;
        }
        let Some(tag_end) = after.find('>') else {
            break;
        };
        let tag = &after[..tag_end];
        let name = attr(tag, "name").unwrap_or_default();
        let class = attr(tag, "classname").unwrap_or_default();
        let full = if class.is_empty() {
            name
        } else {
            format!("{class}.{name}")
        };
        let (body, next) = if tag.trim_end().ends_with('/') {
            ("", &after[tag_end + 1..])
        } else {
            let b = &after[tag_end + 1..];
            match b.find("</testcase>") {
                Some(e) => (&b[..e], &b[e + "</testcase>".len()..]),
                None => (b, ""),
            }
        };
        let status = if let Some(f) = ["<failure", "<error"].iter().find_map(|t| body.find(t)) {
            let t = &body[f..];
            let t = &t[..t.find('>').unwrap_or(t.len())];
            Status::Fail(attr(t, "message").unwrap_or_else(|| "failed".into()))
        } else if body.contains("<skipped") {
            Status::Skip
        } else {
            Status::Pass
        };
        out.push(Case {
            name: full,
            status,
            ms: None,
        });
        rest = next;
    }
    out
}

// ---- TAP -------------------------------------------------------------------

fn tap(text: &str) -> Vec<Case> {
    let r = tap_re();
    text.lines().filter_map(|l| tap_line(&r, l)).collect()
}

fn tap_re() -> Regex {
    re(r"^(not )?ok\b\s*(\d+)?\s*-?\s*(.*)$")
}

fn tap_line(r: &Regex, l: &str) -> Option<Case> {
    {
        let c = r.captures(l.trim_end())?;
        let desc = c.get(3).map_or("", |m| m.as_str());
        let (name, directive) = match desc.split_once(" # ") {
            Some((n, d)) => (n.trim(), d.to_lowercase()),
            None => (desc.trim(), String::new()),
        };
        let name = if name.is_empty() {
            format!("test {}", c.get(2).map_or("?", |m| m.as_str()))
        } else {
            name.to_string()
        };
        let status = if directive.starts_with("skip") || directive.starts_with("todo") {
            Status::Skip
        } else if c.get(1).is_some() {
            Status::Fail("not ok".into())
        } else {
            Status::Pass
        };
        Some(Case {
            name,
            status,
            ms: None,
        })
    }
}

// ---- xctest ----------------------------------------------------------------

fn xctest(text: &str) -> Vec<Case> {
    let r = xctest_re();
    text.lines().filter_map(|l| xctest_line(&r, l)).collect()
}

/// XCTest's own lines (`Test Case '-[A b]' passed (0.1 seconds).`, what a package's bundle
/// prints) and the ones `xcodebuild` writes for a test bundle hosted in an app (Xcode 16 on:
/// `Test case 'A/b()' passed on 'My Mac - App (123)' (0.1 seconds)`, XCTest and Swift Testing
/// alike). One run prints one form or the other, never both, so no test is counted twice.
fn xctest_re() -> Regex {
    re(r"^Test [Cc]ase '(.+?)' (passed|failed|skipped)\b")
}

fn xctest_line(r: &Regex, l: &str) -> Option<Case> {
    let c = r.captures(l.trim())?;
    let name = c[1].to_string();
    Some(match &c[2] {
        "passed" => Case::pass(name),
        "failed" => Case::fail(name, "failed"),
        _ => Case::skip(name),
    })
}

// ---- Swift Testing ---------------------------------------------------------

fn swift_testing(text: &str) -> Vec<Case> {
    let r = swift_testing_re();
    text.lines()
        .filter_map(|l| swift_testing_line(&r, l))
        .collect()
}

fn swift_testing_re() -> Regex {
    re(r"^\S+\s+Test (.+?) (passed after|failed after|skipped)\b")
}

fn swift_testing_line(r: &Regex, l: &str) -> Option<Case> {
    let l = l.trim();
    if l.contains("Test run ") {
        return None;
    }
    let c = r.captures(l)?;
    let name = c[1].trim().to_string();
    Some(match &c[2] {
        "passed after" => Case::pass(name),
        "failed after" => Case::fail(name, "failed"),
        _ => Case::skip(name),
    })
}

// ---- cargo-nextest ---------------------------------------------------------

/// One case per test; a failure beats a pass if a name repeats (retries).
fn dedupe(cases: Vec<Case>) -> Vec<Case> {
    let mut out: Vec<Case> = vec![];
    for c in cases {
        match out.iter_mut().find(|o| o.name == c.name) {
            Some(o) => {
                if matches!(c.status, Status::Fail(_)) {
                    o.status = c.status;
                }
            }
            None => out.push(c),
        }
    }
    out
}

fn nextest(text: &str) -> Vec<Case> {
    let r = nextest_re();
    dedupe(text.lines().filter_map(|l| nextest_line(&r, l)).collect())
}

fn nextest_re() -> Regex {
    re(
        r"^\s*(PASS|FAIL|SKIP|TIMEOUT|LEAK|FLAKY \d+/\d+|SIG[A-Z]+|ABORT|EXEC-FAIL)\s+\[[^\]]*\]\s+(\S.*?)\s*$",
    )
}

fn nextest_line(r: &Regex, l: &str) -> Option<Case> {
    let c = r.captures(l)?;
    let name = c[2].to_string();
    Some(match &c[1] {
        "PASS" | "LEAK" => Case::pass(name),
        s if s.starts_with("FLAKY") => Case::pass(name),
        "SKIP" => Case::skip(name),
        other => Case::fail(name, other.to_lowercase()),
    })
}

// ---- pytest ----------------------------------------------------------------

fn pytest(text: &str) -> Vec<Case> {
    let r = pytest_re();
    dedupe(text.lines().filter_map(|l| pytest_line(&r, l)).collect())
}

fn pytest_re() -> (Regex, Regex) {
    (
        re(r"^(\S+::\S+)\s+(PASSED|FAILED|SKIPPED|ERROR|XFAIL|XPASS)\b"),
        re(r"^\[gw\d+\]\s+\[\s*\d+%\]\s+(PASSED|FAILED|SKIPPED|ERROR|XFAIL|XPASS)\s+(\S+::\S+)"),
    )
}

fn pytest_line(r: &(Regex, Regex), l: &str) -> Option<Case> {
    let (a, b) = r;
    let (name, status) = if let Some(c) = a.captures(l) {
        (c[1].to_string(), c[2].to_string())
    } else if let Some(c) = b.captures(l) {
        (c[2].to_string(), c[1].to_string())
    } else {
        return None;
    };
    Some(match status.as_str() {
        "PASSED" | "XPASS" => Case::pass(name),
        "SKIPPED" | "XFAIL" => Case::skip(name),
        other => Case::fail(name, other.to_lowercase()),
    })
}

// ---- Gradle ----------------------------------------------------------------

fn gradle(text: &str) -> Vec<Case> {
    let r = gradle_re();
    text.lines().filter_map(|l| gradle_line(&r, l)).collect()
}

fn gradle_re() -> Regex {
    re(r"^(\S.*?) > (.+?) (PASSED|FAILED|SKIPPED)\s*$")
}

fn gradle_line(r: &Regex, l: &str) -> Option<Case> {
    let c = r.captures(l)?;
    let name = format!("{} > {}", &c[1], &c[2]);
    Some(match &c[3] {
        "PASSED" => Case::pass(name),
        "FAILED" => Case::fail(name, "failed"),
        _ => Case::skip(name),
    })
}

// ---- Playwright (JSON reporter) --------------------------------------------

fn playwright(text: &str) -> Result<Vec<Case>, String> {
    let v: Value =
        serde_json::from_str(text).map_err(|e| format!("not Playwright JSON output: {e}"))?;
    let mut out = vec![];
    fn walk(suite: &Value, path: &[String], out: &mut Vec<Case>) {
        let mut path = path.to_vec();
        if let Some(t) = suite["title"].as_str().filter(|t| !t.is_empty()) {
            path.push(t.to_string());
        }
        for spec in suite["specs"].as_array().into_iter().flatten() {
            let title = spec["title"].as_str().unwrap_or("?");
            let name = path
                .iter()
                .map(String::as_str)
                .chain([title])
                .collect::<Vec<_>>()
                .join(" > ");
            let statuses: Vec<&str> = spec["tests"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|t| t["status"].as_str())
                .collect();
            let status = if statuses.contains(&"unexpected") || spec["ok"] == Value::Bool(false) {
                Status::Fail("unexpected".into())
            } else if statuses.iter().all(|s| *s == "skipped") {
                // (also true of no tests at all: nothing ran)
                Status::Skip
            } else {
                Status::Pass
            };
            out.push(Case {
                name,
                status,
                ms: None,
            });
        }
        for child in suite["suites"].as_array().into_iter().flatten() {
            walk(child, &path, out);
        }
    }
    for s in v["suites"].as_array().into_iter().flatten() {
        walk(s, &[], &mut out);
    }
    Ok(out)
}

// ---- Cucumber JSON ---------------------------------------------------------

fn cucumber(text: &str) -> Result<Vec<Case>, String> {
    let v: Value =
        serde_json::from_str(text).map_err(|e| format!("not Cucumber JSON output: {e}"))?;
    let mut out = vec![];
    for f in v.as_array().into_iter().flatten() {
        let feature = f["name"].as_str().unwrap_or("?");
        for e in f["elements"].as_array().into_iter().flatten() {
            if e["type"].as_str() == Some("background") {
                continue;
            }
            let name = format!("{feature} > {}", e["name"].as_str().unwrap_or("?"));
            let steps: Vec<&str> = e["steps"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|s| s["result"]["status"].as_str())
                .collect();
            let status = if steps
                .iter()
                .any(|s| matches!(*s, "failed" | "undefined" | "pending" | "ambiguous"))
            {
                Status::Fail("a step did not pass".into())
            } else if steps.is_empty() || steps.iter().all(|s| *s == "skipped") {
                Status::Skip
            } else {
                Status::Pass
            };
            out.push(Case {
                name,
                status,
                ms: None,
            });
        }
    }
    Ok(out)
}

// ---- regex fallback --------------------------------------------------------

/// The compiled patterns of the regex parser.
pub(crate) struct RegexPair {
    pass: Option<Regex>,
    fail: Option<Regex>,
}

/// The time libtest's `--report-time` appends to a result line: `test a::b ... ok <0.120s>`
/// gives 120. `None` when the line has no such suffix.
fn duration_ms(l: &str) -> Option<u64> {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"<(\d+(?:\.\d+)?)s>\s*$").expect("a fixed pattern"));
    let secs: f64 = re.captures(l)?[1].parse().ok()?;
    Some((secs * 1000.0).round() as u64)
}

fn regex_pair(suite: &Suite) -> Result<RegexPair, String> {
    let compile = |p: &Option<String>, what: &str| -> Result<Option<Regex>, String> {
        p.as_deref()
            .map(|p| Regex::new(p).map_err(|e| format!("{what}: {e}")))
            .transpose()
    };
    let pass = compile(&suite.pass_regex, "pass_regex")?;
    let fail = compile(&suite.fail_regex, "fail_regex")?;
    if pass.is_none() && fail.is_none() {
        return Err("the regex parser needs pass_regex or fail_regex".into());
    }
    Ok(RegexPair { pass, fail })
}

/// The location a compiler-style diagnostic names: `path:line:col: error: ...`
/// gives `path:line`. Anything else gives `None`.
fn compiler_location(l: &str) -> Option<String> {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"^\s*(\S+?):(\d+)(?::\d+)?:\s*(?:fatal\s+)?error\b").expect("a fixed pattern")
    });
    let c = re.captures(l)?;
    Some(format!("{}:{}", &c[1], &c[2]))
}

/// The name of a failure. A compiler diagnostic is named by its location, a
/// capture by the capture, and anything else (a line of source, a linker
/// message) by one fixed name, so the same broken build gets the same names
/// on every run and no line of code reaches the flake records.
fn regex_fail_name(c: &regex::Captures, l: &str) -> String {
    compiler_location(l).unwrap_or_else(|| {
        c.get(1)
            .map_or_else(|| "build".to_string(), |m| m.as_str().to_string())
    })
}

fn regex_line(r: &RegexPair, l: &str) -> Option<Case> {
    if let Some(c) = r.fail.as_ref().and_then(|r| r.captures(l)) {
        let mut case = Case::fail(regex_fail_name(&c, l), "matched fail_regex");
        case.ms = duration_ms(l);
        return Some(case);
    }
    let c = r.pass.as_ref().and_then(|r| r.captures(l))?;
    let name = c
        .get(1)
        .map_or_else(|| l.trim().to_string(), |m| m.as_str().to_string());
    let mut case = Case::pass(name);
    case.ms = duration_ms(l);
    Some(case)
}

fn regex_lines(text: &str, suite: &Suite) -> Result<Vec<Case>, String> {
    let r = regex_pair(suite)?;
    Ok(text.lines().filter_map(|l| regex_line(&r, l)).collect())
}

// ---- one line at a time ----------------------------------------------------

/// A parser that reads one line at a time, for the formats whose output is
/// lines (not a whole document). The full [`parse`] uses the same per-line
/// functions, so a live count cannot drift from it.
pub(crate) enum LineParser {
    Tap(Regex),
    Xctest(Regex),
    SwiftTesting(Regex),
    Nextest(Regex),
    Pytest((Regex, Regex)),
    Gradle(Regex),
    Regex(RegexPair),
}

impl LineParser {
    /// None for the formats that are a whole document (JUnit, Playwright, Cucumber).
    pub(crate) fn new(kind: ParserKind, suite: &Suite) -> Option<LineParser> {
        Some(match kind {
            ParserKind::Tap => LineParser::Tap(tap_re()),
            ParserKind::Xctest => LineParser::Xctest(xctest_re()),
            ParserKind::SwiftTesting => LineParser::SwiftTesting(swift_testing_re()),
            ParserKind::Nextest => LineParser::Nextest(nextest_re()),
            ParserKind::Pytest => LineParser::Pytest(pytest_re()),
            ParserKind::Gradle => LineParser::Gradle(gradle_re()),
            ParserKind::Regex => LineParser::Regex(regex_pair(suite).ok()?),
            ParserKind::Junit | ParserKind::Playwright | ParserKind::Cucumber => return None,
        })
    }

    /// True for the parsers where a repeated name is one test (a failure wins).
    pub(crate) fn dedupes(&self) -> bool {
        matches!(self, LineParser::Nextest(_) | LineParser::Pytest(_))
    }

    pub(crate) fn line(&self, l: &str) -> Option<Case> {
        match self {
            LineParser::Tap(r) => tap_line(r, l),
            LineParser::Xctest(r) => xctest_line(r, l),
            LineParser::SwiftTesting(r) => swift_testing_line(r, l),
            LineParser::Nextest(r) => nextest_line(r, l),
            LineParser::Pytest(r) => pytest_line(r, l),
            LineParser::Gradle(r) => gradle_line(r, l),
            LineParser::Regex(r) => regex_line(r, l),
        }
    }
}

#[cfg(test)]
mod build_failure_names {
    use super::*;

    fn suite() -> Suite {
        let mut s = Suite::new("s", "x", ParserKind::Regex);
        s.fail_regex = Some(r"error:|await viewModel".into());
        s
    }

    const LOG: &str = "/src/View.swift:42:17: error: cannot find 'localized' in scope\n\
        await viewModel.report(error: localized(\"ui.render_failed\", error))\n\
        ld: error: undefined symbol\n";

    #[test]
    fn a_compiler_line_is_named_by_path_and_line_and_the_name_is_stable() {
        let a = regex_lines(LOG, &suite()).unwrap();
        let b = regex_lines(LOG, &suite()).unwrap();
        let names = |v: &[Case]| v.iter().map(|c| c.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&a), ["/src/View.swift:42", "build", "build"]);
        assert_eq!(names(&a), names(&b));
        assert!(
            a.iter().all(|c| matches!(c.status, Status::Fail(_))),
            "counts and verdicts unchanged"
        );
    }
}

#[cfg(test)]
mod report_time {
    use super::*;

    fn suite() -> Suite {
        let mut s = Suite::new("s", "x", ParserKind::Regex);
        s.pass_regex = Some(r"^test (.+) \.\.\. ok(?: <[0-9.]+s>)?$".into());
        s.fail_regex = Some(r"^test (.+) \.\.\. FAILED(?: <[0-9.]+s>)?$".into());
        s
    }

    #[test]
    fn a_result_line_with_a_time_reads_as_a_pass_or_failure_with_its_duration() {
        let log = "test a::b ... ok <0.120s>\ntest a::c ... FAILED <3.000s>\n\
                   test a::d ... ok\ntest a::e ... FAILED\ntest a::f ... ignored\n";
        let r = regex_lines(log, &suite()).unwrap();
        let got: Vec<_> = r.iter().map(|c| (c.name.as_str(), c.ms)).collect();
        assert_eq!(
            got,
            [
                ("a::b", Some(120)),
                ("a::c", Some(3000)),
                ("a::d", None),
                ("a::e", None)
            ]
        );
        assert_eq!(r[0].status, Status::Pass);
        assert!(matches!(r[1].status, Status::Fail(_)));
        assert_eq!(r[2].status, Status::Pass);
        assert!(matches!(r[3].status, Status::Fail(_)));
    }

    #[test]
    fn a_world_wait_line_is_subtracted_from_that_tests_time_only() {
        let log = "mason-world-wait 600000 a::b\ntest a::b ... ok <600.500s>\n\
                   test a::c ... ok <1.000s>\nmason-world-wait 5 a::zzz\n";
        let r = parse(ParserKind::Regex, log, &suite()).unwrap();
        let got: Vec<_> = r.cases.iter().map(|c| (c.name.as_str(), c.ms)).collect();
        assert_eq!(got, [("a::b", Some(500)), ("a::c", Some(1000))]);
    }

    #[test]
    fn the_times_to_store_leave_out_a_tests_world_wait() {
        // What the daemon records after a run: a test that queued 600 s for a slot and worked
        // for half a second is stored as half a second, and a test with no time is not stored.
        let log = "mason-world-wait 600000 a::b\ntest a::b ... ok <600.500s>\n\
                   mason-world-wait 120 a::b\ntest a::c ... ok <1.000s>\ntest a::d ... ok\n";
        let r = parse(ParserKind::Regex, log, &suite()).unwrap();
        assert_eq!(
            r.test_times(),
            [("a::b".to_string(), 380), ("a::c".to_string(), 1000)]
        );
    }
}
