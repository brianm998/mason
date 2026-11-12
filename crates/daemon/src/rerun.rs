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
            // libtest: `test a::b ... FAILED`
            if let Some(name) = rest.strip_suffix(" ... FAILED") {
                out.insert(name.to_string());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn libtest_xctest_and_pytest_failures_are_read() {
        let t = "test a::one ... ok\ntest a::two ... FAILED\nTest Case '-[S t]' failed (0.1 seconds).\n\
                 tests/x.py::test_y FAILED\nFAILED tests/z.py::test_w - assert\nfailures:\n    a::two\n";
        assert_eq!(
            failing_names(t),
            vec!["-[S t]", "a::two", "tests/x.py::test_y", "tests/z.py::test_w"]
        );
        assert!(failing_names("nope\nexit 1\n").is_empty());
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
