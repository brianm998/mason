// SPDX-License-Identifier: GPL-3.0-or-later

//! Stage 9 end to end: suites run by a test step, verdicts proven from output,
//! reds classified before anyone is blamed, evidence reused, force skips and
//! debts, the optional gates, and the closing verdict.

use crate::support;
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
    // The known-red ledger has one entry for it, and one fix task was filed.
    let ledger = d.store.ledger().unwrap();
    assert_eq!(ledger.len(), 1, "{ledger:?}");
    assert_eq!(
        (ledger[0].suite.as_str(), ledger[0].test.as_str()),
        ("checks", "legacy")
    );
    assert_eq!(ledger[0].source, "base");
    let fixes: Vec<_> = d
        .world()
        .unwrap()
        .tasks
        .values()
        .filter(|t| t.title.starts_with("legacy fails on "))
        .map(|t| t.id.0)
        .collect();
    assert_eq!(fixes.len(), 1, "{fixes:?}");
    assert_eq!(ledger[0].fix_task, Some(fixes[0]));
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
    let debt = mason_daemon::suites::skip(&d, t, "suite:slow", "not worth a minute", "brian", true)
        .unwrap();
    // Remove the flag so the deferred run, after landing, is quick.
    std::fs::remove_file(&flag).unwrap();
    // The suite sleeps 60 s; landing within this wait means it was stopped, not waited out.
    wait_state(&d, t, State::Landed, 50);
    assert!(
        sleeping.exists(),
        "the slow suite was running when it was skipped"
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
    // The skip ended its wait and took no slot: the task is on to landing with the slot still
    // held outside.
    wait_state(&d, t, State::Landing, 90);
    assert_eq!(
        d.broker.used("compile"),
        1,
        "only the outside build holds it"
    );
    // Landing builds the batch before it promotes, whatever was skipped (the build floor), under
    // a compile slot of its own, so the task lands once the outside build lets go.
    drop(held);
    d.broker.lease_close(id);
    wait_state(&d, t, State::Landed, 90);
    let (v, note) = closing(&d, t);
    assert_eq!(v, "unverified", "{note}");
    assert!(note.contains("skipped by force"), "{note}");
    wait_for("the landing build to give its slot back", 60, || {
        d.broker.used("compile") == 0
    });
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

#[test]
fn paying_a_debt_writes_in_the_projects_test_command() {
    // On 2026-11-13 `mason debt run` on mason's own test step failed at once: the debt run
    // gave `{project_test}` to the shell as a command (exit 127), so no debt could be paid.
    let definition =
        def("", "", "").replace("command = \"sh checks.sh\"", "command = \"{project_test}\"");
    let s = Scratch::new(
        "sh checks.sh",
        json!({"steps": {"develop": [commit(HELLO, "add hello")]}}),
    );
    s.push_files(
        &[(".mason/mason.toml", &definition), ("checks.sh", CHECKS)],
        "project setup",
    );
    let d = start(&s);
    let t = file_it(&d, "Test me later");
    wait_state(&d, t, State::Landed, 120);
    // A new tip, so the debt run cannot reuse the green the test step left for the landed tree.
    s.push_files(&[("later.txt", "later\n")], "a later change");
    let debt = mason_daemon::suites::skip(&d, t, "suite:checks", "owed", "brian", false).unwrap();
    let paid = mason_daemon::suites::pay(&d, debt);
    assert!(
        paid.as_deref().is_ok_and(|m| m.starts_with("paid")),
        "{paid:?}"
    );
    assert_eq!(d.store.debts(Some(t.0), false).unwrap()[0].status, "paid");
}

#[test]
fn a_debt_payment_waits_for_the_test_steps_slot_and_runs_once_it_is_free() {
    // On 2026-11-18 a landing of nine test-skipped tasks started seven full test runs at
    // once, one per debt, holding no broker slot, and the Mac hung until its watchdog reset
    // it. A payment now queues for what the test step `uses`, like the test step itself.
    let runs = tempfile::tempdir().unwrap();
    let log = runs.path().join("ran");
    let definition = def("", "uses = { compile = 1 }", "").replace(
        "command = \"sh checks.sh\"",
        &format!(
            "command = \"echo debt >> {} && sh checks.sh\"",
            log.display()
        ),
    );
    let s = Scratch::new(
        "sh checks.sh",
        json!({"steps": {"develop": [commit(HELLO, "add hello")]}}),
    );
    s.push_files(
        &[(".mason/mason.toml", &definition), ("checks.sh", CHECKS)],
        "project setup",
    );
    let d = start(&s);
    let t = file_it(&d, "Test me later");
    wait_state(&d, t, State::Landed, 120);
    s.push_files(&[("later.txt", "later\n")], "a later change");
    let debt = mason_daemon::suites::skip(&d, t, "suite:checks", "owed", "brian", false).unwrap();
    let _ = std::fs::remove_file(&log);
    let Ok(held) = d.broker.acquire(
        t,
        "#holder",
        mason_scheduler::Rank::new(false, 0, 0, 1, 1, false),
        mason_scheduler::Needs::of(&[("compile", 1)]),
        &|| true,
        &mut |_| {},
    ) else {
        panic!("the holder was not granted the compile slot")
    };
    let payer = {
        let d = d.clone();
        std::thread::spawn(move || mason_daemon::suites::pay(&d, debt))
    };
    let label = format!("debt #{debt}");
    wait_for("the payment to queue for the slot", 60, || {
        d.broker.snapshot().to_string().contains(&label)
    });
    assert!(
        !log.exists(),
        "the debt ran its suite while the test step's slot was held"
    );
    drop(held);
    let paid = payer.join().unwrap();
    assert!(
        paid.as_deref().is_ok_and(|m| m.starts_with("paid")),
        "{paid:?}"
    );
    assert!(log.exists(), "the payment never ran the suite");
}

#[test]
fn paying_a_debt_runs_the_worktree_setup_first() {
    // On 2026-11-14 star's base checks could never build: MASON's own checkouts skipped the
    // `setup` command step that links star's gitignored OpenCV into a worktree (s312).
    let definition = def("", "", "")
        .replace("start = \"develop\"", "start = \"setup\"")
        .replace(
            "[step.develop]",
            "[step.setup]\ntype = \"command\"\ncategory = \"developing\"\ncommand = \"touch linked.flag\"\nnext = \"develop\"\n\n[step.develop]",
        )
        .replace(
            "command = \"sh checks.sh\"",
            "command = \"test -f linked.flag && sh checks.sh\"",
        );
    let s = Scratch::new(
        "sh checks.sh",
        json!({"steps": {"develop": [commit(HELLO, "add hello")]}}),
    );
    s.push_files(
        &[
            (".mason/mason.toml", &definition),
            ("checks.sh", CHECKS),
            (".gitignore", "linked.flag\n"),
        ],
        "project setup",
    );
    let d = start(&s);
    let t = file_it(&d, "Test me later");
    wait_state(&d, t, State::Landed, 120);
    s.push_files(&[("later.txt", "later\n")], "a later change");
    let debt = mason_daemon::suites::skip(&d, t, "suite:checks", "owed", "brian", false).unwrap();
    let paid = mason_daemon::suites::pay(&d, debt);
    assert!(
        paid.as_deref().is_ok_and(|m| m.starts_with("paid")),
        "the payment's checkout had no linked.flag: {paid:?}"
    );
}

#[test]
fn a_task_test_step_runs_only_the_tests_its_change_needs_and_proves_nothing_for_the_tree() {
    // s386: star #216 ran all 808 desktop tests for a 48-line change. A suite with narrow keys
    // runs only the test units the change touches in a task's test step.
    let runs = tempfile::tempdir().unwrap();
    let log = runs.path().join("ran");
    let definition = def(
        &format!(
            "narrow_command = \"echo narrowed {{tests}} >> {l}; echo 1..1; echo ok 1 - picked\"\n\
             narrow_tests = \"tests/*_test.sh\"\n\
             narrow_root = \"tests/\"",
            l = log.display()
        ),
        "",
        "",
    );
    let s = Scratch::new(
        "sh checks.sh",
        json!({"steps": {"develop": [commit(&format!("{HELLO} && echo '# changed' >> tests/a_test.sh"), "change a")]}}),
    );
    s.push_files(
        &[
            (".mason/mason.toml", &definition),
            ("checks.sh", CHECKS),
            ("tests/a_test.sh", "# a\n"),
            ("tests/b_test.sh", "# b\n"),
            ("tests/c_test.sh", "# c\n"),
        ],
        "project setup",
    );
    let d = start(&s);
    let t = file_it(&d, "Narrowed");
    wait_state(&d, t, State::Landed, 120);
    let ran = std::fs::read_to_string(&log).unwrap_or_default();
    assert_eq!(
        ran.trim(),
        "narrowed a_test",
        "only the edited test unit ran: {ran:?}"
    );
    let (_, note) = closing(&d, t);
    assert!(note.contains("narrowed to 1 of 3 test units"), "{note}");
}

const WHOLE_LATER_LANDING: &str =
    "mode = \"direct\"\nwindow = \"1s\"\nmax_size = 1\n[[tier]]\nname = \"batch\"\nsuites = [\"checks\"]\n";

fn whole_later_world(extra: &str, log: &std::path::Path) -> (Scratch, Arc<mason_daemon::Daemon>) {
    let definition = def(
        &format!(
            "narrow_command = \"echo narrowed {{tests}} >> {l}; echo 1..1; echo ok 1 - picked\"\n\
             narrow_tests = \"tests/*_test.sh\"\n\
             narrow_root = \"tests/\"\n\
             whole_later = \"batch\"\n{extra}",
            l = log.display()
        ),
        "",
        "",
    );
    let s = Scratch::new(
        "sh checks.sh",
        json!({"steps": {"develop": [commit(HELLO, "change a file no test names")]}}),
    );
    s.push_files(
        &[
            (".mason/mason.toml", &definition),
            (".mason/landing.toml", WHOLE_LATER_LANDING),
            ("checks.sh", CHECKS),
            ("tests/a_test.sh", "# a\n"),
            ("tests/b_test.sh", "# b\n"),
            ("tests/c_test.sh", "# c\n"),
        ],
        "project setup",
    );
    let d = start(&s);
    let pid = d.project_id_by_name("scratch").unwrap();
    let cfg = mason_daemon::landing::load_config(&d, pid);
    assert!(cfg.is_ok(), "landing config: {cfg:?}");
    (s, d)
}

#[test]
fn a_suite_that_runs_whole_at_the_batch_is_not_run_by_the_test_step_when_no_test_is_reached() {
    // #676: an empty pick ran the whole suite in the step and again in the landing batch.
    let runs = tempfile::tempdir().unwrap();
    let log = runs.path().join("ran");
    let (_s, d) = whole_later_world("", &log);
    let t = file_it(&d, "Whole later");
    wait_state(&d, t, State::Landed, 120);
    let ran = std::fs::read_to_string(&log).unwrap_or_default();
    assert_eq!(ran.trim(), "", "the step ran nothing narrowed: {ran:?}");
    let (_, note) = closing(&d, t);
    assert!(note.contains("checks: not selected"), "{note}");
    let notes = std::fs::read_to_string(d.task_dir(t).join("test-notes.txt")).unwrap_or_default();
    assert!(
        notes.contains(
            "checks: not run: nothing in this change reaches its tests; runs whole at batch"
        ),
        "{notes}"
    );
    // The batch tier lists the suite, so landing proves the union.
    let cfg = mason_daemon::landing::load_config(&d, ProjectId(1)).unwrap();
    assert_eq!(cfg.batch_suites(), Some(&["checks".to_string()][..]));
}

#[test]
fn a_suite_that_runs_whole_at_the_batch_runs_only_its_smoke_units_when_no_test_is_reached() {
    let runs = tempfile::tempdir().unwrap();
    let log = runs.path().join("ran");
    let (_s, d) = whole_later_world("narrow_smoke = [\"c_test\"]", &log);
    let t = file_it(&d, "Whole later, smoke");
    wait_state(&d, t, State::Landed, 120);
    let ran = std::fs::read_to_string(&log).unwrap_or_default();
    assert_eq!(ran.trim(), "narrowed c_test", "{ran:?}");
}

fn two_tasks_world() -> Scratch {
    world(
        vec![
            commit(HELLO, "add hello"),
            commit("echo b > b.txt", "add b"),
        ],
        &[],
        &def("", "", ""),
    )
}

#[test]
fn a_debt_closes_by_itself_when_the_suite_later_goes_green_on_a_base_with_the_task() {
    let s = two_tasks_world();
    let d = start(&s);
    let t = file_it(&d, "Skip the checks");
    mason_daemon::suites::skip(&d, t, "suite:checks", "later", "brian", false).unwrap();
    wait_state(&d, t, State::Landed, 120);
    // The debt exists. It may already be paid: since s426 landing runs every open debt of the
    // task once at the new base, on its own thread, so its status here is not fixed.
    let rows = d.store.debts(Some(t.0), false).unwrap();
    assert_eq!(rows.len(), 1);
    assert!(
        matches!(rows[0].status.as_str(), "open" | "paid"),
        "{}",
        rows[0].status
    );
    // The next task's own run of the suite is green at a base that holds the first task.
    let t2 = file_it(&d, "Second");
    wait_state(&d, t2, State::Landed, 120);
    wait_for("the debt to close by itself", 60, || {
        mason_daemon::suites::settle_debts(&d);
        d.store.debts(Some(t.0), false).unwrap()[0].status == "paid"
    });
    // Closed by a green at a base holding the task's commit: either the recorded green of a
    // later run ("checks green on origin/main at <sha> since") or the run landing makes of it
    // ("green when run on origin/main at <sha>").
    let how = d.store.debts(Some(t.0), false).unwrap()[0].how.clone();
    assert!(
        how.contains("checks green on") || how.contains("green when run on"),
        "{how}"
    );
}

fn record_skip_green_at_tip(d: &std::sync::Arc<mason_daemon::daemon::Daemon>, tests: &[&str]) {
    let pid = d.project_id_by_name("scratch").unwrap();
    let p = d.project(pid).unwrap().clone();
    let tip = mason_landing::git::run(&p.path, &["rev-parse", &p.base_ref()]).unwrap();
    let tests: Vec<String> = tests.iter().map(|t| t.to_string()).collect();
    d.store
        .put_suite_skip_green("scratch", "checks", tip.trim(), 5, &tests, 1)
        .unwrap();
}

#[test]
fn a_debt_is_paid_by_a_green_that_skipped_a_quarantined_test_and_the_test_is_named() {
    // s476: while a test is quarantined no whole-suite green is recorded, so nothing was paid.
    let s = two_tasks_world();
    let d = start(&s);
    let t = file_it(&d, "Skip the checks");
    mason_daemon::suites::skip(&d, t, "suite:checks", "later", "brian", false).unwrap();
    wait_state(&d, t, State::Landed, 120);
    // Landing runs the debt once on its own thread (s426); let that finish, then put the debt
    // back as it was when nothing could pay it.
    wait_for("the landing's own run of the debt", 60, || {
        d.store.debts(Some(t.0), false).unwrap()[0].status == "paid"
    });
    d.store
        .write(|c| {
            c.execute("DELETE FROM suite_green", [])?;
            c.execute(
                "UPDATE debts SET status = 'open', how = '', paid_at = NULL",
                [],
            )
            .map(|_| ())
        })
        .unwrap();
    record_skip_green_at_tip(&d, &["net::flaky_dns"]);
    assert_eq!(mason_daemon::suites::settle_debts(&d), 1);
    let how = d.store.debts(Some(t.0), false).unwrap()[0].how.clone();
    assert!(how.contains("net::flaky_dns"), "{how}");
}

#[test]
fn a_debt_whose_task_added_the_skipped_test_is_not_paid_by_a_green_that_skipped_it() {
    let s = world(
        vec![commit(
            "echo 'fn flaky_dns() {}' > added_test.rs",
            "add the test",
        )],
        &[],
        &def("", "", ""),
    );
    let d = start(&s);
    let t = file_it(&d, "Add a test");
    mason_daemon::suites::skip(&d, t, "suite:checks", "later", "brian", false).unwrap();
    wait_state(&d, t, State::Landed, 120);
    d.store
        .write(|c| c.execute("DELETE FROM suite_green", []).map(|_| ()))
        .unwrap();
    record_skip_green_at_tip(&d, &["net::flaky_dns"]);
    assert_eq!(mason_daemon::suites::settle_debts(&d), 0);
    // (An idle run may pay it by running the whole suite; it never pays it on the skip.)
    let row = d.store.debts(Some(t.0), false).unwrap().remove(0);
    assert!(
        row.status == "open" || !row.how.contains("flaky_dns"),
        "{}",
        row.how
    );
}

#[test]
fn a_debt_never_closes_on_a_green_at_a_base_without_the_tasks_commit() {
    let s = two_tasks_world();
    let d = start(&s);
    let t = file_it(&d, "Add hello");
    wait_state(&d, t, State::Landed, 120);
    // The task's own test step was green, at a base from before the task.
    let debt = mason_daemon::suites::skip(&d, t, "suite:checks", "later", "brian", false).unwrap();
    assert_eq!(mason_daemon::suites::settle_debts(&d), 0);
    let row = d.store.debts(Some(t.0), false).unwrap().remove(0);
    assert_eq!((row.id, row.status.as_str()), (debt, "open"));
}

#[test]
fn a_debt_whose_suite_is_still_red_stays_open_and_is_run_once_when_idle() {
    let s = two_tasks_world();
    let d = start(&s);
    // The base is red before the task lands: since s426 landing runs every open debt of the
    // task once at the new base, so a base that went red only afterwards would race that run.
    s.push_files(&[("legacy.broken", "x\n")], "break the base");
    let t = file_it(&d, "Skip the checks");
    mason_daemon::suites::skip(&d, t, "suite:checks", "later", "brian", false).unwrap();
    wait_state(&d, t, State::Landed, 120);
    assert_eq!(mason_daemon::suites::settle_debts(&d), 0);
    // The run at landing (or, failing that, an idle run) tries it once and it stays owed.
    wait_for("the debt to be tried", 60, || {
        mason_daemon::suites::run_idle_debts(&d);
        d.store.debts(Some(t.0), false).unwrap()[0].tried_at > 0
    });
    let row = d.store.debts(Some(t.0), false).unwrap().remove(0);
    assert_eq!(row.status, "open");
    assert!(row.tried_at > 0, "nothing ever tried it");
    // Once is enough: a second pass leaves it alone.
    mason_daemon::suites::run_idle_debts(&d);
    let again = d.store.debts(Some(t.0), false).unwrap().remove(0);
    assert_eq!(again.tried_at, row.tried_at);
}

#[test]
fn a_task_test_step_runs_only_the_crates_its_change_touches_and_their_dependents() {
    // s396: with `narrow_packages`, the test step runs `dev_command` for the changed crates
    // and their dependents, and records no evidence for the tree.
    let runs = tempfile::tempdir().unwrap();
    let log = runs.path().join("ran");
    let definition = def(
        &format!(
            "dev_command = \"echo dev {{packages}} >> {l}; sh checks.sh\"\n\
             narrow_packages = true",
            l = log.display()
        ),
        "",
        "",
    );
    let s = Scratch::new(
        "sh checks.sh",
        json!({"steps": {"develop": [commit("echo '// c' >> crates/leaf/src/lib.rs", "change leaf")]}}),
    );
    let krate = |name: &str, dep: &str| {
        format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n{dep}")
    };
    s.push_files(
        &[
            (".mason/mason.toml", &definition),
            ("checks.sh", CHECKS),
            (
                "Cargo.toml",
                "[workspace]\nresolver = \"2\"\nmembers = [\"crates/*\"]\n",
            ),
            ("crates/base/Cargo.toml", &krate("base", "")),
            ("crates/base/src/lib.rs", "\n"),
            (
                "crates/leaf/Cargo.toml",
                &krate("leaf", "[dependencies]\nbase = { path = \"../base\" }\n"),
            ),
            ("crates/leaf/src/lib.rs", "\n"),
        ],
        "project setup",
    );
    let d = start(&s);
    let t = file_it(&d, "Packages");
    wait_state(&d, t, State::Landed, 120);
    // The step may run the command more than once (a rerun, a run at the base to tell its
    // red from the task's own); what matters is which crates it named: only the leaf.
    let ran = std::fs::read_to_string(&log).unwrap_or_default();
    let crates: std::collections::BTreeSet<&str> = ran.lines().map(str::trim).collect();
    assert_eq!(
        crates.into_iter().collect::<Vec<_>>(),
        vec!["dev -p leaf"],
        "only the leaf crate ran: {ran:?}"
    );
    let (_, note) = closing(&d, t);
    assert!(note.contains("narrowed to 1 packages (leaf)"), "{note}");
}

#[test]
fn a_test_red_in_three_tasks_but_green_at_the_base_is_not_made_known_red() {
    // `mine` already failed on every run in two other tasks; this task's first attempt breaks
    // it too, which makes three. It passes at the merge base, so each task broke it with its
    // own change (s416): no ledger entry and no fix task.
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
        &[],
        &def("", "", ""),
    );
    let d = start(&s);
    let now = mason_daemon::daemon::now_ms();
    for other in [9001, 9002] {
        d.store
            .record_persistent_red("checks", "mine", other, now)
            .unwrap();
    }
    let t = file_it(&d, "Repeat");
    wait_state(&d, t, State::Landed, 90);
    assert_eq!(
        steps_run(&d, t),
        vec!["develop", "test", "develop", "test", "land"]
    );
    let ledger = d.store.ledger().unwrap();
    assert!(
        !ledger.iter().any(|e| e.test == "mine"),
        "green at the base, yet known red: {ledger:?}"
    );
    let fixes = d
        .world()
        .unwrap()
        .tasks
        .values()
        .filter(|t| t.title.starts_with("mine fails on "))
        .count();
    assert_eq!(fixes, 0);
}

#[test]
fn a_passed_build_is_remembered_per_tree_and_worktree_and_a_failed_one_is_not() {
    // A build that passed on an unchanged tree takes no `compile` slot again (s428): the test
    // step and run_check ask this memo before they queue for one.
    use mason_daemon::cmdreuse::*;
    let s = world(vec![], &[], &def("", "", ""));
    let d = start(&s);
    let wt = std::path::Path::new("/w/one");
    let other = std::path::Path::new("/w/two");
    let key = build_key("make all");
    assert!(!build_green_for(&d, "tree-a", &key, wt), "nothing yet");
    record_build_green(&d, "tree-a", &key, wt);
    assert!(build_green_for(&d, "tree-a", &key, wt));
    assert!(
        !build_green_for(&d, "tree-b", &key, wt),
        "a changed tree builds"
    );
    assert!(
        !build_green_for(&d, "tree-a", &key, other),
        "another worktree builds"
    );
    assert!(
        !build_green_for(&d, "tree-a", &build_key("make"), wt),
        "another command builds"
    );
    record_build_red(&d, "tree-a", &key, wt);
    assert!(
        !build_green_for(&d, "tree-a", &key, wt),
        "a failed build is not reused"
    );
}

/// `CHECKS` taking test names (a rerun): at the merge base (no `hello.txt`) it records the
/// names it was given in `rec`, and, when `kill` is set, dies by a signal there as a run
/// stopped by the load does.
fn checks_with_names(rec: &std::path::Path, kill: bool) -> String {
    let at_base = format!(
        "if [ $# -gt 0 ] && [ ! -f hello.txt ]; then echo \"$@\" >> {}; {} fi\n",
        rec.display(),
        if kill { "kill -TERM $$;" } else { "" }
    );
    CHECKS.replacen("[ -f skip.flag ]", &format!("{at_base}[ -f skip.flag ]"), 1)
}

fn rerun_def() -> String {
    def("rerun = \"sh checks.sh {tests}\"", "", "")
}

#[test]
fn a_develop_run_that_is_killed_does_not_count_the_round_against_the_task() {
    let rec = s_counter();
    let s = world(
        vec![commit(
            &format!("{HELLO} && echo x > mine.broken"),
            "add hello (and break mine)",
        )],
        &[],
        &rerun_def(),
    );
    s.push_files(
        &[("checks.sh", &checks_with_names(&rec, true))],
        "kill at base",
    );
    let d = start(&s);
    let t = file_it(&d, "Killed at develop");
    wait_state(&d, t, State::Landed, 90);
    // No trip back to development: the round was not the task's.
    assert_eq!(steps_run(&d, t), vec!["develop", "test", "land"]);
    let (v, note) = closing(&d, t);
    assert_eq!(v, "unverified", "{note}");
    assert!(note.contains("not counted against the task"), "{note}");
    // Tried twice, each time with only the failing test.
    let seen = std::fs::read_to_string(&rec).unwrap();
    let lines: Vec<&str> = seen.lines().collect();
    assert_eq!(lines, vec!["mine", "mine"], "{seen}");
}

#[test]
fn a_develop_run_of_only_the_failing_tests_that_fails_the_same_test_excuses_it() {
    let rec = s_counter();
    let s = world(
        vec![commit(HELLO, "add hello")],
        &[("legacy.broken", "known\n")],
        &rerun_def(),
    );
    s.push_files(
        &[("checks.sh", &checks_with_names(&rec, false))],
        "record at base",
    );
    let d = start(&s);
    let t = file_it(&d, "Same red at develop");
    wait_state(&d, t, State::Landed, 90);
    assert_eq!(steps_run(&d, t), vec!["develop", "test", "land"]);
    let (v, note) = closing(&d, t);
    assert_eq!(v, "unverified", "{note}");
    assert!(note.contains("failures excused"), "{note}");
    let seen = std::fs::read_to_string(&rec).unwrap();
    // Whatever the number of runs, develop was only ever given the failing test's name.
    assert!(
        !seen.is_empty() && seen.lines().all(|l| l == "legacy"),
        "{seen}"
    );
    let ledger = d.store.ledger().unwrap();
    assert!(ledger.iter().any(|e| e.test == "legacy"), "{ledger:?}");
}

// ---- #774: the idle debt pass runs each suite once per tip and settles from CI ----

/// A world whose suite appends a line to a log each time it runs, then behaves as CHECKS does.
fn counting_world(
    suite_extra: &str,
    log: &std::path::Path,
) -> (Scratch, Arc<mason_daemon::Daemon>) {
    let body = CHECKS.replacen(
        "#!/bin/sh\n",
        &format!("#!/bin/sh\necho run >> {}\n", log.display()),
        1,
    );
    let definition = def(suite_extra, "", "").replace("sh checks.sh", "sh counted.sh");
    let s = world(
        vec![commit(HELLO, "add hello")],
        &[("counted.sh", &body)],
        &definition,
    );
    // The hourly pass is off: the test runs the pass itself.
    let d = s.start_with(|c| {
        c.env_retry = Duration::from_millis(200);
        c.debt_interval = Duration::ZERO;
    });
    (s, d)
}

fn runs_of(log: &std::path::Path) -> usize {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .count()
}

fn base_tip(d: &Arc<mason_daemon::Daemon>) -> String {
    let pid = d.project_id_by_name("scratch").unwrap();
    let p = d.project(pid).unwrap().clone();
    mason_landing::git::run(&p.path, &["rev-parse", &p.base_ref()])
        .unwrap()
        .trim()
        .to_string()
}

/// The task landed; its debts are put back as owed with no green on record.
fn owe(d: &Arc<mason_daemon::Daemon>, t: TaskId, scopes: &[&str]) {
    for sc in scopes {
        let id = d
            .store
            .add_debt(t.0, "scratch", sc, "owner skipped", "brian", false, 0)
            .unwrap();
        // The task's branch is deleted some time after it lands, so the commit it landed as is
        // pinned now: the base tip holds it.
        d.store.set_debt_landed(id, &base_tip(d)).unwrap();
    }
    d.store
        .write(|c| {
            c.execute("DELETE FROM suite_green", [])?;
            c.execute("DELETE FROM suite_skip_green", [])?;
            // The landing recorded the tip's tree as tested; the debt is for a run that has not
            // been made, so that record would pay it without one.
            c.execute("DELETE FROM evidence", []).map(|_| ())
        })
        .unwrap();
}

fn open_debts(d: &Arc<mason_daemon::Daemon>, t: TaskId) -> usize {
    d.store
        .debts(Some(t.0), true)
        .unwrap()
        .iter()
        .filter(|x| x.status == "open")
        .count()
}

fn now_wall_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

#[test]
fn three_debts_of_different_scopes_on_one_tip_run_the_suite_once_and_one_green_settles_all() {
    let runs = tempfile::tempdir().unwrap();
    let log = runs.path().join("ran");
    let (_s, d) = counting_world("", &log);
    let t = file_it(&d, "Add hello");
    wait_state(&d, t, State::Landed, 120);
    owe(&d, t, &["suite:checks", "step:test", "tests:all"]);
    let before = runs_of(&log);
    mason_daemon::suites::run_idle_debts(&d);
    assert_eq!(
        runs_of(&log) - before,
        1,
        "the suite ran once for three debts"
    );
    assert_eq!(open_debts(&d, t), 0);
    mason_daemon::suites::run_idle_debts(&d);
    assert_eq!(runs_of(&log) - before, 1, "nothing more to run");
}

#[test]
fn after_a_red_a_new_tip_runs_the_suite_only_when_it_touched_a_red_module() {
    let runs = tempfile::tempdir().unwrap();
    let log = runs.path().join("ran");
    let (s, d) = counting_world("", &log);
    let t = file_it(&d, "Add hello");
    wait_state(&d, t, State::Landed, 120);
    owe(&d, t, &["suite:checks"]);
    let pid = d.project_id_by_name("scratch").unwrap();
    d.store
        .put_suite_red(
            "scratch",
            "checks",
            &base_tip(&d),
            &["probe::resolver::resolves".to_string()],
            now_wall_ms(),
        )
        .unwrap();
    let before = runs_of(&log);
    // The same tip: nothing runs.
    mason_daemon::suites::run_idle_debts(&d);
    assert_eq!(runs_of(&log), before);
    // A new tip that touched no red module: nothing runs.
    s.push_files(&[("docs/note.txt", "x\n")], "unrelated");
    d.fetch_remote(pid).unwrap();
    mason_daemon::suites::run_idle_debts(&d);
    assert_eq!(runs_of(&log), before);
    assert_eq!(open_debts(&d, t), 1);
    // A later one that touched the `resolver` module: the suite runs again, and is green.
    s.push_files(&[("src/resolver/mod.txt", "x\n")], "touch the module");
    d.fetch_remote(pid).unwrap();
    mason_daemon::suites::run_idle_debts(&d);
    assert_eq!(runs_of(&log) - before, 1);
    assert_eq!(open_debts(&d, t), 0);
    assert!(d.store.suite_red("scratch", "checks").unwrap().is_none());
}

struct FakeCi {
    sha: String,
    jobs: Vec<(&'static str, &'static str)>,
}

impl mason_daemon::ci_watch::Source for FakeCi {
    fn runs(&self, _: &str) -> Result<Vec<mason_adapters::github::ActionsRun>, String> {
        Ok(vec![mason_adapters::github::ActionsRun {
            id: 7,
            workflow: "ci".into(),
            sha: self.sha.clone(),
            url: String::new(),
            created_ms: 1,
            conclusion: Some("failure".into()),
            event: "push".into(),
        }])
    }
    fn jobs(&self, _: u64) -> Result<Vec<mason_adapters::github::ActionsJob>, String> {
        Ok(self
            .jobs
            .iter()
            .enumerate()
            .map(|(i, (n, c))| mason_adapters::github::ActionsJob {
                id: i as u64,
                name: n.to_string(),
                conclusion: Some(c.to_string()),
                status: "completed".into(),
            })
            .collect())
    }
    fn log_tail(&self, _: u64) -> Option<String> {
        None
    }
}

#[test]
fn a_ci_run_with_every_mapped_job_green_records_the_suite_and_settles_a_debt_one_red_job_does_not()
{
    let runs = tempfile::tempdir().unwrap();
    let log = runs.path().join("ran");
    let (_s, d) = counting_world("ci_jobs = [\"test (ubuntu-latest, *)\", \"lint\"]", &log);
    let pid = d.project_id_by_name("scratch").unwrap();
    let t = file_it(&d, "Add hello");
    wait_state(&d, t, State::Landed, 120);
    owe(&d, t, &["suite:checks"]);
    let before = runs_of(&log);
    // One mapped job red (the run's conclusion is failure either way): nothing is recorded.
    let red = FakeCi {
        sha: base_tip(&d),
        jobs: vec![
            ("test (ubuntu-latest, 1)", "success"),
            ("test (ubuntu-latest, 2)", "failure"),
            ("lint", "success"),
        ],
    };
    assert_eq!(mason_daemon::suites::record_ci_greens(&d, pid, &red), 0);
    assert_eq!(open_debts(&d, t), 1);
    // A mapped pattern with no job in the run is not covered either.
    let missing = FakeCi {
        sha: base_tip(&d),
        jobs: vec![("test (ubuntu-latest, 1)", "success")],
    };
    assert_eq!(mason_daemon::suites::record_ci_greens(&d, pid, &missing), 0);
    // Every mapped job green, an unmapped one red: the suite is green at the run's commit.
    let green = FakeCi {
        sha: base_tip(&d),
        jobs: vec![
            ("test (ubuntu-latest, 1)", "success"),
            ("test (ubuntu-latest, 2)", "success"),
            ("lint", "success"),
            ("swift", "failure"),
        ],
    };
    assert_eq!(mason_daemon::suites::record_ci_greens(&d, pid, &green), 1);
    assert_eq!(open_debts(&d, t), 0);
    let how = d.store.debts(Some(t.0), false).unwrap()[0].how.clone();
    assert!(how.contains("checks green on"), "{how}");
    assert_eq!(runs_of(&log), before, "the suite was not run");
}
