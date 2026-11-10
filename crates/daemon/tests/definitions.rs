// SPDX-License-Identifier: GPL-3.0-or-later

//! Stage 7 end to end: tasks run by the definition they are pinned to, skipping
//! exploration by each route, trust, "Explore anyway", approvals, live reload of
//! definitions (and refusal of bad ones), and the configuration layers.

mod support;
use mason_core::*;
use mason_daemon::defs::Reload;
use serde_json::json;
use std::time::Duration;
use support::*;

/// The built-in graph, written out so it is read from the base branch.
const DEF_SKIP: &str = r#"schema = 1
start = "explore"
explored_entry = "develop"

[step.explore]
type = "agent"
category = "exploring"
next = "develop"

[step.develop]
type = "agent"
category = "developing"
next = "test"

[step.test]
type = "test"
category = "testing"
max_rounds = 3
on_red = "develop"
next = "land"

[step.land]
type = "landing"
category = "landing"
"#;

/// A different shape: no exploring at all, and an extra command step.
const DEF_LINT: &str = r#"schema = 1
start = "develop"

[step.develop]
type = "agent"
category = "developing"
next = "lint"

[step.lint]
type = "command"
category = "testing"
command = "test -f hello.txt"
max_rounds = 2
on_red = "develop"
next = "test"

[step.test]
type = "test"
category = "testing"
max_rounds = 2
on_red = "develop"
next = "land"

[step.land]
type = "landing"
category = "landing"
"#;

const CLEAR_BODY: &str = "Add a greeting.\n\nFiles: `src/lib.rs`.\n\n## Approach\nCreate hello.txt (new) at the top of the repository and mention it from the library crate as the plan says.\n\n## Acceptance criteria\n- hello.txt exists\n- the tests pass\n";

fn scratch_with(def: Option<&str>, script: serde_json::Value) -> Scratch {
    let s = Scratch::new("test -f hello.txt", script);
    let mut files = vec![("src/lib.rs", "// library\n")];
    if let Some(d) = def {
        files.push((".mason/mason.toml", d));
    }
    s.push_files(&files, "project setup");
    s
}

fn flagged(d: &std::sync::Arc<mason_daemon::Daemon>, title: &str) -> TaskId {
    file_task_as(d, title, "Do the thing.", true, TrustSource::Owner, false)
}

#[test]
fn a_flagged_task_skips_exploring_and_a_vague_one_explores() {
    let s = scratch_with(Some(DEF_SKIP), happy_script());
    let d = s.start();
    let skipped = flagged(&d, "Already planned");
    let vague = file_task_as(
        &d,
        "Make it better",
        "Make the thing better somehow.",
        false,
        TrustSource::Owner,
        false,
    );
    wait_state(&d, skipped, State::Landed, 90);
    wait_state(&d, vague, State::Landed, 90);

    let (a, b) = (task_of(&d, skipped), task_of(&d, vague));
    assert_eq!(a.route, Some(Route::Flag));
    assert_eq!(
        steps_run(&d, skipped),
        vec!["develop", "test", "land"],
        "no exploring step ever ran"
    );
    assert_eq!(b.route, Some(Route::Explore));
    assert_eq!(
        steps_run(&d, vague),
        vec!["explore", "develop", "test", "land"]
    );
    assert_eq!(
        a.def, b.def,
        "both pinned to the definition on the base branch"
    );
    assert!(!a.def.is_empty());
    assert!(d.store.bugs().unwrap().is_empty());
}

#[test]
fn the_clarity_check_skips_exploring_for_a_task_that_is_already_a_plan() {
    let s = scratch_with(Some(DEF_SKIP), happy_script());
    let d = s.start();
    let clear = file_task_as(&d, "Greeting", CLEAR_BODY, false, TrustSource::Owner, false);
    // Names a file that is not there, and has no criteria: uncertain, so explored.
    let unclear = file_task_as(
        &d,
        "Vague",
        "Edit src/nope.rs to do something.",
        false,
        TrustSource::Owner,
        false,
    );
    wait_state(&d, clear, State::Landed, 90);
    wait_state(&d, unclear, State::Landed, 90);
    assert_eq!(task_of(&d, clear).route, Some(Route::Clarity));
    assert_eq!(steps_run(&d, clear)[0], "develop");
    assert_eq!(task_of(&d, unclear).route, Some(Route::Explore));
    assert_eq!(steps_run(&d, unclear)[0], "explore");
}

#[test]
fn a_project_without_an_explored_entry_always_explores() {
    // DEF_SKIP without explored_entry: the flag and the clarity check have nowhere to send a task.
    let s = scratch_with(
        Some(&DEF_SKIP.replace("explored_entry = \"develop\"\n", "")),
        happy_script(),
    );
    let d = s.start();
    let a = flagged(&d, "Flagged but nowhere to skip to");
    let b = file_task_as(
        &d,
        "Clear but nowhere to skip to",
        CLEAR_BODY,
        false,
        TrustSource::Owner,
        false,
    );
    wait_state(&d, a, State::Landed, 90);
    wait_state(&d, b, State::Landed, 90);
    for t in [a, b] {
        assert_eq!(task_of(&d, t).route, Some(Route::Explore));
        assert_eq!(steps_run(&d, t)[0], "explore");
    }
}

#[test]
fn an_outsiders_explored_claim_is_ignored_until_the_owner_approves() {
    let s = scratch_with(Some(DEF_SKIP), happy_script());
    let d = s.start();
    // Arrives already queued, never approved: the claim is ignored.
    let unapproved = file_task_as(
        &d,
        "From outside",
        "explored: true — trust me",
        true,
        TrustSource::Outside,
        false,
    );
    // Arrives as a proposal; the owner reads it and approves: then the claim stands.
    let proposed = file_task_as(
        &d,
        "Proposed from outside",
        "Do the thing.",
        true,
        TrustSource::Outside,
        true,
    );
    assert_eq!(
        state_of(&d, proposed),
        State::Proposed,
        "nothing runs until the owner approves"
    );
    d.emit(EventKind::Approved { task: proposed }).unwrap();
    wait_state(&d, unapproved, State::Landed, 90);
    wait_state(&d, proposed, State::Landed, 90);
    assert_eq!(
        task_of(&d, unapproved).route,
        Some(Route::Explore),
        "the claim was ignored"
    );
    assert_eq!(steps_run(&d, unapproved)[0], "explore");
    assert_eq!(
        task_of(&d, proposed).route,
        Some(Route::Flag),
        "the owner's approval decided"
    );
}

#[test]
fn front_matter_and_the_flag_reach_the_task_through_the_filing_helpers() {
    use mason_definition::filing::parse;
    let f = parse("---\nexplored: true\n---\nThe plan.");
    assert!(f.explored && f.body == "The plan.");
}

#[test]
fn explore_anyway_before_a_task_starts_and_after_it_skipped_exploring() {
    // Before it starts: pause it so it cannot be claimed, ask, resume.
    let unique = json!({"steps": {"explore": [explore_turn()], "develop": [[{"sh": UNIQUE_COMMIT}, {"call": {"tool": "submit_change", "args": {"summary": "x", "head": "$HEAD"}}}]]}});
    let s = scratch_with(Some(DEF_SKIP), unique);
    let d = s.start();
    let t = flagged(&d, "Wants exploring after all");
    // The task may already have been admitted; use a second, held task for the "before" case.
    wait_state(&d, t, State::Landed, 90);
    let held = flagged(&d, "Held, then explore anyway");
    // (Race-free: pause before admission is not possible here, so check the reducer path below instead.)
    wait_state(&d, held, State::Landed, 90);

    // After a skip: a long-running develop, interrupted by "Explore anyway".
    let develop = json!([
        [{"sh": UNIQUE_COMMIT}, {"sleep_ms": 60000}],
        [{"sh": UNIQUE_COMMIT}, {"call": {"tool": "submit_change", "args": {"summary": "done", "head": "$HEAD"}}}]
    ]);
    let s2 = scratch_with(
        Some(DEF_SKIP),
        json!({"steps": {"explore": [explore_turn()], "develop": develop}}),
    );
    let d2 = s2.start();
    let u = flagged(&d2, "Skips, then explores anyway");
    wait_state(&d2, u, State::Developing, 60);
    assert_eq!(task_of(&d2, u).route, Some(Route::Flag));
    d2.emit(EventKind::ExploreAnyway {
        task: u,
        step: "explore".into(),
    })
    .unwrap();
    let after = task_of(&d2, u);
    assert_eq!(
        (after.state, after.route, after.failures),
        (State::Exploring, Some(Route::ExploreAnyway), 0)
    );
    wait_for("the old agent to be stopped", 20, || {
        mason_step_host::registry::Registry::new(d2.cfg.hosts_dir())
            .scan()
            .unwrap()
            .iter()
            .filter(|h| h.info.id.starts_with(&format!("t{}r1-", u.0)))
            .all(|h| h.state != mason_step_host::registry::HostState::Running)
    });
    wait_state(&d2, u, State::Landed, 90);
    assert_eq!(steps_run(&d2, u)[..3], ["develop", "explore", "develop"]);
}

#[test]
fn explore_anyway_before_start_is_honoured_at_admission() {
    let s = scratch_with(Some(DEF_SKIP), happy_script());
    let d = s.start_with(|c| c.max_concurrent = 1);
    // Occupy the only slot so the next task waits in the queue, where the owner can still decide.
    let develop = json!([{"sh": UNIQUE_COMMIT}, {"sleep_ms": 1500}, {"call": {"tool": "submit_change", "args": {"summary": "x", "head": "$HEAD"}}}]);
    std::fs::write(
        &s.script,
        json!({"steps": {"explore": [explore_turn()], "develop": [develop]}}).to_string(),
    )
    .unwrap();
    let first = flagged(&d, "Occupies the slot");
    wait_state(&d, first, State::Developing, 60);
    let second = flagged(&d, "Waits, then explores anyway");
    assert_eq!(state_of(&d, second), State::Queued);
    d.emit(EventKind::ExploreAnyway {
        task: second,
        step: "explore".into(),
    })
    .unwrap();
    assert!(task_of(&d, second).force_explore);
    wait_state(&d, second, State::Landed, 120);
    assert_eq!(
        task_of(&d, second).route,
        Some(Route::Explore),
        "exploring was forced"
    );
    assert_eq!(steps_run(&d, second)[0], "explore");
}

#[test]
fn request_exploration_sends_a_skipped_task_back_and_charges_no_failure() {
    let develop = json!([
        [{"call": {"tool": "request_exploration", "args": {"reason": "the plan does not match the code"}}}],
        [{"sh": TASK_SCRIPT_COMMIT}, {"call": {"tool": "submit_change", "args": {"summary": "now it fits", "head": "$HEAD"}}}]
    ]);
    let s = scratch_with(
        Some(DEF_SKIP),
        json!({"steps": {"explore": [explore_turn()], "develop": develop}}),
    );
    let d = s.start();
    let t = flagged(&d, "Flag that was wrong");
    wait_state(&d, t, State::Landed, 90);
    let task = task_of(&d, t);
    assert_eq!(
        (task.failures, task.explore_returns, task.route),
        (0, 1, Some(Route::Flag))
    );
    assert_eq!(
        steps_run(&d, t),
        vec!["develop", "explore", "develop", "test", "land"]
    );
}

#[test]
fn two_different_definitions_run_through_the_same_daemon() {
    // Project one: the built-in-shaped graph. Project two: no exploring, an extra lint step.
    let s = scratch_with(Some(DEF_SKIP), happy_script());
    let d = s.start();
    let t = file_task_as(&d, "A", "Make it so.", false, TrustSource::Owner, false);
    wait_state(&d, t, State::Landed, 90);
    assert_eq!(steps_run(&d, t), vec!["explore", "develop", "test", "land"]);

    let s2 = scratch_with(
        Some(DEF_LINT),
        json!({"steps": {"develop": [develop_turn()]}}),
    );
    let d2 = s2.start();
    let u = file_task_as(&d2, "B", "Make it so.", false, TrustSource::Owner, false);
    wait_state(&d2, u, State::Landed, 90);
    assert_eq!(steps_run(&d2, u), vec!["develop", "lint", "test", "land"]);
    assert_ne!(task_of(&d, t).def, task_of(&d2, u).def);
    // The lint step really ran its own command, and its output is in the logs.
    assert!(mason_daemon::http::render_logs(&d2, u.0).contains("(test)"));
}

#[test]
fn an_invalid_definition_edit_is_refused_and_the_old_one_keeps_running() {
    let s = scratch_with(Some(DEF_SKIP), happy_script());
    // This is about the explicit reload, so the watcher is kept out of the way: it
    // fetches on its own schedule, and if it applied the new definition between the
    // push and the explicit reload, that reload rightly said `Unchanged`.
    let d = s.start_with(|c| {
        c.reload_interval = Duration::from_secs(3600);
        c.fetch_interval = Duration::from_secs(3600);
    });
    let pid = d.project_id_by_name("scratch").unwrap();
    let good_hash = d.current_def(pid).hash.clone();
    assert!(d.definition_error(pid).is_none());

    // Somebody lands a broken definition: a loop with no bound, and a typo.
    let broken = DEF_SKIP
        .replace("max_rounds = 3\n", "")
        .replace("next = \"land\"", "nxt = \"land\"");
    s.push_files(&[(".mason/mason.toml", &broken)], "break the pipeline");
    match d.reload_definition(pid) {
        Reload::Refused(errors) => {
            let all = errors.join("\n");
            assert!(
                all.contains("unknown key `nxt`") && all.contains("did you mean `next`"),
                "{all}"
            );
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
    assert_eq!(
        d.current_def(pid).hash,
        good_hash,
        "the current definition keeps running"
    );
    assert!(
        d.definition_error(pid).is_some(),
        "and the refusal is reported"
    );

    // A task filed now runs, and lands, under the old definition.
    let t = flagged(&d, "Still works");
    wait_state(&d, t, State::Landed, 90);
    assert_eq!(task_of(&d, t).def, good_hash);
    // After the `Landed` event the landing job cleans up and then reloads the
    // definition itself (the base just moved). Let it finish, or that reload can
    // apply the edit below first and the explicit one finds nothing to do.
    wait_for("the landing job to finish", 60, || d.job_count() == 0);

    // A valid edit replaces it and clears the report.
    s.push_files(&[(".mason/mason.toml", DEF_LINT)], "a good one");
    assert!(matches!(d.reload_definition(pid), Reload::Applied { .. }));
    assert!(
        d.definition_error(pid).is_none(),
        "an alert clears when its condition clears"
    );
    assert_ne!(d.current_def(pid).hash, good_hash);
}

#[test]
fn the_watcher_picks_up_a_new_definition_and_an_invalid_overlay_is_refused() {
    let s = scratch_with(Some(DEF_SKIP), happy_script());
    let d = s.start_with(|c| {
        c.reload_interval = Duration::from_millis(150);
        c.fetch_interval = Duration::from_millis(300);
    });
    let pid = d.project_id_by_name("scratch").unwrap();
    let before = d.current_def(pid).hash.clone();
    s.push_files(
        &[(".mason/mason.toml", DEF_LINT)],
        "switch to the lint graph",
    );
    // Nobody calls reload: the watcher fetches on its own and notices.
    wait_for("the watcher to apply it", 20, || {
        d.current_def(pid).hash != before
    });
    assert_eq!(d.current_def(pid).steps.len(), 4);

    // A broken overlay is refused, the last good overlay stays.
    let toml = std::fs::read_to_string(s.config_dir.join("projects/scratch.toml")).unwrap();
    std::fs::write(
        s.config_dir.join("projects/scratch.toml"),
        format!("{toml}\n[step.test]\nnext = \"develop\"\n"),
    )
    .unwrap();
    wait_for("the overlay refusal", 20, || {
        d.definition_error(pid)
            .is_some_and(|e| e.iter().any(|m| m.contains("cannot change the graph")))
    });
    std::fs::write(s.config_dir.join("projects/scratch.toml"), toml).unwrap();
    wait_for("the refusal to clear", 20, || {
        d.definition_error(pid).is_none()
    });
}

#[test]
fn a_task_stays_pinned_to_the_definition_it_started_under() {
    let develop = json!([{"sh": TASK_SCRIPT_COMMIT}, {"sleep_ms": 4000}, {"call": {"tool": "submit_change", "args": {"summary": "x", "head": "$HEAD"}}}]);
    let s = scratch_with(
        Some(DEF_LINT),
        json!({"steps": {"develop": [develop, develop_turn()]}}),
    );
    // The watcher's first pass fetches; keep it away so the explicit reload below is
    // the one that applies the change.
    let d = s.start_with(|c| {
        c.reload_interval = Duration::from_secs(3600);
        c.fetch_interval = Duration::from_secs(3600);
    });
    let pid = d.project_id_by_name("scratch").unwrap();
    let old = flagged(&d, "Started under the lint graph");
    wait_state(&d, old, State::Developing, 60);
    let old_hash = d.current_def(pid).hash.clone();

    // The definition changes while it works: the lint step is gone.
    let without_lint = DEF_LINT
        .replace("next = \"lint\"", "next = \"test\"")
        .replace("[step.lint]\ntype = \"command\"\ncategory = \"testing\"\ncommand = \"test -f hello.txt\"\nmax_rounds = 2\non_red = \"develop\"\nnext = \"test\"\n\n", "");
    s.push_files(&[(".mason/mason.toml", &without_lint)], "drop lint");
    assert!(matches!(d.reload_definition(pid), Reload::Applied { .. }));
    let new_hash = d.current_def(pid).hash.clone();
    assert_ne!(old_hash, new_hash);

    let new = flagged(&d, "Started under the new graph");
    wait_state(&d, old, State::Landed, 120);
    wait_state(&d, new, State::Landed, 120);
    assert_eq!(task_of(&d, old).def, old_hash);
    assert_eq!(task_of(&d, new).def, new_hash);
    assert_eq!(
        steps_run(&d, old),
        vec!["develop", "lint", "test", "land"],
        "finished on the old version"
    );
    assert_eq!(steps_run(&d, new), vec!["develop", "test", "land"]);
}

#[test]
fn a_change_that_would_break_the_definition_is_not_accepted() {
    // The agent edits .mason/mason.toml badly, submits, is told why, fixes it, and lands.
    let bad_edit = "printf 'schema = 1\\nstart = \"nowhere\"\\n' > .mason/mason.toml";
    let good_edit = format!(
        "printf '%s' '{}' > .mason/mason.toml",
        DEF_SKIP.replace('\'', "")
    );
    let develop = json!([
        {"sh": "mkdir -p .mason && echo hi > hello.txt"},
        {"sh": bad_edit},
        {"sh": "git add -A && git -c user.name=t -c user.email=t@t commit -qm 'edit the pipeline'"},
        {"call": {"tool": "submit_change", "args": {"summary": "I changed the pipeline", "head": "$HEAD"}}},
        {"sh": good_edit},
        {"sh": "git add -A && git -c user.name=t -c user.email=t@t commit -qm 'fix the pipeline edit'"},
        {"call": {"tool": "submit_change", "args": {"summary": "fixed", "head": "$HEAD"}}}
    ]);
    let s = scratch_with(
        Some(DEF_SKIP),
        json!({"steps": {"explore": [explore_turn()], "develop": [develop]}}),
    );
    let d = s.start();
    let pid = d.project_id_by_name("scratch").unwrap();
    let before = d.current_def(pid).hash.clone();
    let t = flagged(&d, "Edits the pipeline");
    wait_state(&d, t, State::Landed, 90);
    let logs = mason_daemon::http::render_logs(&d, t.0);
    assert!(
        logs.contains("edits .mason/mason.toml, and the new definition is invalid"),
        "{logs}"
    );
    // The edit that landed is the good one, and it takes effect only after landing.
    assert_eq!(
        task_of(&d, t).def,
        before,
        "judged by the definition it started under"
    );
    assert!(sh(&s.origin, &["show", "main:.mason/mason.toml"]).contains("explored_entry"));
}

const DEF_APPROVAL: &str = r#"schema = 1
start = "develop"

[step.develop]
type = "agent"
category = "developing"
next = "test"

[step.test]
type = "test"
category = "testing"
max_rounds = 2
on_red = "develop"
next = "sign-off"

[step.sign-off]
type = "approval"
category = "review"
max_rounds = 2
on_red = "develop"
next = "land"

[step.land]
type = "landing"
category = "landing"
"#;

#[test]
fn an_approval_step_waits_for_the_owner_holds_no_slot_and_can_reject() {
    let script = json!({"steps": {"develop": [develop_turn(), [{"sh": "echo again > again.txt && git add -A && git -c user.name=t -c user.email=t@t commit -qm again"}, {"call": {"tool": "submit_change", "args": {"summary": "second try", "head": "$HEAD"}}}]]}});
    let s = scratch_with(Some(DEF_APPROVAL), script);
    let d = s.start_with(|c| c.max_concurrent = 1);
    let a = flagged(&d, "Needs sign-off");
    wait_for("task a to wait for approval", 60, || {
        task_of(&d, a).activity == Activity::Waiting(WaitReason::On(APPROVAL_WAIT.into()))
    });
    assert_eq!(state_of(&d, a), State::Review);

    // It holds no running slot: a second task is admitted although max_concurrent is 1.
    let b = flagged(&d, "Second, same sign-off");
    wait_for("task b to start while a waits", 60, || {
        task_of(&d, b).state.is_step()
    });
    wait_for("task b to wait too", 60, || {
        task_of(&d, b).activity == Activity::Waiting(WaitReason::On(APPROVAL_WAIT.into()))
    });
    assert!(
        !s.origin_log().contains(&"add hello".to_string()),
        "nothing lands without approval"
    );

    // Approve a: it lands. Reject b: back to development, as the step says, then waits again.
    mason_daemon::jobs::conclude_approval(&d, a, mason_definition::graph::Outcome::Green, "")
        .unwrap();
    wait_state(&d, a, State::Landed, 90);
    mason_daemon::jobs::conclude_approval(
        &d,
        b,
        mason_definition::graph::Outcome::Red,
        "needs another look",
    )
    .unwrap();
    wait_for(
        "b to go back to development and return to sign-off",
        90,
        || {
            let t = task_of(&d, b);
            t.step_attempts.get("develop").copied() == Some(2)
                && t.activity == Activity::Waiting(WaitReason::On(APPROVAL_WAIT.into()))
        },
    );
    // Approving something that is not waiting for approval is refused.
    assert!(mason_daemon::jobs::conclude_approval(
        &d,
        a,
        mason_definition::graph::Outcome::Green,
        ""
    )
    .is_err());
    mason_daemon::jobs::conclude_approval(&d, b, mason_definition::graph::Outcome::Green, "")
        .unwrap();
    wait_state(&d, b, State::Landed, 90);
}

#[test]
fn rejecting_an_approval_with_no_way_back_fails_the_task_with_the_reason() {
    let def = DEF_APPROVAL.replace(
        "max_rounds = 2\non_red = \"develop\"\nnext = \"land\"",
        "next = \"land\"",
    );
    let s = scratch_with(Some(&def), happy_script());
    let d = s.start();
    let t = flagged(&d, "Rejected outright");
    wait_for("the approval", 60, || {
        task_of(&d, t).activity == Activity::Waiting(WaitReason::On(APPROVAL_WAIT.into()))
    });
    mason_daemon::jobs::conclude_approval(
        &d,
        t,
        mason_definition::graph::Outcome::Red,
        "not what I wanted",
    )
    .unwrap();
    wait_state(&d, t, State::Failed, 30);
    assert!(task_of(&d, t)
        .failure
        .unwrap()
        .contains("not what I wanted"));
}

#[test]
fn steps_that_cannot_run_yet_are_refused_when_a_definition_is_applied() {
    let gate = DEF_SKIP.replace("type = \"test\"", "type = \"gate\"");
    let s = scratch_with(Some(DEF_SKIP), happy_script());
    let d = s.start();
    let pid = d.project_id_by_name("scratch").unwrap();
    let before = d.current_def(pid).hash.clone();
    s.push_files(&[(".mason/mason.toml", &gate)], "use a gate");
    match d.reload_definition(pid) {
        Reload::Refused(e) => assert!(
            e.join(" ").contains("cannot run yet") && e.join(" ").contains("Stage 9"),
            "{e:?}"
        ),
        other => panic!("{other:?}"),
    }
    assert_eq!(d.current_def(pid).hash, before);
}

#[test]
fn a_task_whose_definition_vanished_from_view_still_finds_its_pin_in_the_store() {
    // The pinned text lives in the store, not only on the branch.
    let s = scratch_with(Some(DEF_SKIP), happy_script());
    let d = s.start();
    let pid = d.project_id_by_name("scratch").unwrap();
    let h = d.current_def(pid).hash.clone();
    assert!(d.store.definition(&h).unwrap().is_some());
    assert_eq!(d.pinned_def(&h).unwrap().hash, h);
    assert!(d.pinned_def("nonexistent").is_none());
    assert_eq!(
        d.pinned_def("").unwrap().start,
        "explore",
        "tasks from before definitions ran the built-in graph"
    );
}

#[test]
fn runtime_and_per_task_overrides_change_a_running_pipeline_and_runtime_ones_expire() {
    // Tests that never pass: the layers decide how many rounds it gets.
    let script = json!({"steps": {"explore": [explore_turn()], "develop": [develop_turn(), develop_turn(), develop_turn(), develop_turn(), develop_turn()]}});
    let s = scratch_with(Some(DEF_SKIP), script);
    s.write_project("scratch", "exit 1", "");
    let d = s.start();
    let now = mason_daemon::daemon::now_ms();

    // A runtime override: one round only, for the next two seconds.
    d.store
        .add_override(
            "scratch",
            "step.test.max_rounds",
            "1",
            "brian",
            now,
            now + 60_000,
        )
        .unwrap();
    let a = flagged(&d, "One round");
    wait_state(&d, a, State::Failed, 90);
    assert_eq!(
        task_of(&d, a).step_attempts["test"],
        1,
        "the runtime override gave it one round"
    );
    assert!(task_of(&d, a)
        .failure
        .unwrap()
        .contains("still red after 1 round"));

    // Once it expires (or is removed) the layer below shows through: three rounds.
    d.store
        .remove_override("scratch", "step.test.max_rounds")
        .unwrap();
    let b = flagged(&d, "Back to the default");
    wait_state(&d, b, State::Failed, 120);
    assert_eq!(task_of(&d, b).step_attempts["test"], 3);

    // A per-task override beats the default, for that task only.
    let c = flagged(&d, "Two rounds for this one");
    d.store
        .set_task_override(c.0, "step.test.max_rounds", "2", "brian", now)
        .unwrap();
    let c2 = flagged(&d, "Default for this one");
    wait_state(&d, c, State::Failed, 120);
    wait_state(&d, c2, State::Failed, 120);
    assert_eq!(task_of(&d, c).step_attempts["test"], 2);
    assert_eq!(task_of(&d, c2).step_attempts["test"], 3);
}

#[test]
fn a_lone_flagged_task_is_the_recorded_skip_run() {
    let s = scratch_with(Some(DEF_SKIP), happy_script());
    let d = s.start();
    let t = flagged(&d, "Already planned");
    wait_state(&d, t, State::Landed, 90);
    wait_for("cleanup", 15, || d.job_count() == 0);
    maybe_record(&d, "skipped_exploring");
    assert_eq!(steps_run(&d, t), vec!["develop", "test", "land"]);
}

#[test]
fn an_optional_classifier_is_consulted_only_when_the_rules_are_unsure() {
    // The classifier reads the task body on stdin and answers; here, by the word "obvious".
    let classifier = "clarity_classifier = \"grep -qi obvious && echo clear || echo unclear\"";
    let s = scratch_with(Some(DEF_SKIP), unique_script());
    s.write_project("scratch", "test -f hello.txt", classifier);
    let d = s.start();
    let by_classifier = file_task_as(
        &d,
        "A",
        "This one is obvious, trust me.",
        false,
        TrustSource::Owner,
        false,
    );
    let unsure = file_task_as(
        &d,
        "B",
        "Make the thing better somehow.",
        false,
        TrustSource::Owner,
        false,
    );
    let by_rules = file_task_as(
        &d,
        "C",
        &format!("{CLEAR_BODY}\nNot so obvious."),
        false,
        TrustSource::Owner,
        false,
    );
    let by_flag = flagged(&d, "D");
    for t in [by_classifier, unsure, by_rules, by_flag] {
        wait_state(&d, t, State::Landed, 120);
    }
    assert_eq!(task_of(&d, by_classifier).route, Some(Route::Classifier));
    assert_eq!(task_of(&d, unsure).route, Some(Route::Explore));
    assert_eq!(
        task_of(&d, by_rules).route,
        Some(Route::Clarity),
        "the deterministic check decides before any classifier"
    );
    assert_eq!(task_of(&d, by_flag).route, Some(Route::Flag));
    // A classifier that fails, hangs on nothing, or says anything but "clear" means explore.
    assert!(mason_daemon::jobs::classify("echo clear", "x"));
    assert!(mason_daemon::jobs::classify(
        "cat >/dev/null; echo CLEAR",
        "x"
    ));
    for cmd in [
        "echo unclear",
        "echo clearly",
        "exit 1",
        "echo clear; exit 1",
        "no-such-command-anywhere",
        "",
    ] {
        assert!(!mason_daemon::jobs::classify(cmd, "x"), "{cmd:?}");
    }
}

#[test]
fn two_tasks_one_strictly_after_the_other_both_land() {
    // The case that failed on CI by timing: with one slot the second task starts only
    // after the first has landed, so the shared scripted commit must not be empty.
    let s = scratch_with(Some(DEF_SKIP), happy_script());
    let d = s.start_with(|c| c.max_concurrent = 1);
    let a = flagged(&d, "First");
    let b = flagged(&d, "Second");
    wait_state(&d, a, State::Landed, 90);
    wait_state(&d, b, State::Landed, 90);
    let log = s.origin_log();
    assert_eq!(
        log.iter().filter(|m| *m == "add hello").count(),
        2,
        "{log:?}"
    );
}
