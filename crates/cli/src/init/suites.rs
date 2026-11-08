// SPDX-License-Identifier: GPL-3.0-or-later

//! Question 5 of `mason init`: propose test suites from what was detected, run each once to
//! see that its output parses, and keep what that run showed (how many tests, how long) as a
//! baseline. A suite that is red or finds nothing is reported, never quietly kept green.

use super::questions::SuiteSpec;
use mason_testing::parse::parse as read;
use mason_testing::results::Results;
use mason_testing::verdict::{judge, Verdict};
use mason_testing::{ParserKind, Suite};
use std::path::Path;
use std::time::{Duration, Instant};

fn spec(
    name: &str,
    command: &str,
    parser: &str,
    pass: Option<&str>,
    fail: Option<&str>,
) -> SuiteSpec {
    SuiteSpec {
        name: name.into(),
        command: command.into(),
        parser: parser.into(),
        pass_regex: pass.map(String::from),
        fail_regex: fail.map(String::from),
        min_tests: 1,
    }
}

/// One suite for each language MASON can read the output of. (Languages without one are
/// still detected and said so; a suite for them is written by hand in `.mason/mason.toml`.)
pub fn propose(root: &Path, languages: &[String]) -> Vec<SuiteSpec> {
    let mut out = vec![];
    for l in languages {
        match l.as_str() {
            "rust" => out.push(spec(
                "rust",
                "cargo test 2>&1",
                "regex",
                Some(r"^test (.+) \.\.\. ok$"),
                Some(r"^test (.+) \.\.\. FAILED$"),
            )),
            "python" => {
                let pytest = root.join("pytest.ini").exists()
                    || root.join("conftest.py").exists()
                    || std::fs::read_to_string(root.join("pyproject.toml"))
                        .is_ok_and(|t| t.contains("pytest"));
                if pytest {
                    out.push(spec(
                        "python",
                        "python3 -m pytest -v 2>&1",
                        "pytest",
                        None,
                        None,
                    ));
                } else {
                    out.push(spec(
                        "python",
                        "python3 -m unittest discover -v 2>&1",
                        "regex",
                        Some(r"^(.+) \.\.\. ok$"),
                        Some(r"^(.+) \.\.\. (?:FAIL|ERROR)$"),
                    ));
                }
            }
            "swift" if root.join("Package.swift").exists() => {
                out.push(spec("swift", "swift test 2>&1", "xctest", None, None))
            }
            "go" => out.push(spec(
                "go",
                "go test -v ./... 2>&1",
                "regex",
                Some(r"^--- PASS: (\S+)"),
                Some(r"^--- FAIL: (\S+)"),
            )),
            _ => {}
        }
    }
    out
}

/// What the one run showed.
#[derive(Clone, Debug, PartialEq)]
pub struct Baseline {
    pub name: String,
    pub executed: usize,
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub seconds: f64,
    /// Green, with proof: parsed, at least `min_tests` ran, nothing failed.
    pub green: bool,
    /// Why it is not, in words.
    pub problem: Option<String>,
}

fn to_suite(s: &SuiteSpec) -> Option<Suite> {
    let mut suite = Suite::new(&s.name, &s.command, ParserKind::from_name(&s.parser)?);
    suite.min_tests = s.min_tests;
    suite.pass_regex = s.pass_regex.clone();
    suite.fail_regex = s.fail_regex.clone();
    Some(suite)
}

/// Judge what a run printed, the way the pipeline would. Pure, so it can be tested on text.
pub fn judge_output(s: &SuiteSpec, output: &str, exit: Option<i32>, took: Duration) -> Baseline {
    let mut b = Baseline {
        name: s.name.clone(),
        executed: 0,
        passed: 0,
        failed: 0,
        skipped: 0,
        seconds: took.as_secs_f64(),
        green: false,
        problem: None,
    };
    let Some(suite) = to_suite(s) else {
        b.problem = Some(format!("unknown parser `{}`", s.parser));
        return b;
    };
    let results: Results = match read(suite.parser, output, &suite) {
        Ok(r) => r,
        Err(e) => {
            b.problem = Some(format!("the output could not be read: {e}"));
            return b;
        }
    };
    (b.executed, b.passed, b.failed, b.skipped) = (
        results.executed(),
        results.passed(),
        results.failures().len(),
        results.skipped(),
    );
    match judge(&results, s.min_tests, exit) {
        Verdict::Green => b.green = true,
        Verdict::Vacuous { executed, needed } => {
            b.problem = Some(format!("proved nothing: {executed} test(s) found by the parser, at least {needed} needed. Either there are no tests yet or the output is in a form the `{}` parser does not read", s.parser))
        }
        Verdict::Red { why, failures } => {
            b.problem = Some(format!("red before any change: {why}{}", if failures.is_empty() { String::new() } else { format!(" ({})", failures.iter().take(3).cloned().collect::<Vec<_>>().join(", ")) }))
        }
    }
    b
}

/// Run a suite once in `dir` and judge it.
pub fn run_once(s: &SuiteSpec, dir: &Path) -> Baseline {
    let start = Instant::now();
    let out = std::process::Command::new("sh")
        .arg("-c")
        .arg(&s.command)
        .current_dir(dir)
        .output();
    let took = start.elapsed();
    match out {
        Ok(o) => {
            let mut text = String::from_utf8_lossy(&o.stdout).to_string();
            text.push_str(&String::from_utf8_lossy(&o.stderr));
            judge_output(s, &text, o.status.code(), took)
        }
        Err(e) => Baseline {
            name: s.name.clone(),
            executed: 0,
            passed: 0,
            failed: 0,
            skipped: 0,
            seconds: took.as_secs_f64(),
            green: false,
            problem: Some(format!("could not run `{}`: {e}", s.command)),
        },
    }
}

/// `.mason/baselines.toml`: what the first run showed. Written for people (and the next
/// `mason init --force`); the pipeline itself does not read it.
pub fn render_baselines(all: &[Baseline]) -> String {
    let mut s = String::from("# Written by `mason init`: what each suite showed when it was run once, before any task.\n# Informational. Re-run `mason init --force --suites accept` to refresh it.\n");
    for b in all {
        s.push_str(&format!(
            "\n[suite.{}]\nexecuted = {}\npassed = {}\nfailed = {}\nskipped = {}\nseconds = {:.1}\ngreen = {}\n",
            b.name, b.executed, b.passed, b.failed, b.skipped, b.seconds, b.green
        ));
        if let Some(p) = &b.problem {
            s.push_str(&format!("problem = {p:?}\n"));
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suites_are_proposed_by_language_and_python_follows_pytest() {
        let d = tempfile::tempdir().unwrap();
        let langs: Vec<String> = ["rust", "python", "swift", "go", "node"]
            .map(String::from)
            .to_vec();
        let p = propose(d.path(), &langs);
        // swift needs a Package.swift; node has no parser to propose.
        assert_eq!(
            p.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            ["rust", "python", "go"]
        );
        assert!(p[1].command.contains("unittest"));
        std::fs::write(d.path().join("conftest.py"), "").unwrap();
        std::fs::write(d.path().join("Package.swift"), "").unwrap();
        let p = propose(d.path(), &langs);
        assert!(p.iter().any(|s| s.name == "python" && s.parser == "pytest"));
        assert!(p.iter().any(|s| s.name == "swift" && s.parser == "xctest"));
    }

    #[test]
    fn the_proposed_regexes_read_real_output() {
        let p = propose(
            Path::new("/nonexistent"),
            &["rust".to_string(), "python".to_string(), "go".to_string()],
        );
        let t = Duration::from_millis(1500);
        let rust = "running 2 tests\ntest tests::adds ... ok\ntest tests::subs ... ok\n\ntest result: ok. 2 passed; 0 failed\n";
        let b = judge_output(&p[0], rust, Some(0), t);
        assert!(b.green && b.passed == 2 && b.seconds > 1.4, "{b:?}");
        let red = "test tests::adds ... ok\ntest tests::subs ... FAILED\n";
        let b = judge_output(&p[0], red, Some(101), t);
        assert!(!b.green && b.failed == 1 && b.problem.unwrap().contains("red before any change"));
        let py = "test_add (test_calc.CalcTests.test_add) ... ok\n\nRan 1 test in 0.000s\n\nOK\n";
        assert!(judge_output(&p[1], py, Some(0), t).green);
        let go = "=== RUN   TestAdd\n--- PASS: TestAdd (0.00s)\nPASS\n";
        assert!(judge_output(&p[2], go, Some(0), t).green);
    }

    #[test]
    fn output_with_no_tests_in_it_is_vacuous_not_green() {
        let p = propose(Path::new("/x"), &["rust".to_string()]);
        let b = judge_output(&p[0], "running 0 tests\n", Some(0), Duration::ZERO);
        assert!(!b.green && b.problem.unwrap().contains("proved nothing"));
    }

    #[test]
    fn a_suite_runs_once_for_real_and_its_baseline_is_written() {
        let s = SuiteSpec {
            name: "t".into(),
            command: "printf 'ok 1 - a\\nok 2 - b\\n'".into(),
            parser: "tap".into(),
            pass_regex: None,
            fail_regex: None,
            min_tests: 2,
        };
        let b = run_once(&s, Path::new("."));
        assert!(b.green && b.executed == 2, "{b:?}");
        let text = render_baselines(&[b]);
        assert!(
            text.contains("[suite.t]")
                && text.contains("executed = 2")
                && text.contains("green = true")
        );
        let bad = SuiteSpec {
            command: "exit 3".into(),
            ..s
        };
        let b = run_once(&bad, Path::new("."));
        assert!(!b.green);
        assert!(render_baselines(&[b]).contains("problem ="));
    }
}
