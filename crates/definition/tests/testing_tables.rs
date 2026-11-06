// SPDX-License-Identifier: GPL-3.0-or-later

//! The testing tables of a definition: what parses, and every reference that
//! must exist.

use mason_definition::{parse, validate, Capabilities};
use mason_testing::{EnvKind, ParserKind, When};

const BASE: &str = r#"schema = 1
start = "develop"

[resource.compile]
count = 2

[step.develop]
type = "agent"
category = "developing"
next = "test"

[step.test]
type = "test"
category = "testing"
suites = ["unit", "e2e"]
gates = ["assertion_scan"]
on_red = "develop"
max_rounds = 2
next = "land"

[step.land]
type = "landing"
category = "landing"

[suite.unit]
command = "cargo nextest run"
parser = "nextest"
min_tests = 10
level = "unit"
uses = { compile = 1 }
rerun = "cargo nextest run {tests}"
timeout = "20m"
test_paths = ["**/tests/**"]

[suite.e2e]
command = "npx playwright test --reporter=json"
parser = "playwright"
level = "e2e"
env = "stack"
results_file = "report.json"

[environment.stack]
type = "compose"
file = "docker-compose.yml"
health = "curl -f http://localhost:8080/health"
health_timeout = "90s"

[binding.unit]
when = ["development", "test_step"]

[binding.e2e]
when = ["batch", "after_landing"]

[impact]
"src/**" = ["unit"]
"web/**" = ["unit", "e2e"]

[signature.no-device]
pattern = "No (devices|simulators) found"
advice = "plug a device in"
"#;

fn problems(text: &str) -> Vec<String> {
    match parse(text) {
        Err(p) => p.iter().map(|x| x.to_string()).collect(),
        Ok(d) => validate(&d, &Capabilities::now())
            .iter()
            .map(|x| x.to_string())
            .collect(),
    }
}

#[test]
fn the_testing_tables_parse_into_the_model() {
    let d = parse(BASE).unwrap();
    assert!(validate(&d, &Capabilities::now()).is_empty());
    let t = &d.testing;
    let unit = t.suite("unit").unwrap();
    assert_eq!(unit.parser, ParserKind::Nextest);
    assert_eq!(unit.min_tests, 10);
    assert_eq!(unit.uses["compile"], 1);
    assert_eq!(unit.timeout.unwrap().as_secs(), 1200);
    assert_eq!(t.suite("e2e").unwrap().env, "stack");
    let e = t.environment("stack").unwrap();
    assert_eq!(e.kind, EnvKind::Compose);
    assert_eq!(e.health_timeout.as_secs(), 90);
    assert_eq!(t.bound(When::Development).len(), 1);
    assert_eq!(t.bound(When::Batch).len(), 1);
    assert_eq!(t.impact.entries.len(), 2);
    assert_eq!(t.signatures[0].name, "no-device");
    assert_eq!(d.step("test").unwrap().gates, vec!["assertion_scan"]);
}

fn with(replace: &str, by: &str) -> String {
    assert!(BASE.contains(replace), "{replace}");
    BASE.replacen(replace, by, 1)
}

#[test]
fn every_reference_must_exist_and_every_mistake_is_named() {
    for (text, want) in [
        (
            with(r#"parser = "playwright""#, r#"parser = "jest""#),
            "unknown parser `jest`",
        ),
        (
            with("min_tests = 10", r#"min_tests = "ten""#),
            "suite.unit.min_tests",
        ),
        (
            with(r#"env = "stack""#, r#"env = "nowhere""#),
            "there is no environment `nowhere`",
        ),
        (
            with("uses = { compile = 1 }", "uses = { gpu = 1 }"),
            "there is no resource `gpu`",
        ),
        (
            with(
                r#"rerun = "cargo nextest run {tests}""#,
                r#"rerun = "cargo nextest run""#,
            ),
            "must contain `{tests}`",
        ),
        (
            with(
                r#"suites = ["unit", "e2e"]"#,
                r#"suites = ["unit", "ghost"]"#,
            ),
            "there is no suite `ghost`",
        ),
        (
            with(r#"gates = ["assertion_scan"]"#, r#"gates = ["vibes"]"#),
            "unknown gate `vibes`",
        ),
        (
            with(r#""src/**" = ["unit"]"#, r#""src/**" = ["nope"]"#),
            "there is no suite `nope`",
        ),
        (
            with("[binding.unit]", "[binding.fast]"),
            "no suite has level `fast`",
        ),
        (
            with(
                r#"when = ["development", "test_step"]"#,
                r#"when = ["sometimes"]"#,
            ),
            "unknown `sometimes`",
        ),
        (
            with(
                r#"pattern = "No (devices|simulators) found""#,
                r#"pattern = "(""#,
            ),
            "not a valid pattern",
        ),
        (
            with("health_timeout = \"90s\"", "health_timeout = \"soon\""),
            "is not a duration",
        ),
        (
            with("type = \"compose\"", "type = \"simulator\""),
            "must be \"local\" or \"compose\"",
        ),
        (
            with("file = \"docker-compose.yml\"\n", ""),
            "needs its compose file",
        ),
        (
            with("timeout = \"20m\"", "timeout = \"20m\"\ncolour = \"red\""),
            "unknown key `colour`",
        ),
    ] {
        let p = problems(&text);
        assert!(
            p.iter().any(|m| m.contains(want)),
            "wanted `{want}` in {p:?}"
        );
    }
}

#[test]
fn a_suite_needs_a_command_and_a_parser_so_a_pass_can_be_proven() {
    let text = format!("{BASE}\n[suite.bare]\nlevel = \"x\"\n");
    let p = problems(&text);
    assert!(p.iter().any(|m| m.contains("suite.bare.command")), "{p:?}");
    assert!(p.iter().any(|m| m.contains("suite.bare.parser")), "{p:?}");
    let text = format!("{BASE}\n[suite.rx]\ncommand = \"x\"\nparser = \"regex\"\n");
    assert!(problems(&text)
        .iter()
        .any(|m| m.contains("needs pass_regex or fail_regex")));
}

#[test]
fn a_step_runs_suites_or_a_command_not_both() {
    let text = with(
        "gates = [\"assertion_scan\"]",
        "gates = [\"assertion_scan\"]\ncommand = \"make\"",
    );
    assert!(problems(&text)
        .iter()
        .any(|m| m.contains("either its suites or one command")));
    let text = with("suites = [\"unit\", \"e2e\"]\n", "");
    assert!(problems(&text)
        .iter()
        .any(|m| m.contains("gates judge suites")));
}
