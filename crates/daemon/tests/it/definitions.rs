// SPDX-License-Identifier: GPL-3.0-or-later

//! Stage 7 end to end: tasks run by the definition they are pinned to, skipping
//! exploration by each route, trust, "Explore anyway", approvals, live reload of
//! definitions (and refusal of bad ones), and the configuration layers.

use crate::support;
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
    let gate = DEF_SKIP.replace("type = \"test\"", "type = \"ci\"");
    let s = scratch_with(Some(DEF_SKIP), happy_script());
    let d = s.start();
    let pid = d.project_id_by_name("scratch").unwrap();
    let before = d.current_def(pid).hash.clone();
    s.push_files(&[(".mason/mason.toml", &gate)], "use a ci step");
    match d.reload_definition(pid) {
        Reload::Refused(e) => assert!(
            e.join(" ").contains("cannot run yet") && e.join(" ").contains("Stage 15"),
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
fn a_tasks_run_on_choice_is_read_live_and_beats_the_global_switch() {
    use mason_daemon::place::{effective_line, set_global, task_choice, Mode};
    let s = scratch_with(Some(DEF_SKIP), happy_script());
    s.write_project(
        "scratch",
        "test -f hello.txt",
        "[remote_runs]\nkind = \"github-actions\"\nrepo = \"o/r\"\ntoken = \"env:MASON_T831_TOKEN\"\n",
    );
    let d = s.start();
    let t = flagged(&d, "Chooses");
    let other = flagged(&d, "Follows");
    let now = mason_daemon::daemon::now_ms();
    assert_eq!(task_choice(&d, Some(t.0)), None);
    assert_eq!(task_choice(&d, None), None);

    d.store
        .set_task_override(t.0, "run_on", "github", "brian", now)
        .unwrap();
    assert_eq!(task_choice(&d, Some(t.0)), Some(Mode::Remote));
    assert_eq!(task_choice(&d, Some(other.0)), None, "only for that task");
    assert_eq!(
        effective_line(&d, t.0)["line"],
        "runs on: GitHub (task choice)"
    );
    assert_eq!(
        effective_line(&d, other.0)["line"],
        "runs on: this Mac (global switch)"
    );

    // Both directions against the global switch.
    set_global(&d, Mode::Remote, "brian").unwrap();
    d.store
        .set_task_override(t.0, "run_on", "local", "brian", now)
        .unwrap();
    assert_eq!(task_choice(&d, Some(t.0)), Some(Mode::Local));
    assert_eq!(
        effective_line(&d, t.0)["line"],
        "runs on: this Mac (task choice)"
    );
    assert_eq!(
        effective_line(&d, other.0)["line"],
        "runs on: GitHub (global switch)"
    );

    // `default` is the override removed.
    d.store.clear_task_override(t.0, "run_on").unwrap();
    assert_eq!(task_choice(&d, Some(t.0)), None);
    assert_eq!(
        effective_line(&d, t.0)["line"],
        "runs on: GitHub (global switch)"
    );
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

fn alert_keys(d: &std::sync::Arc<mason_daemon::Daemon>) -> Vec<String> {
    mason_daemon::attention::sync_alerts(d);
    d.store
        .alerts(true)
        .unwrap()
        .into_iter()
        .filter(|a| a.kind == "needs_you")
        .map(|a| a.key)
        .collect()
}

fn spoken(d: &std::sync::Arc<mason_daemon::Daemon>, key: &str) -> String {
    mason_daemon::notify::tick(d);
    d.store
        .pending_announcements(200)
        .unwrap()
        .into_iter()
        .find(|p| p.key.starts_with(&format!("alert:{key}:")))
        .unwrap_or_else(|| panic!("nothing was to be said for {key}"))
        .sentence
}

#[test]
fn an_approval_step_raises_an_alert_that_clears_when_approved() {
    let s = scratch_with(Some(DEF_APPROVAL), happy_script());
    let d = s.start();
    let t = flagged(&d, "Needs sign-off");
    wait_for("the approval wait", 60, || {
        task_of(&d, t).activity == Activity::Waiting(WaitReason::On(APPROVAL_WAIT.into()))
    });
    let key = format!("approval:{}", t.0);
    assert!(alert_keys(&d).contains(&key));
    let line = spoken(&d, &key);
    assert!(line.contains("waiting for your approval"), "{line}");
    assert!(!line.contains("decision"), "{line}");
    mason_daemon::jobs::conclude_approval(&d, t, mason_definition::graph::Outcome::Green, "")
        .unwrap();
    wait_state(&d, t, State::Landed, 90);
    assert!(!alert_keys(&d).contains(&key));
}

#[test]
fn a_proposed_task_raises_an_alert_that_clears_on_approve_and_on_reject() {
    let s = scratch_with(Some(DEF_SKIP), happy_script());
    let d = s.start();
    let file =
        |title: &str| file_task_as(&d, title, "Do the thing.", true, TrustSource::Owner, true);
    let (a, b) = (file("Proposed A"), file("Proposed B"));
    let keys = alert_keys(&d);
    for t in [a, b] {
        assert!(keys.contains(&format!("proposed:{}", t.0)), "{keys:?}");
    }
    let line = spoken(&d, &format!("proposed:{}", a.0));
    assert!(
        line.contains("proposed and waiting for your approval"),
        "{line}"
    );
    d.emit(EventKind::Approved { task: a }).unwrap();
    d.emit(EventKind::Rejected { task: b }).unwrap();
    let keys = alert_keys(&d);
    assert!(!keys.contains(&format!("proposed:{}", a.0)), "{keys:?}");
    assert!(!keys.contains(&format!("proposed:{}", b.0)), "{keys:?}");
}

/// `place` keeps its standing placements and its notes in process-wide maps keyed by project
/// name and task id, and every scratch daemon in this binary has project `scratch` and task 1.
/// `favor_local_…` ends with a `Local` placement that stands for ten minutes, and
/// `favor_remote_…` used to read it back as its first answer, but only when the two shared a
/// process in that order: red in whole runs, green alone. The tests that place checks hold this
/// lock and clear their key first.
static PLACEMENT: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn placement_alone(t: TaskId) -> std::sync::MutexGuard<'static, ()> {
    let g = PLACEMENT.lock().unwrap_or_else(|e| e.into_inner());
    mason_daemon::place::forget("scratch", Some(t.0), "unit", "check");
    g
}

#[test]
fn a_develop_checks_suite_is_placed_by_the_switch_and_the_tasks_choice() {
    use mason_daemon::place::{set_global, Mode};
    use mason_daemon::remote::{executor_for, Executor, Kind};
    let s = scratch_with(Some(DEF_SKIP), happy_script());
    s.write_project(
        "scratch",
        "test -f hello.txt",
        "[remote_runs]\nkind = \"github-actions\"\nrepo = \"o/r\"\ntoken = \"env:MASON_T755_TOKEN\"\n",
    );
    let d = s.start();
    let t = flagged(&d, "Checks");
    let _alone = placement_alone(t);
    let suite = mason_testing::Suite::new("unit", "true", mason_testing::ParserKind::Tap);
    let now = mason_daemon::daemon::now_ms();
    let at = |d: &std::sync::Arc<mason_daemon::Daemon>| {
        executor_for(d, "scratch", Kind::Check, Some((Some(t), &suite)))
    };
    // The switch is off: here.
    assert_eq!(at(&d), Executor::Local);
    // On: GitHub.
    set_global(&d, Mode::Remote, "brian").unwrap();
    assert!(matches!(at(&d), Executor::GitHub { .. }));
    // The task's choice of here beats the switch.
    d.store
        .set_task_override(t.0, "run_on", "local", "brian", now)
        .unwrap();
    assert_eq!(at(&d), Executor::Local);
}

#[test]
fn the_project_mode_sits_between_the_task_and_the_global_mode() {
    use mason_daemon::place::{project_effective, set_global, set_project_mode, status, Mode};
    let s = scratch_with(Some(DEF_SKIP), happy_script());
    // The old `fallback = "local"` reads as favor-remote when no `mode` is given.
    s.write_project(
        "scratch",
        "test -f hello.txt",
        "[remote_runs]\nkind = \"github-actions\"\nrepo = \"o/r\"\ntoken = \"env:MASON_T831_TOKEN\"\nfallback = \"local\"\n",
    );
    let d = s.start();
    let t = flagged(&d, "Follows its project");
    assert_eq!(
        project_effective(&d, "scratch"),
        (Mode::FavorRemote, "project")
    );
    set_global(&d, Mode::Remote, "brian").unwrap();
    assert_eq!(
        project_effective(&d, "scratch"),
        (Mode::FavorRemote, "project")
    );

    // The setter writes the registration file and the daemon re-reads it.
    set_project_mode(&d, "scratch", Mode::FavorLocal).unwrap();
    assert_eq!(
        project_effective(&d, "scratch"),
        (Mode::FavorLocal, "project")
    );
    let st = status(&d);
    let p = &st["projects"][0];
    assert_eq!(
        (p["mode"].as_str(), p["source"].as_str()),
        (Some("favor-local"), Some("project"))
    );
    assert_eq!(
        mason_daemon::place::effective_line(&d, t.0)["from"],
        "project"
    );

    // A task's own mode beats the project's, for that task only.
    let other = flagged(&d, "Other");
    d.store
        .set_task_override(
            t.0,
            "run_on",
            "remote",
            "brian",
            mason_daemon::daemon::now_ms(),
        )
        .unwrap();
    let line = |id: u64| mason_daemon::place::effective_line(&d, id);
    assert_eq!(line(t.0)["mode"], "remote");
    assert_eq!(line(t.0)["from"], "task");
    assert_eq!(line(other.0)["mode"], "favor-local");
}

/// #832: `favor-local` runs here while this machine has room, and starts on GitHub when its
/// `compile` slot is held and requests are already waiting for it. A run that is over is
/// forgotten, so the next one is decided afresh. No duration is asserted.
#[test]
fn favor_local_spills_to_github_only_while_this_machine_is_full() {
    use mason_daemon::remote::{executor_for, Executor, Kind};
    use mason_daemon::sched::LeaseState;
    let s = scratch_with(Some(DEF_SKIP), happy_script());
    s.write_project(
        "scratch",
        "test -f hello.txt",
        "[remote_runs]\nkind = \"github-actions\"\nrepo = \"o/r\"\ntoken = \"env:MASON_T832_TOKEN\"\nmode = \"favor-local\"\nspill_after = 1\n",
    );
    let d = s.start();
    let t = flagged(&d, "Favors this Mac");
    let _alone = placement_alone(t);
    let suite = mason_testing::Suite::new("unit", "true", mason_testing::ParserKind::Tap);
    let at = |d: &std::sync::Arc<mason_daemon::Daemon>| {
        executor_for(d, "scratch", Kind::Check, Some((Some(t), &suite)))
    };
    let compile = std::collections::BTreeMap::from([("compile".to_string(), 1u32)]);
    d.broker.set_limit("compile", 1).unwrap();

    // The slot is free: here, and it says why.
    assert_eq!(at(&d), Executor::Local);
    let why = || {
        mason_daemon::place::seen(Some(t.0))
            .last()
            .map(|v| v["why"].as_str().unwrap_or("").to_string())
            .unwrap_or_default()
    };
    assert!(
        why().contains("favor-local, 1 compile slot free"),
        "{}",
        why()
    );
    mason_daemon::place::forget("scratch", Some(t.0), "unit", "check");

    // The slot is held and another request waits for it: GitHub.
    let held = d.broker.lease_open_as("hold", &compile, None);
    crate::support::wait_for("the hold to be granted", 60, || {
        matches!(d.broker.lease_poll(held), LeaseState::Granted { .. })
    });
    let waiter = d.broker.lease_open_as("waiter", &compile, None);
    assert!(!matches!(
        d.broker.lease_poll(waiter),
        LeaseState::Granted { .. }
    ));
    assert!(matches!(at(&d), Executor::GitHub { .. }));
    assert!(
        why().contains("favor-local, no compile slot free and 1 waiting"),
        "{}",
        why()
    );

    // Released, and the run over: the next one runs here.
    d.broker.lease_close(waiter);
    d.broker.lease_close(held);
    mason_daemon::place::forget("scratch", Some(t.0), "unit", "check");
    assert_eq!(at(&d), Executor::Local);
}

/// #833: `favor-remote` goes to GitHub while it has room, and runs here (no run ref is pushed:
/// only the placement is asked) when the `gha` pool is full with a waiter and this machine has a
/// free slot. With this machine full too, it waits on GitHub. No duration is asserted.
#[test]
fn favor_remote_runs_here_only_while_github_is_full_and_this_machine_has_room() {
    use mason_daemon::remote::{executor_for, Executor, Kind};
    use mason_daemon::sched::LeaseState;
    let s = scratch_with(Some(DEF_SKIP), happy_script());
    s.write_project(
        "scratch",
        "test -f hello.txt",
        "[remote_runs]\nkind = \"github-actions\"\nrepo = \"o/r\"\ntoken = \"env:MASON_T833_TOKEN\"\nmode = \"favor-remote\"\nspill_after = 1\n",
    );
    let d = s.start();
    let t = flagged(&d, "Favors GitHub");
    let _alone = placement_alone(t);
    let suite = mason_testing::Suite::new("unit", "true", mason_testing::ParserKind::Tap);
    let at = |d: &std::sync::Arc<mason_daemon::Daemon>| {
        executor_for(d, "scratch", Kind::Check, Some((Some(t), &suite)))
    };
    let forget = || mason_daemon::place::forget("scratch", Some(t.0), "unit", "check");
    let one = |n: &str| std::collections::BTreeMap::from([(n.to_string(), 1u32)]);
    let why = || {
        mason_daemon::place::seen(Some(t.0))
            .last()
            .map(|v| v["why"].as_str().unwrap_or("").to_string())
            .unwrap_or_default()
    };
    d.broker.set_limit("gha", 1).unwrap();
    d.broker.set_limit("compile", 1).unwrap();

    // GitHub has room: there.
    assert!(matches!(at(&d), Executor::GitHub { .. }));
    assert!(why().contains("favor-remote, GitHub had room"), "{}", why());
    forget();

    // The pool is used and another run waits for it; this machine's slot is free: here.
    let held = d.broker.lease_open_as("gha-hold", &one("gha"), None);
    crate::support::wait_for("the pool hold to be granted", 60, || {
        matches!(d.broker.lease_poll(held), LeaseState::Granted { .. })
    });
    let waiter = d.broker.lease_open_as("gha-waiter", &one("gha"), None);
    assert!(!matches!(
        d.broker.lease_poll(waiter),
        LeaseState::Granted { .. }
    ));
    assert_eq!(at(&d), Executor::Local);
    assert!(
        why().contains(
            "favor-remote, the runner pool is full with 1 waiting; this machine had room"
        ),
        "{}",
        why()
    );
    forget();

    // This machine's slot is taken too: it waits on GitHub.
    let busy = d.broker.lease_open_as("busy", &one("compile"), None);
    crate::support::wait_for("the compile hold to be granted", 60, || {
        matches!(d.broker.lease_poll(busy), LeaseState::Granted { .. })
    });
    assert!(matches!(at(&d), Executor::GitHub { .. }));
    assert!(why().contains("so it waits on GitHub"), "{}", why());
    for l in [busy, waiter, held] {
        d.broker.lease_close(l);
    }
    forget();
}
