// SPDX-License-Identifier: GPL-3.0-or-later

//! Stage 9 end to end: suites run by a test step, verdicts proven from output,
//! reds classified before anyone is blamed, evidence reused, force skips and
//! debts, the optional gates, and the closing verdict.

mod support;
use mason_core::*;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use support::*;

/// A TAP suite with four tests whose results depend on marker files, so a
/// scripted agent can make any outcome by committing the right file.
const CHECKS: &str = r#"#!/bin/sh
[ -f skip.flag ] && { echo "1..0"; exit 0; }
echo "1..4"
echo "ok 1 - alpha"
if [ -f hello.txt ]; then echo "ok 2 - hello"; else echo "not ok 2 - hello"; fi
if [ -f legacy.broken ]; then echo "not ok 3 - legacy"; else echo "ok 3 - legacy"; fi
if [ -f mine.broken ]; then echo "not ok 4 - mine"; else echo "ok 4 - mine"; fi
if [ -f legacy.broken ] || [ -f mine.broken ] || [ ! -f hello.txt ]; then exit 1; fi
exit 0
"#;

fn def(suite_extra: &str, step_extra: &str, more: &str) -> String {
    format!(
        r#"schema = 1
start = "develop"

[resource.compile]
count = 1

[step.develop]
type = "agent"
category = "developing"
next = "test"

[step.test]
type = "test"
category = "testing"
suites = ["checks"]
on_red = "develop"
max_rounds = 4
next = "land"
{step_extra}

[step.land]
type = "landing"
category = "landing"

[suite.checks]
command = "sh checks.sh"
parser = "tap"
min_tests = 3
level = "unit"
{suite_extra}
{more}
"#
    )
}

fn commit(cmd: &str, msg: &str) -> Value {
    json!([
        {"sh": format!("{cmd} && git add -A && git -c user.name=t -c user.email=t@t commit -qm '{msg}'")},
        {"call": {"tool": "submit_change", "args": {"summary": msg, "head": "$HEAD"}}}
    ])
}

fn world(turns: Vec<Value>, files: &[(&str, &str)], definition: &str) -> Scratch {
    let s = Scratch::new("false", json!({"steps": {"develop": turns}}));
    let mut all: Vec<(&str, &str)> = vec![(".mason/mason.toml", definition), ("checks.sh", CHECKS)];
    all.extend_from_slice(files);
    s.push_files(&all, "project setup");
    s
}

fn start(s: &Scratch) -> Arc<mason_daemon::Daemon> {
    s.start_with(|c| c.env_retry = Duration::from_millis(200))
}

fn closing(d: &Arc<mason_daemon::Daemon>, t: TaskId) -> (String, String) {
    d.store
        .closing(t.0)
        .unwrap()
        .expect("a closing verdict was recorded")
}

fn file_it(d: &Arc<mason_daemon::Daemon>, title: &str) -> TaskId {
    file_task_as(d, title, "Please do it.", true, TrustSource::Owner, false)
}

const HELLO: &str = "echo hello > hello.txt && echo x > u-$(basename \"$PWD\").txt";

#[test]
fn green_needs_proof_and_a_pass_is_remembered_per_tree() {
    let s = world(vec![commit(HELLO, "add hello")], &[], &def("", "", ""));
    let d = start(&s);
    let t = file_it(&d, "Honest pass");
    wait_state(&d, t, State::Landed, 90);
    let (v, note) = closing(&d, t);
    assert_eq!(v, "verified", "{note}");
    assert!(note.contains("checks: green, with proof"), "{note}");
    // Evidence was stored for the tree the suite passed on.
    let any = d
        .store
        .read(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM evidence WHERE suite = 'checks'",
                [],
                |r| r.get::<_, i64>(0),
            )
        })
        .unwrap();
    assert_eq!(any, 1);
}

#[test]
fn a_vacuous_pass_is_red_and_goes_back_to_development() {
    // First attempt: the suite exits 0 and runs nothing. Second: it really runs.
    let s = world(
        vec![
            commit(
                "echo hello > hello.txt && echo x > skip.flag",
                "add hello (suite runs nothing)",
            ),
            commit(
                "git rm -q skip.flag && echo y > u2-$(basename \"$PWD\").txt",
                "make the suite run",
            ),
        ],
        &[],
        &def("", "", ""),
    );
    let d = start(&s);
    let t = file_it(&d, "Vacuous");
    wait_state(&d, t, State::Landed, 90);
    assert_eq!(
        steps_run(&d, t),
        vec!["develop", "test", "develop", "test", "land"]
    );
    let task = task_of(&d, t);
    assert_eq!(task.failures, 0, "a red is not a failure");
    let fb = std::fs::read_to_string(d.cfg.task_dir(t.0).join("feedback.txt"));
    // (feedback is cleared once green; the first red's text is in the step log)
    assert!(fb.is_err());
    let log: String = std::fs::read_dir(d.cfg.task_dir(t.0))
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("suites-r"))
        .flat_map(|e| {
            std::fs::read_dir(e.path())
                .unwrap()
                .flatten()
                .collect::<Vec<_>>()
        })
        .map(|e| std::fs::read_to_string(e.path()).unwrap_or_default())
        .collect();
    assert!(
        log.contains("1..0"),
        "the first run printed an empty plan: {log}"
    );
    assert_eq!(closing(&d, t).0, "verified");
}

#[test]
fn a_flaky_failure_is_classified_on_rerun_and_the_step_passes() {
    let counter = s_counter();
    let flaky = CHECKS
        .replace(
        "echo \"1..4\"",
        &format!(
            "n=$(cat {c} 2>/dev/null || echo 0); n=$((n+1)); echo $n > {c}\nif [ \"$n\" = 1 ]; then echo \"1..3\"; echo \"ok 1 - a\"; echo \"not ok 2 - flaky_one\"; echo \"ok 3 - c\"; exit 1; fi\necho \"1..4\"",
            c = counter.display()
        ),
    );
    let s = Scratch::new(
        "false",
        json!({"steps": {"develop": [commit(HELLO, "add hello")]}}),
    );
    s.push_files(
        &[
            (".mason/mason.toml", &def("", "", "")),
            ("checks.sh", &flaky),
        ],
        "setup",
    );
    let d = start(&s);
    let t = file_it(&d, "Flaky");
    wait_state(&d, t, State::Landed, 90);
    assert_eq!(
        steps_run(&d, t),
        vec!["develop", "test", "land"],
        "no trip back to development"
    );
    let (v, note) = closing(&d, t);
    assert_eq!(v, "unverified", "{note}");
    assert!(note.contains("passed only on a rerun"), "{note}");
    let flaky_log = d.store.flaky_since(0).unwrap();
    assert_eq!(flaky_log.len(), 1, "{flaky_log:?}");
    assert_eq!(flaky_log[0].1, "flaky_one");
}

fn s_counter() -> std::path::PathBuf {
    let dir = tempfile::Builder::new()
        .prefix("cnt")
        .tempdir_in("/tmp")
        .unwrap();
    let p = dir.path().join("count");
    std::mem::forget(dir);
    p
}

#[test]
fn a_baseline_failure_is_excused_but_a_new_failure_is_the_tasks() {
    // `legacy` fails at the merge base too. The task's first attempt also breaks
    // `mine`; its second does not.
    let s = world(
        vec![
            commit(
                &format!("{HELLO} && echo x > mine.broken"),
                "add hello (and break mine)",
            ),
            commit(
                "rm mine.broken && echo y > u2-$(basename \"$PWD\").txt",
                "fix mine",
            ),
        ],
        &[("legacy.broken", "known\n")],
        &def("", "", ""),
    );
    let d = start(&s);
    let t = file_it(&d, "Baseline");
    wait_state(&d, t, State::Landed, 90);
    assert_eq!(
        steps_run(&d, t),
        vec!["develop", "test", "develop", "test", "land"]
    );
    let (v, note) = closing(&d, t);
    assert_eq!(v, "unverified", "{note}");
    assert!(note.contains("failures excused"), "{note}");
    // The base's failures were worked out once and cached for that commit.
    let cached = d
        .store
        .read(|c| {
            c.query_row(
                "SELECT failing FROM baselines WHERE suite = 'checks'",
                [],
                |r| r.get::<_, String>(0),
            )
        })
        .unwrap();
    assert!(cached.lines().any(|l| l == "legacy"), "{cached}");
}

#[test]
fn only_what_the_impact_map_selects_runs_and_nothing_selected_is_untested() {
    let more = "[impact]\n\"docs/**\" = []\n\"src/**\" = [\"checks\"]\n";
    let s = world(
        vec![commit(
            "mkdir -p docs && echo words > docs/a-$(basename \"$PWD\").md",
            "docs only",
        )],
        &[],
        &def("", "", more),
    );
    let d = start(&s);
    let t = file_it(&d, "Docs");
    wait_state(&d, t, State::Landed, 90);
    let (v, note) = closing(&d, t);
    assert_eq!(v, "untested", "{note}");
    assert!(note.contains("not selected"), "{note}");
    let evidence = d
        .store
        .read(|c| c.query_row("SELECT COUNT(*) FROM evidence", [], |r| r.get::<_, i64>(0)))
        .unwrap();
    assert_eq!(evidence, 0, "no suite ran");
}

#[test]
fn an_unmapped_path_runs_everything() {
    let more = "[impact]\n\"docs/**\" = []\n";
    let s = world(vec![commit(HELLO, "unmapped")], &[], &def("", "", more));
    let d = start(&s);
    let t = file_it(&d, "Unmapped");
    wait_state(&d, t, State::Landed, 90);
    assert_eq!(closing(&d, t).0, "verified");
}

#[test]
fn identical_code_never_tests_twice() {
    // Two test steps run the same suite on the same tree: the second one reuses.
    let counter = s_counter();
    let counting = CHECKS.replace(
        "echo \"1..4\"",
        &format!("echo x >> {}\necho \"1..4\"", counter.display()),
    );
    let definition = def("", "", "").replace(
        "[step.land]",
        "[step.test2]\ntype = \"test\"\ncategory = \"testing\"\nsuites = [\"checks\"]\non_red = \"develop\"\nmax_rounds = 2\nnext = \"land\"\n\n[step.land]",
    ).replacen("next = \"land\"", "next = \"test2\"", 1);
    let s = Scratch::new(
        "false",
        json!({"steps": {"develop": [commit(HELLO, "add hello")]}}),
    );
    s.push_files(
        &[(".mason/mason.toml", &definition), ("checks.sh", &counting)],
        "setup",
    );
    let d = start(&s);
    let t = file_it(&d, "Twice");
    wait_state(&d, t, State::Landed, 90);
    assert_eq!(steps_run(&d, t), vec!["develop", "test", "test2", "land"]);
    let runs = std::fs::read_to_string(&counter).unwrap().lines().count();
    assert_eq!(runs, 1, "the suite ran once for one tree");
    let note = std::fs::read_to_string(d.cfg.task_dir(t.0).join("test-notes.txt")).unwrap();
    assert!(note.contains("green for this exact tree already"), "{note}");
    assert_eq!(closing(&d, t).0, "verified");
}

#[test]
fn a_force_skip_stops_a_running_suite_keeps_green_lanes_and_records_a_debt() {
    // `slow` takes a minute while the flag file exists.
    let flag = s_counter().with_file_name("slow.flag");
    std::fs::write(&flag, "x").unwrap();
    // It says when it has seen the flag and is sleeping, so the test skips it mid-run
    // (waiting for its host to appear was not enough: on a busy machine the script
    // could start only after the flag was gone, and pass before the skip was seen).
    let sleeping = flag.with_file_name("slow.sleeping");
    let slow = "#!/bin/sh\nif [ -f FLAG ]; then touch SLEEPING; sleep 60; fi\necho 1..2; echo 'ok 1 - a'; echo 'ok 2 - b'\n"
        .replace("FLAG", &flag.display().to_string())
        .replace("SLEEPING", &sleeping.display().to_string());
    let more = "[suite.slow]\ncommand = \"sh slow.sh\"\nparser = \"tap\"\nmin_tests = 2\nlevel = \"unit\"\n";
    let definition =
        def("", "", more).replace("suites = [\"checks\"]", "suites = [\"checks\", \"slow\"]");
    let s = Scratch::new(
        "false",
        json!({"steps": {"develop": [commit(HELLO, "add hello")]}}),
    );
    s.push_files(
        &[
            (".mason/mason.toml", &definition),
            ("checks.sh", CHECKS),
            ("slow.sh", &slow),
        ],
        "setup",
    );
    let d = start(&s);
    let t = file_it(&d, "Skip me");
    // Wait for `slow` to be running: its host is up and the script is in its sleep.
    wait_for("the slow suite to start", 60, || {
        sleeping.exists()
            && std::fs::read_dir(d.cfg.hosts_dir())
                .map(|r| {
                    r.flatten()
                        .any(|e| e.file_name().to_string_lossy().ends_with("-u-slow"))
                })
                .unwrap_or(false)
    });
    let started = std::time::Instant::now();
    let debt = mason_daemon::suites::skip(&d, t, "suite:slow", "not worth a minute", "brian", true)
        .unwrap();
    // Remove the flag so the deferred run, after landing, is quick.
    std::fs::remove_file(&flag).unwrap();
    wait_state(&d, t, State::Landed, 60);
    assert!(
        started.elapsed() < Duration::from_secs(40),
        "the running suite was stopped"
    );
    let (v, note) = closing(&d, t);
    assert_eq!(v, "unverified", "{note}");
    assert!(
        note.contains("checks: green, with proof") && note.contains("slow: skipped by force"),
        "{note}"
    );
    // The debt exists and is paid by the deferred run after landing.
    let debts = d.store.debts(Some(t.0), false).unwrap();
    assert_eq!(debts.len(), 1);
    assert_eq!(
        (debts[0].id, debts[0].scope.as_str(), debts[0].deferred),
        (debt, "suite:slow", true)
    );
    wait_for("the deferred run to pay the debt", 60, || {
        d.store.debts(Some(t.0), false).unwrap()[0].status == "paid"
    });
}

#[test]
fn a_skip_ends_the_wait_for_a_build_slot_and_takes_no_slot() {
    // On 2026-11-13 #288's test step was skipped while it waited for a compile slot; it
    // waited on until a slot came, then built, and only then noticed the skip.
    let definition = def("build = \"true\"", "", "");
    let s = world(vec![commit(HELLO, "add hello")], &[], &definition);
    let d = start(&s);
    // Something outside the step holds the only compile slot for the whole test.
    let uses = [("compile".to_string(), 1)].into();
    let id = d.broker.lease_open("outside build", &uses);
    let held = {
        let mut held = None;
        for _ in 0..2_000 {
            if let mason_daemon::sched::LeaseState::Granted {
                classes,
                exclusive,
                capacity,
                locks_dir,
            } = d.broker.lease_poll(id)
            {
                let locks = mason_resources::Locks::new(locks_dir).unwrap();
                let want = mason_resources::Take { classes, exclusive };
                if let Ok(h) = locks.take(id, "outside build", 0, &want, &capacity) {
                    held = Some(h);
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        held.expect("the outside build got the compile slot")
    };
    d.broker.lease_poll(id);
    let t = file_it(&d, "Skip me while I wait");
    wait_for("the test step to wait for compile", 90, || {
        state_of(&d, t) == State::Testing
            && d.broker
                .explain_task(t)
                .is_some_and(|w| w.contains("compile"))
    });
    mason_daemon::suites::skip(&d, t, "step:test", "too many tests", "brian", false).unwrap();
    // It lands with the slot still held outside: the skip ended its wait.
    wait_state(&d, t, State::Landed, 90);
    assert_eq!(
        d.broker.used("compile"),
        1,
        "only the outside build holds it"
    );
    let (v, note) = closing(&d, t);
    assert_eq!(v, "unverified", "{note}");
    assert!(note.contains("skipped by force"), "{note}");
    drop(held);
    d.broker.lease_close(id);
}

#[test]
fn an_environmental_failure_is_a_wait_charged_to_no_task() {
    let counter = s_counter();
    let script = CHECKS.replace(
        "echo \"1..4\"",
        &format!(
            "n=$(cat {c} 2>/dev/null || echo 0); n=$((n+1)); echo $n > {c}\nif [ \"$n\" -le 2 ]; then echo \"No devices found\"; echo \"1..0\"; exit 1; fi\necho \"1..4\"",
            c = counter.display()
        ),
    );
    let more =
        "[signature.no-device]\npattern = \"No devices found\"\nadvice = \"plug a phone in\"\n";
    let s = Scratch::new(
        "false",
        json!({"steps": {"develop": [commit(HELLO, "add hello")]}}),
    );
    s.push_files(
        &[
            (".mason/mason.toml", &def("", "", more)),
            ("checks.sh", &script),
        ],
        "setup",
    );
    let d = start(&s);
    let t = file_it(&d, "No phone");
    let mut saw = String::new();
    wait_for("an environmental wait", 60, || {
        if let Activity::Waiting(WaitReason::On(w)) = task_of(&d, t).activity {
            if w.contains("environment problem") {
                saw = w;
                return true;
            }
        }
        false
    });
    assert!(
        saw.contains("not the task's fault")
            && saw.contains("no-device")
            && saw.contains("plug a phone in"),
        "{saw}"
    );
    wait_state(&d, t, State::Landed, 60);
    let task = task_of(&d, t);
    assert_eq!(task.failures, 0);
    assert_eq!(
        steps_run(&d, t),
        vec!["develop", "test", "land"],
        "sent nowhere, blamed on nobody"
    );
}

#[test]
fn a_quarantined_failure_is_excused_with_an_owner_and_an_expiry() {
    let s = world(
        vec![commit(
            &format!("{HELLO} && echo x > mine.broken"),
            "break mine",
        )],
        &[],
        &def("", "", ""),
    );
    let d = start(&s);
    let now = mason_daemon::daemon::now_ms();
    d.store
        .put_quarantine(
            "checks",
            "mine",
            "brian",
            now + 86_400_000,
            "known flaky",
            now,
        )
        .unwrap();
    let t = file_it(&d, "Quarantined");
    wait_state(&d, t, State::Landed, 90);
    let (v, note) = closing(&d, t);
    assert_eq!(v, "unverified", "{note}");
    assert!(note.contains("failures excused"));
    // An expired quarantine excuses nothing.
    assert!(d
        .store
        .quarantined(now + 2 * 86_400_000)
        .unwrap()
        .is_empty());
}

#[test]
fn the_assertion_scan_wants_a_weakening_explained() {
    // The suite itself is `tests/t.sh`, which the task weakens by deleting an assertion.
    let tsh =
        "#!/bin/sh\nassert_one=1\nassert_two=2\necho 1..2\necho 'ok 1 - one'\necho 'ok 2 - two'\n";
    let more = "";
    let definition = def(
        "test_paths = [\"tests/**\"]",
        "gates = [\"assertion_scan\"]",
        more,
    )
    .replace("command = \"sh checks.sh\"", "command = \"sh tests/t.sh\"")
    .replace("min_tests = 3", "min_tests = 2");
    let weaken = "sed -i.bak '/assert_two/d' tests/t.sh && rm tests/t.sh.bak && echo x > u-$(basename \"$PWD\").txt";
    let s = Scratch::new(
        "false",
        json!({"steps": {"develop": [
            commit(weaken, "tidy up"),
            commit("echo y > u2-$(basename \"$PWD\").txt", "Removed the second assertion because it duplicated the first one"),
        ]}}),
    );
    s.push_files(
        &[(".mason/mason.toml", &definition), ("tests/t.sh", tsh)],
        "setup",
    );
    let d = start(&s);
    let t = file_it(&d, "Weaken");
    wait_state(&d, t, State::Landed, 90);
    // First attempt: weakened and unexplained, sent back. Second: explained in a commit message.
    assert_eq!(
        steps_run(&d, t),
        vec!["develop", "test", "develop", "test", "land"]
    );
}

#[test]
fn the_acceptance_gate_wants_evidence_for_every_criterion() {
    let definition = def("", "gates = [\"acceptance\"]", "");
    let script = json!({"steps": {
    "develop": [
        commit(HELLO, "no table"),
        commit("mkdir -p .mason && echo '{\"hello.txt exists\": [\"hello\"]}' > .mason/acceptance.json", "add the table"),
    ]}});
    let s = Scratch::new("false", script);
    s.push_files(
        &[(".mason/mason.toml", &definition), ("checks.sh", CHECKS)],
        "setup",
    );
    let d = start(&s);
    // The plan (with its criteria) is what an exploring agent would have submitted.
    let t = file_it(&d, "Acceptance");
    let dir = d.cfg.task_dir(t.0);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("plan.json"),
        r#"{"acceptance_criteria": ["hello.txt exists"]}"#,
    )
    .unwrap();
    wait_state(&d, t, State::Landed, 90);
    assert_eq!(
        steps_run(&d, t),
        vec!["develop", "test", "develop", "test", "land"]
    );
}

#[test]
fn negative_control_wants_new_tests_to_fail_against_the_old_code() {
    // Tests are files under tests/; each says what it needs. At the base a test
    // that needs nothing passes. The first attempt adds one: caught.
    let runner = "#!/bin/sh\nn=0\nfor f in tests/*.t; do\n  n=$((n+1))\n  need=$(cat \"$f\")\n  if [ -z \"$need\" ] || [ -f \"$need\" ]; then echo \"ok $n - $(basename $f)\"; else echo \"not ok $n - $(basename $f)\"; fi\ndone\necho 1..$n\n";
    let definition = def(
        "test_paths = [\"tests/**\"]\nmin_tests = 1",
        "gates = [\"negative_control\"]",
        "",
    )
    .replace("command = \"sh checks.sh\"", "command = \"sh run.sh\"")
    .replace("min_tests = 3\n", "");
    let script = json!({"steps": {"develop": [
        commit("mkdir -p tests && : > tests/trivial.t && echo f > feature.txt && echo x > u-$(basename \"$PWD\").txt", "add a test that needs nothing"),
        commit("printf feature.txt > tests/trivial.t && echo z > u3-$(basename \"$PWD\").txt", "make the test need the feature"),
    ]}});
    let s = Scratch::new("false", script);
    s.push_files(
        &[
            (".mason/mason.toml", &definition),
            ("run.sh", runner),
            ("tests/old.t", ""),
        ],
        "setup",
    );
    let d = start(&s);
    let t = file_it(&d, "Negative control");
    wait_state(&d, t, State::Landed, 120);
    assert_eq!(
        steps_run(&d, t),
        vec!["develop", "test", "develop", "test", "land"]
    );
}

#[test]
fn a_definition_whose_suites_name_a_missing_parser_is_refused_and_the_old_one_runs() {
    // (The validator's rules are table-tested in the definition crate; here we
    // check the daemon keeps running the last good definition.)
    let s = world(vec![commit(HELLO, "add hello")], &[], &def("", "", ""));
    let d = start(&s);
    let bad = def("", "", "").replace("parser = \"tap\"", "parser = \"jest\"");
    s.push_files(&[(".mason/mason.toml", &bad)], "break the definition");
    let pid = d.project_id_by_name("scratch").unwrap();
    assert!(matches!(
        d.reload_definition(pid),
        mason_daemon::defs::Reload::Refused(_)
    ));
    let t = file_it(&d, "Still fine");
    wait_state(&d, t, State::Landed, 90);
}
