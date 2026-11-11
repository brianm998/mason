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
            "swift" => {
                if let Some(cmd) = xcodebuild_command(root) {
                    out.push(spec("swift", &cmd, "xctest", None, None))
                }
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

/// A target in a `project.pbxproj`: its name and product type.
fn native_targets(pbxproj: &str) -> Vec<(String, String)> {
    let field = |block: &str, key: &str| -> Option<String> {
        let prefix = format!("{key} = ");
        let v = block.lines().find_map(|l| l.trim().strip_prefix(&prefix))?;
        Some(v.trim_end_matches(';').trim_matches('"').to_string())
    };
    pbxproj
        .split("isa = PBXNativeTarget;")
        .skip(1)
        .filter_map(|rest| {
            // The block's own fields come before its closing `};`.
            let block = &rest[..rest.find("\n\t\t};").unwrap_or(rest.len())];
            Some((field(block, "name")?, field(block, "productType")?))
        })
        .collect()
}

fn shell_quote(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./+".contains(c))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

/// `xcodebuild test` for a macOS app's Xcode project, from the folder it is in. The scheme is
/// the shared one named after the app target if there is one, else the only shared one, else
/// the app target (xcodebuild makes a scheme for each target when none is shared). UI-test
/// targets are skipped: they drive the real app on the screen, which an unattended run cannot
/// count on. None for an iOS project, which needs a simulator (the device-mobile workflow).
fn xcodebuild_command(root: &Path) -> Option<String> {
    let proj = super::detect::xcode_project(root)?;
    let pbx = std::fs::read_to_string(root.join(&proj).join("project.pbxproj")).ok()?;
    if !pbx.contains("SDKROOT = macosx;") || pbx.contains("SDKROOT = iphoneos;") {
        return None;
    }
    let targets = native_targets(&pbx);
    let app = targets
        .iter()
        .find(|(_, t)| t.contains("com.apple.product-type.application"))
        .map(|(n, _)| n.clone())?;
    let mut shared: Vec<String> =
        std::fs::read_dir(root.join(&proj).join("xcshareddata/xcschemes"))
            .map(|rd| {
                rd.flatten()
                    .filter_map(|e| {
                        let p = e.path();
                        (p.extension()? == "xcscheme")
                            .then(|| p.file_stem().map(|s| s.to_string_lossy().to_string()))?
                    })
                    .collect()
            })
            .unwrap_or_default();
    shared.sort();
    let scheme = if shared.contains(&app) || shared.len() != 1 {
        app
    } else {
        shared.remove(0)
    };
    let name = proj.file_name()?.to_string_lossy().to_string();
    let mut cmd = String::new();
    if let Some(dir) = proj.parent().filter(|d| !d.as_os_str().is_empty()) {
        cmd.push_str(&format!("cd {} && ", shell_quote(&dir.to_string_lossy())));
    }
    cmd.push_str(&format!(
        "xcodebuild test -project {} -scheme {} -destination 'platform=macOS'",
        shell_quote(&name),
        shell_quote(&scheme)
    ));
    for (n, _) in targets
        .iter()
        .filter(|(_, t)| t.contains("com.apple.product-type.bundle.ui-testing"))
    {
        cmd.push_str(&format!(" -skip-testing:{}", shell_quote(n)));
    }
    cmd.push_str(" 2>&1");
    Some(cmd)
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

    /// A trimmed `project.pbxproj`: an app, its unit tests and its UI tests.
    fn pbxproj(sdk: &str) -> String {
        let target = |name: &str, ty: &str| {
            format!(
                "\t\tAA /* {name} */ = {{\n\t\t\tisa = PBXNativeTarget;\n\t\t\tbuildPhases = (\n\t\t\t\tBB /* Sources */,\n\t\t\t);\n\t\t\tname = {name};\n\t\t\tproductName = {name};\n\t\t\tproductType = \"com.apple.product-type.{ty}\";\n\t\t}};\n"
            )
        };
        format!(
            "{}{}{}\t\t\t\tSDKROOT = {sdk};\n",
            target("Disk Free", "application"),
            target("DiskFreeTests", "bundle.unit-test"),
            target("DiskFreeUITests", "bundle.ui-testing"),
        )
    }

    fn xcode_repo(sdk: &str) -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let proj = d.path().join("App/Disk Free.xcodeproj");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join("project.pbxproj"), pbxproj(sdk)).unwrap();
        d
    }

    #[test]
    fn a_macos_xcode_app_one_folder_down_gets_an_xcodebuild_suite() {
        let d = xcode_repo("macosx");
        let langs = super::super::detect::languages(d.path());
        assert_eq!(langs, ["swift"]);
        let p = propose(d.path(), &langs);
        assert_eq!(p.len(), 1, "{p:?}");
        assert_eq!(p[0].parser, "xctest");
        assert_eq!(
            p[0].command,
            "cd App && xcodebuild test -project 'Disk Free.xcodeproj' -scheme 'Disk Free' \
             -destination 'platform=macOS' -skip-testing:DiskFreeUITests 2>&1"
        );
        // The only shared scheme is used when none is named after the app.
        let schemes = d
            .path()
            .join("App/Disk Free.xcodeproj/xcshareddata/xcschemes");
        std::fs::create_dir_all(&schemes).unwrap();
        std::fs::write(schemes.join("Release.xcscheme"), "").unwrap();
        assert!(propose(d.path(), &langs)[0]
            .command
            .contains("-scheme Release "));
        std::fs::write(schemes.join("Disk Free.xcscheme"), "").unwrap();
        assert!(propose(d.path(), &langs)[0]
            .command
            .contains("-scheme 'Disk Free' "));
    }

    #[test]
    fn an_ios_xcode_app_gets_no_suite_it_needs_a_simulator() {
        let d = xcode_repo("iphoneos");
        assert!(propose(d.path(), &["swift".to_string()]).is_empty());
    }

    #[test]
    fn what_xcodebuild_prints_for_an_app_hosted_bundle_is_green() {
        let d = xcode_repo("macosx");
        let p = propose(d.path(), &["swift".to_string()]);
        let out = "Testing started\nTest suite 'DiskFreeTests' started on 'My Mac - DiskFree (6603)'\nTest case 'DiskFreeTests/example()' passed on 'My Mac - DiskFree (6603)' (0.000 seconds)\n";
        let b = judge_output(&p[0], out, Some(0), Duration::from_secs(1));
        assert!(b.green && b.passed == 1, "{b:?}");
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
