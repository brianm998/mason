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
        ParserKind::Regex => regex_lines(text, suite)?,
    }))
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
        out.push(Case { name: full, status });
        rest = next;
    }
    out
}

// ---- TAP -------------------------------------------------------------------

fn tap(text: &str) -> Vec<Case> {
    let r = re(r"^(not )?ok\b\s*(\d+)?\s*-?\s*(.*)$");
    let mut out = vec![];
    for l in text.lines() {
        let Some(c) = r.captures(l.trim_end()) else {
            continue;
        };
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
        out.push(Case { name, status });
    }
    out
}

// ---- xctest ----------------------------------------------------------------

fn xctest(text: &str) -> Vec<Case> {
    let r = re(r"^Test Case '(.+)' (passed|failed|skipped)\b");
    text.lines()
        .filter_map(|l| {
            let c = r.captures(l.trim())?;
            let name = c[1].to_string();
            Some(match &c[2] {
                "passed" => Case::pass(name),
                "failed" => Case::fail(name, "failed"),
                _ => Case::skip(name),
            })
        })
        .collect()
}

// ---- Swift Testing ---------------------------------------------------------

fn swift_testing(text: &str) -> Vec<Case> {
    let r = re(r"^\S+\s+Test (.+?) (passed after|failed after|skipped)\b");
    text.lines()
        .filter_map(|l| {
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
        })
        .collect()
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
    let r = re(
        r"^\s*(PASS|FAIL|SKIP|TIMEOUT|LEAK|FLAKY \d+/\d+|SIG[A-Z]+|ABORT|EXEC-FAIL)\s+\[[^\]]*\]\s+(\S.*?)\s*$",
    );
    dedupe(
        text.lines()
            .filter_map(|l| {
                let c = r.captures(l)?;
                let name = c[2].to_string();
                Some(match &c[1] {
                    "PASS" | "LEAK" => Case::pass(name),
                    s if s.starts_with("FLAKY") => Case::pass(name),
                    "SKIP" => Case::skip(name),
                    other => Case::fail(name, other.to_lowercase()),
                })
            })
            .collect(),
    )
}

// ---- pytest ----------------------------------------------------------------

fn pytest(text: &str) -> Vec<Case> {
    let a = re(r"^(\S+::\S+)\s+(PASSED|FAILED|SKIPPED|ERROR|XFAIL|XPASS)\b");
    let b =
        re(r"^\[gw\d+\]\s+\[\s*\d+%\]\s+(PASSED|FAILED|SKIPPED|ERROR|XFAIL|XPASS)\s+(\S+::\S+)");
    dedupe(
        text.lines()
            .filter_map(|l| {
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
            })
            .collect(),
    )
}

// ---- Gradle ----------------------------------------------------------------

fn gradle(text: &str) -> Vec<Case> {
    let r = re(r"^(\S.*?) > (.+?) (PASSED|FAILED|SKIPPED)\s*$");
    text.lines()
        .filter_map(|l| {
            let c = r.captures(l)?;
            let name = format!("{} > {}", &c[1], &c[2]);
            Some(match &c[3] {
                "PASSED" => Case::pass(name),
                "FAILED" => Case::fail(name, "failed"),
                _ => Case::skip(name),
            })
        })
        .collect()
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
            out.push(Case { name, status });
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
            out.push(Case { name, status });
        }
    }
    Ok(out)
}

// ---- regex fallback --------------------------------------------------------

fn regex_lines(text: &str, suite: &Suite) -> Result<Vec<Case>, String> {
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
    let name_of = |c: &regex::Captures, l: &str| {
        c.get(1)
            .map_or_else(|| l.trim().to_string(), |m| m.as_str().to_string())
    };
    let mut out = vec![];
    for l in text.lines() {
        if let Some(c) = fail.as_ref().and_then(|r| r.captures(l)) {
            out.push(Case::fail(name_of(&c, l), "matched fail_regex"));
        } else if let Some(c) = pass.as_ref().and_then(|r| r.captures(l)) {
            out.push(Case::pass(name_of(&c, l)));
        }
    }
    Ok(out)
}
