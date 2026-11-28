// SPDX-License-Identifier: GPL-3.0-or-later

//! Reading which tests failed in a test command's output, and building the command that
//! runs only those again (s165). Pure: text in, text out. The parsers of `mason_testing`
//! are used read-only; libtest and pytest's summary lines are read here, since neither is
//! a parser kind there.

use mason_testing::model::{ParserKind, Suite};
use std::collections::BTreeSet;

/// Names of the tests that failed, from libtest (`test a::b ... FAILED`), XCTest and
/// Swift Testing, pytest (`path::name FAILED`, or the summary's `FAILED path::name - why`)
/// and cargo-nextest lines. Sorted, one entry per name. Empty if none can be read.
pub fn failing_names(text: &str) -> Vec<String> {
    let mut out: BTreeSet<String> = BTreeSet::new();
    for l in text.lines() {
        let l = l.trim_end();
        if let Some(rest) = l.strip_prefix("test ") {
            // libtest: `test a::b ... FAILED`, or with `--report-time` (s634)
            // `test a::b ... FAILED <0.054s>`. Without the timed form no cargo-workspace failure
            // was read, so no check said a failing test was red on develop too.
            if let Some((name, verdict)) = rest.rsplit_once(" ... ") {
                let verdict = verdict.split_once(" <").map_or(verdict, |(v, _)| v);
                if verdict == "FAILED" {
                    out.insert(name.to_string());
                }
            }
        } else if let Some(rest) = l
            .strip_prefix("FAILED ")
            .or_else(|| l.strip_prefix("ERROR "))
        {
            // pytest's summary: `FAILED path::name - why`
            let first = rest.split_whitespace().next().unwrap_or("");
            if first.contains("::") {
                out.insert(first.to_string());
            }
        }
    }
    let suite = Suite::new("rerun", "", ParserKind::Xctest);
    for kind in [
        ParserKind::Xctest,
        ParserKind::SwiftTesting,
        ParserKind::Pytest,
        ParserKind::Nextest,
    ] {
        if let Ok(r) = mason_testing::parse::parse(kind, text, &suite) {
            out.extend(r.failed_names());
        }
    }
    out.into_iter().collect()
}

/// Is `rerun_filter` a form that can be used: `cargo`, or a template with `{tests}`?
pub fn check_filter(f: &str) -> Result<(), String> {
    if f == "cargo" || f.contains("{tests}") {
        Ok(())
    } else {
        Err(format!(
            "`rerun_filter` is {f:?}: it must be \"cargo\" or a command template containing {{tests}}"
        ))
    }
}

fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// The command that runs only `names`, or `cmd` unchanged if there is no filter or no name.
/// `cargo` appends `-- --exact <names>` (just ` --exact <names>` if the command already has
/// a ` -- ` of its own); a template has `{tests}` replaced by the quoted names.
pub fn command(cmd: &str, filter: Option<&str>, names: &[String]) -> String {
    let Some(f) = filter else {
        return cmd.to_string();
    };
    if names.is_empty() {
        return cmd.to_string();
    }
    let list = names.iter().map(|n| quote(n)).collect::<Vec<_>>().join(" ");
    if f == "cargo" {
        let sep = if cmd.contains(" -- ") { "" } else { " --" };
        format!("{cmd}{sep} --exact {list}")
    } else {
        f.replace("{tests}", &list)
    }
}

/// Did a run that was told to run only `names` run them? A filter that matched nothing exits
/// green with no test run, which proves nothing; a run that ran them prints their names.
pub fn ran_them(output: &str, names: &[String]) -> bool {
    names.iter().all(|n| output.contains(n.as_str()))
}

/// Most failing test names a digest lists; the rest are counted.
const DIGEST_NAMES: usize = 30;
/// How many failing tests get their panic text shown.
const DIGEST_PANICS: usize = 3;
/// Most lines of panic text per test, and most characters per line.
const DIGEST_PANIC_LINES: usize = 12;
const DIGEST_LINE_CHARS: usize = 300;

fn clip(l: &str) -> String {
    let l = l.replace("```", "'''");
    if l.chars().count() > DIGEST_LINE_CHARS {
        let mut s: String = l.chars().take(DIGEST_LINE_CHARS).collect();
        s.push('…');
        s
    } else {
        l
    }
}

/// The lines of one failing test's report (`---- name stdout ----`), from the whole log, up to
/// the `panicked at` text and what follows it; bounded. Empty if the log has none.
fn panic_lines(text: &str, name: &str) -> Vec<String> {
    let head = format!("---- {name} stdout ----");
    let mut it = text.lines().skip_while(|l| l.trim_end() != head);
    if it.next().is_none() {
        return vec![];
    }
    let mut out = vec![];
    for l in it {
        if l.starts_with("---- ") || l.trim_end() == "failures:" || out.len() >= DIGEST_PANIC_LINES
        {
            break;
        }
        out.push(clip(l));
    }
    out
}

/// What a red run's result starts with (#1258): the names of the tests that failed (bounded,
/// with a count of the rest) and, for the first few, their panic text, read from the whole
/// log rather than its last lines. Fenced as data. Empty when no failing test can be read.
pub fn failure_digest(text: &str) -> String {
    let names = failing_names(text);
    if names.is_empty() {
        return String::new();
    }
    let mut s = String::from("Failing tests (from the whole log):\n```\n");
    for n in names.iter().take(DIGEST_NAMES) {
        s.push_str(&clip(n));
        s.push('\n');
    }
    if names.len() > DIGEST_NAMES {
        s.push_str(&format!("... and {} more\n", names.len() - DIGEST_NAMES));
    }
    s.push_str("```\n");
    let mut body = String::new();
    for n in names.iter().take(DIGEST_PANICS) {
        let p = panic_lines(text, n);
        if !p.is_empty() {
            body.push_str(&format!("{n}:\n{}\n", p.join("\n")));
        }
    }
    if !body.is_empty() {
        s.push_str("Their panics:\n```\n");
        s.push_str(&body);
        s.push_str("```\n");
    }
    s
}

/// Did a test run finish having run no test at all? True for libtest summaries that all say
/// `0 passed; 0 failed`, and for pytest's `no tests ran`. A log with neither is not judged.
pub fn ran_no_tests(text: &str) -> bool {
    if text.contains("no tests ran") {
        return true;
    }
    let mut seen = false;
    for l in text.lines().filter(|l| l.starts_with("test result: ")) {
        seen = true;
        if !l.contains(" 0 passed; 0 failed;") {
            return false;
        }
    }
    seen
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_names_early_failure_and_panic() {
        let mut t = String::from(
            "test a::bad ... FAILED\n\nfailures:\n\n---- a::bad stdout ----\nthread 'a::bad' panicked at src/a.rs:3:5:\nboom\n\nfailures:\n    a::bad\n\n",
        );
        for i in 0..100 {
            t.push_str(&format!("test b::t{i} ... ok\n"));
        }
        let d = failure_digest(&t);
        assert!(d.contains("a::bad\n") && d.contains("panicked at src/a.rs:3:5"));
        assert!(d.contains("boom"));
    }

    #[test]
    fn digest_counts_the_rest() {
        let t: String = (0..40)
            .map(|i| format!("test m::t{i} ... FAILED\n"))
            .collect();
        assert!(failure_digest(&t).contains("... and 10 more"));
        assert!(failure_digest("all fine\n").is_empty());
    }

    #[test]
    fn zero_test_runs_are_seen() {
        let z = "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 5 filtered out\n";
        assert!(ran_no_tests(z));
        let n = format!("{z}test result: ok. 2 passed; 0 failed; 0 ignored;\n");
        assert!(!ran_no_tests(&n));
        assert!(!ran_no_tests("nothing\n"));
    }

    #[test]
    fn libtest_xctest_and_pytest_failures_are_read() {
        let t = "test a::one ... ok\ntest a::two ... FAILED\nTest Case '-[S t]' failed (0.1 seconds).\n\
                 tests/x.py::test_y FAILED\nFAILED tests/z.py::test_w - assert\nfailures:\n    a::two\n";
        assert_eq!(
            failing_names(t),
            vec![
                "-[S t]",
                "a::two",
                "tests/x.py::test_y",
                "tests/z.py::test_w"
            ]
        );
        assert!(failing_names("nope\nexit 1\n").is_empty());
    }

    #[test]
    fn libtest_failures_with_report_time_are_read() {
        let t = "test a::one ... ok <0.001s>\ntest a::two ... FAILED <0.054s>\n\
                 test a::three ... FAILED\ntest a::slow has been running for over 60 seconds\n";
        assert_eq!(failing_names(t), vec!["a::three", "a::two"]);
    }

    #[test]
    fn the_filter_carries_only_the_names() {
        let n = vec!["a".to_string(), "b c".to_string()];
        assert_eq!(command("cargo test", None, &n), "cargo test");
        assert_eq!(command("cargo test", Some("cargo"), &[]), "cargo test");
        assert_eq!(
            command("cargo test", Some("cargo"), &n),
            "cargo test -- --exact 'a' 'b c'"
        );
        assert_eq!(
            command("cargo test -- -q", Some("cargo"), &n),
            "cargo test -- -q --exact 'a' 'b c'"
        );
        assert_eq!(command("x", Some("./t.sh {tests}"), &n), "./t.sh 'a' 'b c'");
        assert!(check_filter("cargo").is_ok());
        assert!(check_filter("./t.sh {tests}").is_ok());
        assert!(check_filter("guess").is_err());
    }

    #[test]
    fn a_rerun_that_printed_no_names_proved_nothing() {
        let n = vec!["a".to_string()];
        assert!(!ran_them("running 0 tests\n", &n));
        assert!(ran_them("test a ... ok\n", &n));
    }
}
