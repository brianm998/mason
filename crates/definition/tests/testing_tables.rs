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
                r#"rerun = "cargo nextest run {tests}""#,
                r#"dev_command = "cargo nextest run""#,
            ),
            "must contain `{packages}`",
        ),
        (
            with(
                r#"rerun = "cargo nextest run {tests}""#,
                r#"dev_build = "cargo nextest run {packages}""#,
            ),
            "needs dev_command",
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

// A device environment can be kept to some devices by name, so it shares a
// machine with another pipeline's simulators without touching them.
#[test]
fn a_device_environment_takes_a_name_glob_and_an_exclude_list() {
    let text = format!(
        "{BASE}\n[environment.sim]\ntype = \"ios-simulator\"\nname = \"tdt-*\"\nexclude = [\"pipeline-slot-*\", \"ABCD-1234\"]\n"
    );
    let d = parse(&text).unwrap();
    let sim = d.testing.environment("sim").unwrap();
    assert_eq!(sim.device_name.as_deref(), Some("tdt-*"));
    assert_eq!(sim.exclude, vec!["pipeline-slot-*", "ABCD-1234"]);
}

#[test]
fn name_and_exclude_are_refused_off_a_device_environment_and_must_be_the_right_shape() {
    let local = format!("{BASE}\n[environment.db]\ntype = \"local\"\nname = \"tdt-*\"\n");
    let p = problems(&local);
    assert!(
        p.iter()
            .any(|m| m.contains("environment.db.name") && m.contains("only to a device")),
        "{p:?}"
    );
    let bad = format!(
        "{BASE}\n[environment.sim]\ntype = \"ios-simulator\"\nexclude = \"pipeline-slot-*\"\n"
    );
    let p = problems(&bad);
    assert!(
        p.iter()
            .any(|m| m.contains("environment.sim.exclude") && m.contains("list of strings")),
        "{p:?}"
    );
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
        Selection::Some(
            ["cargo-workspace", "fmt", "clippy"]
                .map(String::from)
                .to_vec()
        )
    );
    assert_eq!(pick(&["Cargo.lock"]), one("cargo-workspace"));
    // Shared client inputs run the three build suites; `exit-scripts` (s249) runs only for a
    // change under `scripts/exit/`.
    let three = Selection::Some(
        ["cargo-workspace", "swift-clients", "compose-clients"]
            .map(String::from)
            .to_vec(),
    );
    assert_eq!(pick(&["clients/tokens.toml"]), three);
    assert_eq!(pick(&["schemas/mason.proto"]), three);
    assert_eq!(
        pick(&["scripts/exit/s1.sh"]),
        Selection::Some(
            ["cargo-workspace", "exit-scripts"]
                .map(String::from)
                .to_vec()
        )
    );
    // `scripts/*.sh` maps to exit-scripts (s249), so the probe sits under no glob.
    assert_eq!(pick(&["scripts/unmapped/tool.rs"]), Selection::All);
    assert_eq!(pick(&["docs/x.md"]), one("cargo-workspace"));
    // `clippy` (s778) is picked for Rust paths only, not for client or docs changes.
    let picked = |paths: &[&str]| match pick(paths) {
        Selection::Some(v) => v.contains(&"clippy".to_string()),
        Selection::All => true,
    };
    assert!(picked(&["crates/daemon/src/a.rs"]));
    assert!(!picked(&["clients/swift/Sources/A.swift"]));
    assert!(!picked(&["docs/x.md"]));
    // `[step.test] suites` lists six. The binding is by level, though, so all eight unit-level
    // suites are bound to run_check and the test step: `guards` and `exit-guards` run under
    // run_check but are not in the step's list (s998, decision 0991). This follows the
    // definition as written; a change to the bindings updates the number on purpose.
    assert_eq!(all.len(), 6);
    assert_eq!(d.testing.bound(When::Development).len(), 8);
    assert_eq!(d.testing.bound(When::TestStep).len(), 8);
}

#[test]
fn the_narrow_keys_parse_into_one_narrow_and_their_mistakes_are_named() {
    // s386: a suite may say how to run only the tests a change needs, in a task's test step.
    let keys = r#"rerun = "cargo nextest run {tests}"
narrow_command = "cargo nextest run {tests}"
narrow_tests = "src/**/tests/*.rs"
narrow_root = "src/"
narrow_each = "-E 'test({})'"
narrow_never = ["Cargo.toml"]"#;
    let d = parse(&with(r#"rerun = "cargo nextest run {tests}""#, keys)).unwrap();
    assert!(validate(&d, &Capabilities::now()).is_empty());
    let n = d
        .testing
        .suite("unit")
        .unwrap()
        .narrow
        .clone()
        .expect("narrow");
    assert_eq!(n.command, "cargo nextest run {tests}");
    assert_eq!(n.tests, "src/**/tests/*.rs");
    assert_eq!(n.root.as_deref(), Some("src/"));
    assert_eq!(n.each, "-E 'test({})'");
    assert_eq!(n.never, ["Cargo.toml"]);
    assert!(d.testing.suite("e2e").unwrap().narrow.is_none());
    for (extra, want) in [
        (
            r#"narrow_tests = "t/*.rs""#,
            "needs both `narrow_command` and `narrow_tests`",
        ),
        (
            "narrow_command = \"cargo nextest run\"\nnarrow_tests = \"t/*.rs\"",
            "must contain `{tests}`",
        ),
        (
            "narrow_command = \"x {tests}\"\nnarrow_tests = \"t/*.rs\"\nnarrow_each = \"--tests\"",
            "must contain `{}`",
        ),
        (
            "narrow_command = \"x {tests}\"\nnarrow_tests = \"t/*.rs\"\nnarrow_split = \"(\"",
            "narrow_split",
        ),
    ] {
        let text = with(
            r#"rerun = "cargo nextest run {tests}""#,
            &format!("rerun = \"cargo nextest run {{tests}}\"\n{extra}"),
        );
        let p = problems(&text);
        assert!(p.iter().any(|x| x.contains(want)), "{extra}: {p:?}");
    }
}

#[test]
fn whole_later_and_narrow_smoke_parse_and_their_mistakes_are_named() {
    // #676
    let keys = "rerun = \"x {tests}\"\nnarrow_command = \"x {tests}\"\nnarrow_tests = \"t/*.rs\"\nnarrow_smoke = [\"a\", \"b\"]\nwhole_later = \"batch\"";
    let d = parse(&with(r#"rerun = "cargo nextest run {tests}""#, keys)).unwrap();
    let s = d.testing.suite("unit").unwrap();
    assert_eq!(s.whole_later, Some(mason_testing::WholeLater::Batch));
    assert_eq!(s.narrow.as_ref().unwrap().smoke, ["a", "b"]);
    for (extra, want) in [
        ("whole_later = \"never\"", "must be \"batch\""),
        ("narrow_smoke = [\"a\"]", "narrow_smoke"),
    ] {
        let text = with(
            r#"rerun = "cargo nextest run {tests}""#,
            &format!("rerun = \"x {{tests}}\"\n{extra}"),
        );
        let p = problems(&text);
        assert!(p.iter().any(|x| x.contains(want)), "{extra}: {p:?}");
    }
}

#[test]
fn narrow_packages_parses_and_needs_a_dev_command() {
    // s396: package-mode narrowing for a task's test step.
    let both = "dev_command = \"cargo test {packages}\"\nnarrow_packages = true";
    let d = parse(&with(
        r#"parser = "playwright""#,
        &format!("parser = \"playwright\"\n{both}"),
    ))
    .unwrap();
    assert!(d.testing.suite("e2e").is_some_and(|s| s.narrow_packages));
    let text = with(
        r#"parser = "playwright""#,
        "parser = \"playwright\"\nnarrow_packages = true",
    );
    let p = problems(&text);
    assert!(p.iter().any(|x| x.contains("needs `dev_command`")), "{p:?}");
}

// s524: MASON's own definition watches its CI on develop, with the landing freeze off.
#[test]
fn masons_own_ci_watch_is_on_with_no_freeze() {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../.mason/mason.toml"
    ))
    .unwrap();
    let d = parse(&text).unwrap();
    let w = d.watch_ci.expect("[watch.ci] is enabled");
    assert!(!w.freeze_on_gate);
    assert_eq!(w.gate.as_deref(), Some("CI"));
    assert_eq!(w.max_open_auto, 2);
}

// s684: an `[impact]` value may be a table `{ suites, override }`.
fn impact_text(body: &str) -> String {
    with(
        "\"src/**\" = [\"unit\"]\n\"web/**\" = [\"unit\", \"e2e\"]\n",
        &format!("{body}\n"),
    )
}

#[test]
fn an_override_entry_replaces_broader_impact_entries() {
    use mason_testing::Selection;
    let with_impact = |body: &str| impact_text(body);
    let t = parse(&with_impact(
        "\"scripts/**\" = [\"unit\"]\n\"scripts/exit/**\" = { suites = [], override = true }",
    ))
    .unwrap();
    let all = vec!["unit".to_string(), "e2e".to_string()];
    let pick = |p: &str| t.testing.impact.select(&[p.to_string()], &all);
    assert_eq!(pick("scripts/exit/s1.sh"), Selection::Some(vec![]));
    assert_eq!(pick("scripts/x.sh"), Selection::Some(vec!["unit".into()]));
}

#[test]
fn impact_tables_name_their_mistakes() {
    for (body, want) in [
        ("\"a/**\" = { override = true }", "impact.a/**.suites"),
        (
            "\"a/**\" = { suites = [], override = \"yes\" }",
            "impact.a/**.override",
        ),
        (
            "\"a/**\" = { suites = [\"nope\"], override = true }",
            "no suite `nope`",
        ),
    ] {
        let text = impact_text(body);
        let problems = match parse(&text) {
            Err(e) => format!("{e:?}"),
            Ok(d) => format!("{:?}", validate(&d, &Capabilities::now())),
        };
        assert!(problems.contains(want), "{body}: {problems}");
    }
}

#[test]
fn a_dead_empty_impact_entry_warns_and_an_override_does_not() {
    let warn = |body: &str| {
        let d = parse(&impact_text(body)).unwrap();
        validate::warnings(&d, &Capabilities::now())
    };
    let w = warn("\"scripts/**\" = [\"unit\"]\n\"scripts/exit/**\" = []");
    assert_eq!(w.len(), 1, "{w:?}");
    assert!(w[0].path.contains("scripts/exit/**") && w[0].message.contains("override = true"));
    assert!(warn(
        "\"scripts/**\" = [\"unit\"]\n\"scripts/exit/**\" = { suites = [], override = true }"
    )
    .is_empty());
    assert!(warn("\"docs/**\" = []\n\"src/**\" = [\"unit\"]").is_empty());
}

#[test]
fn remote_and_runs_on_parse_and_their_mistakes_are_named() {
    // #698
    let base = r#"rerun = "cargo nextest run {tests}""#;
    let d = parse(&with(
        base,
        &format!("{base}\nremote = false\nruns_on = \"macos-15\""),
    ))
    .unwrap();
    let s = d.testing.suite("unit").unwrap();
    assert!(!s.remote);
    assert_eq!(s.runs_on.as_deref(), Some("macos-15"));
    let plain = parse(&with(base, base)).unwrap();
    let s = plain.testing.suite("unit").unwrap();
    assert!(s.remote && s.runs_on.is_none());
    for (extra, want) in [
        ("runs_on = \"\"", "runs_on"),
        ("runs_on = 3", "must be a string"),
        ("remote = \"yes\"", "must be true or false"),
    ] {
        let p = problems(&with(base, &format!("{base}\n{extra}")));
        assert!(p.iter().any(|x| x.contains(want)), "{extra}: {p:?}");
    }
}

#[test]
fn shards_parse_and_need_a_shard_command() {
    // #701
    let base = r#"rerun = "cargo nextest run {tests}""#;
    let d = parse(&with(
        base,
        &format!("{base}\nshards = 8\nshard_command = \"python3 -I scripts/test-shard.py -- cargo test\""),
    ))
    .unwrap();
    let s = d.testing.suite("unit").unwrap();
    assert_eq!(s.shards, 8);
    assert!(s
        .shard_command
        .as_deref()
        .is_some_and(|c| c.contains("test-shard")));
    let plain = parse(&with(base, base)).unwrap();
    let s = plain.testing.suite("unit").unwrap();
    assert_eq!((s.shards, s.shard_command.as_deref()), (0, None));
    for (extra, want) in [
        ("shards = 8", "needs `shard_command`"),
        ("shards = 0\nshard_command = \"x\"", "from 1 to 64"),
        ("shards = 65\nshard_command = \"x\"", "from 1 to 64"),
        ("shards = \"eight\"", "from 1 to 64"),
    ] {
        let p = problems(&with(base, &format!("{base}\n{extra}")));
        assert!(p.iter().any(|x| x.contains(want)), "{extra}: {p:?}");
    }
}

#[test]
fn a_pytest_suite_narrowed_by_dotted_names_warns_and_file_does_not() {
    // #924: pytest takes paths, so `{}` alone (a dotted name) names a file that is not there.
    let warn = |each: &str| {
        let keys = format!(
            "rerun = \"cargo nextest run {{tests}}\"\nnarrow_command = \"uv run pytest -v {{tests}}\"\nnarrow_tests = \"pipeline/tests/*.py\"\nnarrow_root = \"pipeline/\"\nnarrow_each = \"{each}\""
        );
        let d = parse(&with(r#"rerun = "cargo nextest run {tests}""#, &keys)).unwrap();
        assert!(validate(&d, &Capabilities::now()).is_empty());
        mason_definition::validate::warnings(&d, &Capabilities::now())
            .iter()
            .map(|w| w.to_string())
            .collect::<Vec<_>>()
    };
    let w = warn("{}");
    assert!(w.iter().any(|x| x.contains("use {file}")), "{w:?}");
    assert!(!warn("{file}").iter().any(|x| x.contains("pytest takes")));
    assert!(!warn("{path}").iter().any(|x| x.contains("pytest takes")));
    assert!(!warn("{file}::{}")
        .iter()
        .any(|x| x.contains("pytest takes")));
}
