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
            "must be \"local\", \"compose\", \"ios-simulator\"",
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

#[test]
fn a_build_goes_with_one_command_not_with_suites() {
    let text = with(
        "gates = [\"assertion_scan\"]",
        "gates = [\"assertion_scan\"]\nbuild = \"cargo test --no-run\"",
    );
    assert!(problems(&text)
        .iter()
        .any(|m| m.contains("step.test.build") && m.contains("suites build in their own")));
}

// Stage 25: the four device environment types parse, and the old refusal of
// them ("simulators, emulators and devices arrive in a later stage") is gone.
#[test]
fn device_environments_parse_with_an_optional_os_prefix() {
    let text = format!(
        "{BASE}\n[environment.phone]\ntype = \"ios-device\"\nos = \"17\"\nsetup = \"./install.sh\"\n[environment.emu]\ntype = \"android-emulator\"\n"
    );
    let d = parse(&text).unwrap();
    let phone = d.testing.environment("phone").unwrap();
    assert_eq!(
        phone.kind,
        EnvKind::Device(mason_testing::DeviceType::IosDevice)
    );
    assert_eq!(phone.os.as_deref(), Some("17"));
    assert_eq!(
        d.testing.environment("emu").unwrap().kind,
        EnvKind::Device(mason_testing::DeviceType::AndroidEmulator)
    );
    assert!(validate(&d, &Capabilities::now()).is_empty());
}

#[test]
fn an_unknown_environment_type_lists_the_real_ones() {
    let text = format!("{BASE}\n[environment.x]\ntype = \"toaster\"\n");
    let p = problems(&text);
    assert!(
        p.iter()
            .any(|m| m.contains("ios-simulator") && m.contains("android-device")),
        "{p:?}"
    );
}

#[test]
fn built_in_mobile_signatures_go_only_to_projects_with_a_device_environment() {
    let plain = parse(BASE).unwrap();
    assert_eq!(
        plain.testing.signatures.len(),
        1,
        "a project without a device environment is unchanged"
    );
    let text = format!("{BASE}\n[environment.phone]\ntype = \"ios-device\"\n");
    let with_device = parse(&text).unwrap();
    let names: Vec<&str> = with_device
        .testing
        .signatures
        .iter()
        .map(|s| s.name.as_str())
        .collect();
    assert!(names.contains(&"expired-certificate") && names.contains(&"device-offline-or-locked"));
    assert_eq!(
        names.iter().filter(|n| **n == "no-device").count(),
        1,
        "the project's own no-device stays the only one"
    );
}

#[test]
fn the_device_sample_definition_is_valid_and_gets_the_mobile_signatures() {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/examples/device-sample/.mason/mason.toml"
    ))
    .unwrap();
    let d = parse(&text).unwrap();
    assert!(validate(&d, &Capabilities::now()).is_empty());
    assert_eq!(d.testing.environments.len(), 4);
    assert!(d
        .testing
        .signatures
        .iter()
        .any(|s| s.name == "expired-certificate"));
}

#[test]
fn pty_is_a_bool_on_a_suite_and_a_step_and_off_when_absent() {
    let d = parse(BASE).unwrap();
    assert!(!d.testing.suite("unit").unwrap().pty);
    assert!(!d.step("test").unwrap().pty);
    let on = with(
        "level = \"unit\"\nuses",
        "level = \"unit\"\npty = true\nuses",
    );
    assert!(parse(&on).unwrap().testing.suite("unit").unwrap().pty);
    let step = with(
        "gates = [\"assertion_scan\"]",
        "gates = [\"assertion_scan\"]\npty = true",
    );
    assert!(parse(&step).unwrap().step("test").unwrap().pty);
    for bad in [
        with(
            "level = \"unit\"\nuses",
            "level = \"unit\"\npty = \"yes\"\nuses",
        ),
        with(
            "gates = [\"assertion_scan\"]",
            "gates = [\"assertion_scan\"]\npty = 1",
        ),
    ] {
        let p = problems(&bad);
        assert!(p.iter().any(|x| x.contains("pty")), "{p:?}");
    }
}

#[test]
fn suites_bound_to_the_test_step_but_not_listed_warn() {
    let warn = |t: &str| validate::warnings(&parse(t).unwrap(), &Capabilities::now());
    let bare = BASE.replace("suites = [\"unit\", \"e2e\"]\n", "");
    let w = warn(&bare);
    assert_eq!(w.len(), 1, "{w:?}");
    assert!(w[0].message.contains("`test`") && w[0].message.contains("`unit`"));
    assert!(!w[0].message.contains("`e2e`"), "{w:?}");
    assert!(warn(
        &bare
            .replace("type = \"test\"", "type = \"test\"\nsuites = [\"a\"]")
            .replace(
                "[suite.unit]",
                "[suite.a]\ncommand = \"x\"\nparser = \"nextest\"\nlevel = \"unit\"\n[suite.unit]"
            )
    )
    .is_empty());
    assert!(warn(BASE).is_empty());
    let unbound = bare.replace(
        "when = [\"development\", \"test_step\"]",
        "when = [\"development\"]",
    );
    assert!(warn(&unbound).is_empty());
}

// s178: a suite may build first, holding `compile` only for the build.
#[test]
fn a_suite_may_have_its_own_build_and_it_must_not_be_empty() {
    let text = with(
        "command = \"cargo nextest run\"\n",
        "command = \"cargo nextest run\"\nbuild = \"cargo nextest run --no-run\"\n",
    );
    let d = parse(&text).unwrap();
    assert!(validate(&d, &Capabilities::now()).is_empty());
    assert_eq!(
        d.testing.suite("unit").unwrap().build.as_deref(),
        Some("cargo nextest run --no-run")
    );
    assert_eq!(d.testing.suite("e2e").unwrap().build, None);
    let text = with(
        "command = \"cargo nextest run\"\n",
        "command = \"cargo nextest run\"\nbuild = \"  \"\n",
    );
    assert!(problems(&text)
        .iter()
        .any(|m| m.contains("suite.unit.build") && m.contains("must not be empty")));
    let text = with(
        "command = \"cargo nextest run\"\n",
        "command = \"cargo nextest run\"\nbuild = 3\n",
    );
    assert!(problems(&text)
        .iter()
        .any(|m| m.contains("suite.unit.build") && m.contains("must be a string")));
}

// s178: MASON's own definition runs three suites, and the impact map picks them by path.
#[test]
fn masons_own_impact_map_selects_suites_by_path() {
    use mason_testing::Selection;
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../.mason/mason.toml"
    ))
    .unwrap();
    let d = parse(&text).unwrap();
    assert!(validate(&d, &Capabilities::now()).is_empty());
    let step = d.step("test").unwrap();
    let all = step.suites.clone();
    let pick = |paths: &[&str]| {
        let p: Vec<String> = paths.iter().map(|s| s.to_string()).collect();
        d.testing.impact.select(&p, &all)
    };
    let one = |n: &str| Selection::Some(vec![n.to_string()]);
    assert_eq!(
        pick(&["clients/swift/Sources/A.swift"]),
        one("swift-clients")
    );
    assert_eq!(pick(&["clients/compose/core/B.kt"]), one("compose-clients"));
    assert_eq!(
        pick(&[
            "crates/daemon/src/a.rs",
            "Cargo.lock",
            "rust-toolchain.toml"
        ]),
        one("cargo-workspace")
    );
    assert_eq!(pick(&["clients/tokens.toml"]), Selection::Some(all.clone()));
    assert_eq!(pick(&["schemas/mason.proto"]), Selection::Some(all.clone()));
    assert_eq!(pick(&["scripts/unmapped.sh"]), Selection::All);
    assert_eq!(pick(&["docs/x.md"]), one("cargo-workspace"));
    // Every suite builds first, and all are bound to run_check and the test step.
    for s in &all {
        assert!(d.testing.suite(s).unwrap().build.is_some(), "{s}");
    }
    assert_eq!(d.testing.bound(When::Development).len(), 3);
    assert_eq!(d.testing.bound(When::TestStep).len(), 3);
}
