// SPDX-License-Identifier: GPL-3.0-or-later

//! The nightly pipeline review (s1093): its settings, when it is due, what is filed, the
//! duplicate search over every task, and what becomes of its findings. The clock is passed in;
//! claims are paused, so the tests play the agent.

use crate::support;
use mason_core::introspect::CATEGORIES;
use mason_core::*;
use mason_daemon::daemon::now_ms;
use mason_daemon::{attention, holds, introspect, proposals, review, Daemon};
use mason_definition::layers::resolve;
use mason_definition::tiers::{launch, Mapping};
use mason_store::{ProposalRow, Store};
use serde_json::{json, Value};
use std::sync::Arc;
use support::lw::*;
use support::*;

const LANDING: &str = "mode = \"direct\"\nwindow = \"0s\"\n";
/// `scratch` is MASON's own; its table would queue a low, small finding.
const OWN: &str = "[self]\nmax_open = 50\nper_week = 50\nmax_steps = 500\n\
                   [proposals.introspection]\nlow_small = \"queued\"\n";
const HOUR: u64 = 3_600_000;
const DAY: u64 = 24 * HOUR;
const EVIDENCE: &str = "crates/daemon/src/sched.rs:120 ranks review below develop";

/// MASON's own project, a second one the review covers and a third it does not.
fn world() -> W {
    let w = world_with(
        LANDING,
        &|s| {
            for (name, extra) in [("web", ""), ("quiet", "[introspect]\nanalyst = false\n")] {
                let repo = s.root.path().join(format!("{name}-code"));
                std::fs::create_dir_all(&repo).unwrap();
                sh(&repo, &["init", "-q", "-b", "main"]);
                std::fs::write(repo.join("README"), "x\n").unwrap();
                sh(&repo, &["add", "-A"]);
                sh(
                    &repo,
                    &[
                        "-c",
                        "user.name=t",
                        "-c",
                        "user.email=t@t",
                        "commit",
                        "-qm",
                        "init",
                    ],
                );
                std::fs::write(
                    s.config_dir.join("projects").join(format!("{name}.toml")),
                    format!(
                        "path = \"{}\"\nbase = \"main\"\ntest = \"true\"\n{extra}",
                        repo.display()
                    ),
                )
                .unwrap();
            }
            OWN.into()
        },
        &[],
    );
    attention::pause(&w.d, "", "claims", "test", 3600).unwrap();
    w
}

fn pid(d: &Arc<Daemon>) -> ProjectId {
    d.project_id_by_name("scratch").unwrap()
}

fn state(d: &Arc<Daemon>, t: TaskId) -> State {
    d.world().unwrap().tasks[&t].state
}

fn cancel(d: &Arc<Daemon>, task: TaskId) {
    d.emit(EventKind::Canceled {
        task,
        delete_branch: true,
    })
    .unwrap();
}

/// Today's midnight, UTC.
fn midnight() -> u64 {
    now_ms() / DAY * DAY
}

fn nothing(except: &[&str]) -> Vec<Value> {
    CATEGORIES
        .iter()
        .filter(|c| !except.contains(c))
        .map(|c| json!({"category": c, "because": "the facts show none"}))
        .collect()
}

fn finding(title: &str, category: &str) -> Value {
    json!({"category": category, "title": title, "body": format!("{title}. Cause and change."),
           "evidence": EVIDENCE, "severity": "low", "cost": "small"})
}

/// Play the review's agent: write `result` and have the daemon read it. Returns the log dir.
fn answer(d: &Arc<Daemon>, task: TaskId, result: &Value) -> tempfile::TempDir {
    let wt = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(wt.path().join(introspect::ANALYST_FILE), result.to_string()).unwrap();
    let r = introspect::ingest_analyst(d, task, "scratch", wt.path(), dir.path());
    assert_eq!(r, introspect::Ingest::Done);
    dir
}

#[test]
fn with_no_file_the_review_is_on_at_midnight_on_opus_at_medium() {
    let w = world();
    let s = review::status(&w.d)["settings"].clone();
    assert_eq!(s["enabled"], true);
    assert_eq!(s["at"], "00:00");
    assert_eq!(s["every_days"], 1);
    assert_eq!(s["model"], "opus");
    assert_eq!(s["effort"], "medium");
    assert_eq!(s["max_proposals"], 10);
    assert!(!review::path(&w.d).exists());
}

#[test]
fn bad_settings_are_refused_and_a_change_keeps_the_comments_and_says_who() {
    let w = world();
    let d = &w.d;
    for bad in [
        json!({"at": "24:00"}),
        json!({"at": 5}),
        json!({"every_days": 0}),
        json!({"every_days": 31}),
        json!({"max_proposals": 0}),
        json!({"effort": "ultra"}),
        json!({"enabled": "no"}),
        json!({"weekday": "monday"}),
        json!({"at": "01:00", "colour": "red"}),
    ] {
        let (code, why) = review::set(d, &bad, "brian").unwrap_err();
        assert_eq!(code, 400, "{bad}: {why}");
    }
    assert!(!review::path(d).exists(), "a refused change writes nothing");

    std::fs::write(
        review::path(d),
        "# my review\nat = \"01:00\" # after the backups\n",
    )
    .unwrap();
    review::set(
        d,
        &json!({"at": "3:30", "every_days": 7, "enabled": false, "as": "brian"}),
        "brian",
    )
    .unwrap();
    let text = std::fs::read_to_string(review::path(d)).unwrap();
    assert!(text.contains("# my review"), "{text}");
    assert!(
        text.contains("at = \"03:30\" # after the backups"),
        "{text}"
    );
    assert!(text.contains("changed_by = \"brian\""), "{text}");
    assert!(text.contains("changed_at = "), "{text}");
    let s = review::status(d)["settings"].clone();
    assert_eq!(
        (&s["at"], &s["every_days"], &s["enabled"]),
        (&json!("03:30"), &json!(7), &json!(false))
    );
    assert_eq!(review::status(d)["next_run"], Value::Null);

    // A file that cannot be read turns the review off and says why.
    std::fs::write(review::path(d), "at = \"nine\"\n").unwrap();
    let s = review::status(d);
    assert_eq!(s["settings"]["enabled"], false);
    assert!(s["problem"].as_str().unwrap().contains("`at`"));
}

#[test]
fn it_is_filed_at_the_next_midnight_once_after_a_gap_and_never_while_one_is_open() {
    let w = world();
    let d = &w.d;
    let day0 = midnight();
    // First looked at just after midnight: nothing until the next one.
    assert_eq!(review::tick_at(d, day0 + 60_000, 0), None);
    assert_eq!(review::tick_at(d, day0 + 23 * HOUR, 0), None);
    let first = review::tick_at(d, day0 + DAY, 0).expect("due at the next midnight");
    // Open: not filed again, however late.
    assert_eq!(review::tick_at(d, day0 + 3 * DAY, 0), None);
    let row = d.store.schedule(review::ROW).unwrap().unwrap();
    assert!(
        row.last_status.contains("still open"),
        "{}",
        row.last_status
    );
    assert!(review::file(d, day0 + 3 * DAY).is_err(), "nor by hand");
    answer(
        d,
        first,
        &json!({"findings": [], "nothing_found": nothing(&[])}),
    );
    cancel(d, first);

    // Not watched for days: one catch-up run, and no second.
    let back = day0 + 6 * DAY + 9 * HOUR;
    let second = review::tick_at(d, back, 0).expect("a catch-up run");
    assert_eq!(review::tick_at(d, back + 60_000, 0), None);
    cancel(d, second);
    assert_eq!(review::tick_at(d, back + 120_000, 0), None);
    let third = review::tick_at(d, day0 + 7 * DAY, 0).expect("the next night");
    cancel(d, third);

    // Off is never due.
    review::set(d, &json!({"enabled": false}), "brian").unwrap();
    assert_eq!(review::tick_at(d, day0 + 40 * DAY, 0), None);
}

#[test]
fn every_seven_days_waits_a_week() {
    let w = world();
    let d = &w.d;
    review::set(d, &json!({"every_days": 7}), "brian").unwrap();
    let day0 = midnight();
    assert_eq!(review::tick_at(d, day0 + 60_000, 0), None);
    for k in 1..7 {
        assert_eq!(review::tick_at(d, day0 + k * DAY, 0), None, "day {k}");
    }
    assert!(review::tick_at(d, day0 + 7 * DAY, 0).is_some());
}

#[test]
fn a_held_account_waits_and_is_tried_again() {
    let w = world();
    let d = &w.d;
    let day0 = midnight();
    assert_eq!(review::tick_at(d, day0 + 60_000, 0), None);
    let account = d.project(pid(d)).unwrap().account.clone();
    holds::hold(d, &account, "brian", None, "").unwrap();
    assert_eq!(review::tick_at(d, day0 + DAY, 0), None);
    let row = d.store.schedule(review::ROW).unwrap().unwrap();
    assert!(row.last_status.contains("held"), "{}", row.last_status);
    holds::release(d, &account, "brian").unwrap();
    assert!(review::tick_at(d, day0 + DAY + 60_000, 0).is_some());
}

#[test]
fn one_review_covers_every_included_project_and_asks_the_six_questions_on_opus() {
    let w = world();
    let d = &w.d;
    d.store
        .put_schedule(&mason_store::ScheduleRow {
            name: "analyst:web".into(),
            project: "web".into(),
            kind: "analyst".into(),
            every_ms: DAY,
            ..Default::default()
        })
        .unwrap();
    let day0 = midnight();
    review::tick_at(d, day0 + 60_000, 0);
    let task = review::tick_at(d, day0 + DAY, 0).unwrap();
    let world = d.world().unwrap();
    assert_eq!(world.tasks.len(), 1, "no per-project analyst task");
    let t = &world.tasks[&task];
    assert_eq!(t.project, pid(d));
    assert_eq!(t.title, introspect::ANALYST_TITLE);
    assert_eq!(t.priority, Priority::Low);
    assert_eq!(t.state, State::Queued);
    assert!(
        t.body.contains("Projects covered: scratch, web."),
        "{}",
        t.body
    );
    for (category, question) in introspect::QUESTIONS {
        assert!(
            t.body.contains(&format!("`{category}`: {question}")),
            "{category}"
        );
    }
    assert_eq!(
        d.store.task_overrides(task.0).unwrap(),
        vec![
            ("step.develop.effort".to_string(), "medium".to_string()),
            ("step.develop.model".to_string(), "opus".to_string()),
        ]
    );
    // What the session is launched with: `medium` is also an abstract level, which must not
    // leave the CLI without an effort.
    let def = d.current_def(pid(d));
    let step = def.first_in(State::Developing).unwrap().clone();
    let overrides = d.store.task_overrides(task.0).unwrap();
    let resolved = resolve(&def, &step, &d.overlay(pid(d)), &[], &overrides, now_ms()).unwrap();
    let mut launched = launch(&resolved, &Mapping::builtin("claude"), &[]);
    review::pin_effort(d, task, &step.name, &mut launched);
    assert_eq!(launched.model.as_deref(), Some("opus"));
    assert_eq!(launched.effort.as_deref(), Some("medium"));
    assert!(introspect::is_analyst_task(d, task, "scratch"));
    assert!(d.store.schedule("analyst:web").unwrap().is_none());
    assert!(
        d.store.reservations(task.0).unwrap().is_empty(),
        "a review takes no numbers"
    );
}

/// A task of `project` that the owner filed and that landed or was canceled, at `at`. The
/// event clock only moves forward, so a past `at` needs a store with nothing newer in it.
fn finished_task(store: &Store, project: ProjectId, title: &str, landed: bool, at: u64) -> TaskId {
    let append = |kind: EventKind| {
        store
            .append(Event {
                at: Timestamp(at),
                kind,
            })
            .unwrap()
    };
    let task = append(EventKind::TaskFiled {
        project,
        title: title.into(),
        body: format!("{title}. Cause and change."),
        priority: Priority::Normal,
        trust: TrustSource::Owner,
        explored: true,
        proposed: false,
        blockers: vec![],
        not_before: None,
    })
    .effects
    .iter()
    .find_map(|e| match e {
        Effect::TaskCreated { task } => Some(*task),
        _ => None,
    })
    .unwrap();
    let want = if landed {
        append(EventKind::Admitted { task, entry: None });
        append(EventKind::ClosedNoChange {
            task,
            reason: "done".into(),
        });
        State::Landed
    } else {
        append(EventKind::Canceled {
            task,
            delete_branch: true,
        });
        State::Canceled
    };
    assert_eq!(store.world().unwrap().tasks[&task].state, want);
    task
}

fn filing(d: &Arc<Daemon>, title: &str, all: bool) -> Result<proposals::Filed, String> {
    proposals::file(
        d,
        &proposals::Filing {
            from_task: None,
            project: pid(d),
            title: title.into(),
            body: format!("{title}. Cause and change."),
            evidence: EVIDENCE.into(),
            blocks_task: false,
            explored: true,
            estimate: None,
            lookback: all.then_some(proposals::Lookback::All),
        },
    )
}

#[test]
fn the_wide_search_compares_with_every_task_and_the_usual_one_is_unchanged() {
    // A daemon's history from 30 days ago, in the store before this one starts.
    let scratch = Scratch::new("true", json!({"steps": {}}));
    scratch.write_project("scratch", "true", OWN);
    std::fs::create_dir_all(&scratch.data_dir).unwrap();
    let then = now_ms() - 30 * DAY;
    let (landed, canceled) = {
        let store = Store::open(scratch.data_dir.join("store.db")).unwrap();
        let project = ProjectId(1);
        store
            .append(Event {
                at: Timestamp(then),
                kind: EventKind::ProjectRegistered {
                    project,
                    name: "scratch".into(),
                },
            })
            .unwrap();
        (
            finished_task(
                &store,
                project,
                "Landing queue idles between batches",
                true,
                then,
            ),
            finished_task(&store, project, "Monitor redraws while hidden", false, then),
        )
    };
    let d = &scratch.start();
    attention::pause(d, "", "claims", "test", 3600).unwrap();
    assert_eq!(d.world().unwrap().tasks[&landed].state_entered_at.0, then);
    // The landed one was a proposal too, long ago.
    d.store
        .put_proposal(&ProposalRow {
            task: landed.0,
            project: "scratch".into(),
            source: "agent".into(),
            severity: "normal".into(),
            route: "project".into(),
            filed_by: None,
            title: "Landing queue idles between batches".into(),
            body: "Landing queue idles between batches. Cause and change.".into(),
            evidence: EVIDENCE.into(),
            key: "batches idles landing queue".into(),
            status: "approved".into(),
            reject_reason: String::new(),
            publish_state: "local".into(),
            access: String::new(),
            branch: String::new(),
            pr: None,
            pr_state: String::new(),
            at: now_ms() - 30 * DAY,
            decided_by: String::new(),
        })
        .unwrap();
    for (title, of) in [
        ("Landing queue idles between batches", landed),
        ("Monitor redraws while hidden", canceled),
    ] {
        let e = filing(d, title, true).unwrap_err();
        assert!(
            e.starts_with(&format!("duplicate of #{}:", of.0)),
            "{title}: {e}"
        );
    }
    // The canceled one was the owner's, never a proposal. A new idea is filed, once.
    let open = filing(d, "Disk fills during the night", true).unwrap();
    assert!(open.proposed);
    let fresh = filing(d, "Disk fills during the night", true).unwrap_err();
    assert!(fresh.contains(&format!("#{}", open.task.0)), "{fresh}");
    // Another caller still accepts what landed a month ago.
    filing(d, "Landing queue idles between batches", false).unwrap();
}

#[test]
fn findings_are_proposed_capped_at_ten_and_the_run_is_summed_up() {
    let w = world();
    let d = &w.d;
    let landed = finished_task(
        &d.store,
        pid(d),
        "Landing queue idles between batches",
        true,
        now_ms(),
    );
    let task = review::file(d, now_ms()).unwrap();
    let titles = [
        "Alpha takeover drops checks",
        "Bravo cache starts cold",
        "Charlie suite runs twice",
        "Delta alert repeats forever",
        "Echo worktree never swept",
        "Foxtrot retry loses notes",
        "Golf monitor freezes scrolling",
        "Hotel lease leaks slots",
        "India update ignores holds",
        "Juliet digest misses waits",
        "Kilo pairing code expires",
    ];
    let mut findings = vec![finding("Landing queue idles between batches", "speed")];
    findings.extend(titles.iter().map(|t| finding(t, "coverage")));
    findings.insert(1, finding("Morale is low", "morale"));
    let mut vague = finding("Something feels slow", "speed");
    vague["evidence"] = json!("it feels slow to me");
    findings.insert(1, vague);
    let result = json!({
        "findings": findings,
        "duplicates": [{"category": "stuck", "idea": "Old stall", "task": landed.0}],
        "nothing_found": nothing(&["speed", "coverage", "stuck"]),
    });
    let dir = answer(d, task, &result);

    let rows = d.store.proposals().unwrap();
    assert_eq!(rows.len(), 10, "{rows:?}");
    for r in &rows {
        assert_eq!(state(d, TaskId(r.task)), State::Proposed, "{}", r.title);
        assert_eq!(r.filed_by, Some(task.0));
    }
    let last = review::status(d)["last"].clone();
    assert_eq!(last["task"], task.0);
    assert_eq!(last["filed"].as_array().unwrap().len(), 10);
    assert_eq!(last["capped"], json!(["Kilo pairing code expires"]));
    assert_eq!(last["duplicates"][0]["task"], landed.0);
    assert_eq!(last["duplicates"][1]["task"], landed.0);
    assert_eq!(last["duplicates"][1]["by"], "mason");
    assert_eq!(last["refused"].as_array().unwrap().len(), 2);
    let row = d.store.schedule(review::ROW).unwrap().unwrap();
    for part in [
        "10 filed",
        "2 duplicate(s) dropped",
        "nothing found: ",
        "crash (because the facts show none)",
        "1 not filed, over the cap of 10",
    ] {
        assert!(
            row.last_status.contains(part),
            "{part}: {}",
            row.last_status
        );
    }
    let refused = std::fs::read_to_string(dir.path().join("analyst-refused.txt")).unwrap();
    assert!(refused.contains("unknown category"), "{refused}");
    assert!(refused.contains("Something feels slow"), "{refused}");

    attention::sync_alerts(d);
    let alert = d
        .store
        .alerts(true)
        .unwrap()
        .into_iter()
        .find(|a| a.kind == "review")
        .expect("the review's notice");
    assert_eq!(
        alert.text,
        "Pipeline review: 10 proposed, 2 duplicates dropped, 1 over the cap"
    );
}

#[test]
fn the_facts_are_written_into_the_worktree_and_the_submission_needs_the_result() {
    let w = world();
    let d = &w.d;
    let old = finished_task(
        &d.store,
        pid(d),
        "Monitor redraws while hidden",
        false,
        now_ms(),
    );
    let task = review::file(d, now_ms()).unwrap();
    let wt = tempfile::tempdir().unwrap();
    sh(wt.path(), &["init", "-q", "-b", "main"]);
    sh(
        wt.path(),
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-qm",
            "init",
            "--allow-empty",
        ],
    );
    review::write_facts(d, task, wt.path()).unwrap();
    let facts = wt.path().join(review::FACTS_DIR);
    for name in [
        "README.md",
        "digest-7d.json",
        "digest-window.json",
        "failed-tasks.json",
        "diagnoses.json",
        "retries.json",
        "interventions.json",
        "health.json",
        "durations.json",
        "tests.json",
        "batches.json",
        "daemon.json",
        "ci-watch.json",
        "owner.json",
        "tasks.json",
    ] {
        assert!(facts.join(name).is_file(), "{name}");
    }
    let read = |name: &str| -> Value {
        serde_json::from_str(&std::fs::read_to_string(facts.join(name)).unwrap()).unwrap()
    };
    let index = read("tasks.json");
    let entry = index
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == old.0)
        .unwrap();
    assert_eq!(entry["state"], "canceled");
    assert_eq!(entry["title"], "Monitor redraws while hidden");
    let digest = read("digest-7d.json");
    assert!(digest["scratch"].is_object() && digest["web"].is_object());
    assert!(digest.get("quiet").is_none());

    let check = review::validator(wt.path().to_path_buf());
    let head = mason_landing::git::rev_parse(wt.path(), "HEAD").unwrap();
    let args = json!({"head": head, "summary": "reviewed"});
    assert!(check("submit_change", &args).is_err(), "no result yet");
    std::fs::write(
        wt.path().join(introspect::ANALYST_FILE),
        json!({"findings": [], "nothing_found": nothing(&[])}).to_string(),
    )
    .unwrap();
    check("submit_change", &args).unwrap();
    // Neither the facts nor the result shows as work to commit.
    let dirty = mason_landing::git::run(wt.path(), &["status", "--porcelain"]).unwrap();
    assert!(dirty.trim().is_empty(), "{dirty}");
}

const NOTE: &str = "No timing assertions, use a hang guard";

/// Write the facts of a fresh review and read `corrections.json`.
fn corrections_of(d: &Arc<Daemon>) -> (TaskId, Value) {
    let task = review::file(d, now_ms()).unwrap();
    let wt = tempfile::tempdir().unwrap();
    sh(wt.path(), &["init", "-q", "-b", "main"]);
    review::write_facts(d, task, wt.path()).unwrap();
    let text = std::fs::read_to_string(wt.path().join(review::FACTS_DIR).join("corrections.json"))
        .unwrap();
    (task, serde_json::from_str(&text).unwrap())
}

fn lesson(title: &str, project: &str) -> Value {
    let mut f = finding(title, introspect::LESSON);
    f["project"] = json!(project);
    f
}

#[test]
fn a_correction_on_two_tasks_is_grouped_and_one_on_a_single_task_is_not() {
    let w = world();
    let d = &w.d;
    let (a, b, c) = (
        finished_task(&d.store, pid(d), "First", false, now_ms()),
        finished_task(&d.store, pid(d), "Second", false, now_ms()),
        finished_task(&d.store, pid(d), "Third", false, now_ms()),
    );
    for t in [a, b] {
        mason_daemon::ownernotes::append(&d.task_dir(t), now_ms(), "develop", NOTE);
    }
    mason_daemon::ownernotes::append(&d.task_dir(c), now_ms(), "develop", "Rename the flag");
    // Twice on one task is still one task.
    mason_daemon::ownernotes::append(&d.task_dir(c), now_ms(), "test", "Rename the flag");
    let (_, facts) = corrections_of(d);
    let repeated = facts["repeated"].as_array().unwrap();
    assert_eq!(repeated.len(), 1, "{facts}");
    assert_eq!(repeated[0]["tasks"], json!([a.0, b.0]));
    assert_eq!(repeated[0]["items"].as_array().unwrap().len(), 2);
    assert_eq!(facts["items"].as_array().unwrap().len(), 4);
}

#[test]
fn a_lesson_is_filed_once_citing_both_tasks_and_not_again() {
    let w = world();
    let d = &w.d;
    let task = review::file(d, now_ms()).unwrap();
    let mut f = lesson(
        "CLAUDE.md: no timing assertions, use a hang guard",
        "scratch",
    );
    f["body"] = json!(format!(
        "Add to CLAUDE.md: \"{NOTE}\". Task #11: \"{NOTE}\". Task #12: \"{NOTE}\"."
    ));
    let result = json!({"findings": [f.clone()], "nothing_found": nothing(&[])});
    answer(d, task, &result);
    let world_now = d.world().unwrap();
    let proposed: Vec<_> = world_now
        .tasks
        .values()
        .filter(|t| t.title.starts_with("CLAUDE.md: no timing"))
        .collect();
    assert_eq!(proposed.len(), 1);
    assert_eq!(proposed[0].state, State::Proposed);
    assert!(proposed[0].body.contains("Task #11") && proposed[0].body.contains("Task #12"));
    // The same proposal the next night is a duplicate of the first.
    cancel(d, task);
    let again = review::file(d, now_ms()).unwrap();
    answer(d, again, &result);
    let n = d
        .world()
        .unwrap()
        .tasks
        .values()
        .filter(|t| t.title.starts_with("CLAUDE.md: no timing"))
        .count();
    assert_eq!(n, 1);
}

#[test]
fn at_most_three_lessons_are_filed_a_night() {
    let w = world();
    let d = &w.d;
    let task = review::file(d, now_ms()).unwrap();
    let findings: Vec<Value> = [
        "Alpha line about fixtures",
        "Bravo line about signing",
        "Charlie line about hangs",
        "Delta line about clippy",
    ]
    .iter()
    .map(|t| lesson(t, "scratch"))
    .collect();
    let dir = answer(
        d,
        task,
        &json!({"findings": findings, "nothing_found": nothing(&[])}),
    );
    let filed = d
        .world()
        .unwrap()
        .tasks
        .values()
        .filter(|t| t.state == State::Proposed || t.state == State::Queued)
        .count();
    assert_eq!(filed, introspect::MAX_LESSONS);
    let refused = std::fs::read_to_string(dir.path().join("analyst-refused.txt")).unwrap();
    assert!(refused.contains("Delta line about clippy"), "{refused}");
}

#[test]
fn the_review_prompt_has_the_seventh_part() {
    let w = world();
    let t = review::file(&w.d, now_ms()).unwrap();
    let body = w.d.world().unwrap().tasks[&t].body.clone();
    assert!(body.contains("corrections.json"), "{body}");
    assert!(body.contains("two or more tasks"));
}
