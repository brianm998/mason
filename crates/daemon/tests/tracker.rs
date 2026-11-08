// SPDX-License-Identifier: GPL-3.0-or-later

//! The tracker as an outside signal: what is imported and what is not, the
//! split of authority, mirroring out, and polling (ETag, a permanent error
//! stops it, refresh on demand), plus the webhook's signature check.

mod support;
use mason_adapter_api::service::*;
use mason_adapter_api::webhook::{hex, hmac_sha256};
use mason_core::*;
use mason_daemon::config::TrackerCfg;
use mason_daemon::tracker::{mirror_out, poll_once, sync_items};
use mason_daemon::Daemon;
use serde_json::json;
use std::sync::{Arc, Mutex};
use support::*;

#[derive(Default)]
struct Fake {
    script: Mutex<Vec<Result<Listing, ApiError>>>,
    seen_etags: Mutex<Vec<Option<String>>>,
    sent: Mutex<Vec<(u64, String, bool)>>,
    fail_mirror: Mutex<bool>,
}

impl TrackerAdapter for Fake {
    fn name(&self) -> &'static str {
        "fake-tracker"
    }
    fn capabilities(&self) -> TrackerCaps {
        TrackerCaps {
            conditional_polling: true,
            comments: true,
            labels: true,
            webhooks: true,
            dependencies: false,
        }
    }
    fn list(&self, etag: Option<&str>) -> Result<Listing, ApiError> {
        self.seen_etags.lock().unwrap().push(etag.map(String::from));
        let mut s = self.script.lock().unwrap();
        if s.is_empty() {
            Ok(Listing::NotModified)
        } else {
            s.remove(0)
        }
    }
    fn mirror(&self, item: &Item, note: &str) -> Result<(), ApiError> {
        if *self.fail_mirror.lock().unwrap() {
            return Err(ApiError::new(ErrorKind::Unavailable, "down"));
        }
        self.sent
            .lock()
            .unwrap()
            .push((item.number, note.into(), false));
        Ok(())
    }
    fn close(&self, item: &Item, note: &str) -> Result<(), ApiError> {
        if *self.fail_mirror.lock().unwrap() {
            return Err(ApiError::new(ErrorKind::Unavailable, "down"));
        }
        self.sent
            .lock()
            .unwrap()
            .push((item.number, note.into(), true));
        Ok(())
    }
}

fn item(n: u64, title: &str, author: &str, labels: &[&str]) -> Item {
    Item {
        id: format!("I_{n}"),
        number: n,
        title: title.into(),
        body: format!("body of {title}"),
        priority: "normal".into(),
        labels: labels.iter().map(|s| s.to_string()).collect(),
        author: author.into(),
        open: true,
        is_pr: false,
    }
}

fn cfg() -> TrackerCfg {
    TrackerCfg {
        api: "x".into(),
        repo: "o/r".into(),
        token: "env:X".into(),
        label: "mason".into(),
        interval_s: 30,
        webhook_secret: None,
    }
}

fn world() -> (Scratch, Arc<Daemon>, ProjectId) {
    let s = Scratch::new("true", json!({"steps": {}}));
    let d = s.start();
    // Imported tasks are not claimed: nothing here is about running them.
    mason_daemon::attention::pause(&d, "", "claims", "test", 3600).unwrap();
    let pid = d.project_id_by_name("scratch").unwrap();
    (s, d, pid)
}

fn task_for(d: &Arc<Daemon>, n: u64) -> Option<Task> {
    let row = d
        .store
        .tracker_items("scratch")
        .unwrap()
        .into_iter()
        .find(|r| r.number == n)?;
    d.world().unwrap().tasks.get(&TaskId(row.task)).cloned()
}

#[test]
fn outside_issues_are_ignored_until_the_owner_labels_them_and_stay_untrusted() {
    let (_s, d, pid) = world();
    let owner = d.cfg.owner_names[0].clone();
    let mut pr = item(5, "A pull request", &owner, &["mason"]);
    pr.is_pr = true;
    let items = vec![
        item(1, "By the owner", &owner, &[]),
        item(2, "A drive-by", "stranger", &[]),
        item(3, "A stranger's, labeled", "stranger", &["mason"]),
        pr,
        Item {
            open: false,
            ..item(4, "Closed before we met", &owner, &[])
        },
    ];
    let sum = sync_items(&d, pid, "mason", &items);
    assert_eq!((sum.imported, sum.ignored), (2, 2), "{sum:?}");
    assert_eq!(task_for(&d, 1).unwrap().trust, TrustSource::Owner);
    assert!(
        task_for(&d, 2).is_none(),
        "an unlabeled stranger's issue is not a task"
    );
    assert_eq!(
        task_for(&d, 3).unwrap().trust,
        TrustSource::Outside,
        "labeled, but the words are still untrusted"
    );
    assert!(task_for(&d, 5).is_none(), "a pull request is never a task");
    // The owner labels the drive-by: the next look imports it.
    let sum = sync_items(
        &d,
        pid,
        "mason",
        &[item(2, "A drive-by", "stranger", &["mason"])],
    );
    assert_eq!(sum.imported, 1);
    assert_eq!(task_for(&d, 2).unwrap().trust, TrustSource::Outside);
    // Looking again changes nothing.
    assert_eq!(
        sync_items(&d, pid, "mason", &items[..3]),
        mason_daemon::tracker::Summary {
            ignored: 0,
            // The three imported issues are seen again and have not changed.
            unchanged: 3,
            ..Default::default()
        }
    );
}

#[test]
fn the_tracker_owns_title_body_priority_and_closure() {
    let (_s, d, pid) = world();
    let owner = d.cfg.owner_names[0].clone();
    sync_items(&d, pid, "mason", &[item(1, "Old title", &owner, &[])]);
    let t = task_for(&d, 1).unwrap();
    assert_eq!(
        (t.title.as_str(), t.priority),
        ("Old title", Priority::Normal)
    );
    let mut edited = item(1, "New title", &owner, &[]);
    edited.body = "new body".into();
    edited.priority = "urgent".into();
    let sum = sync_items(&d, pid, "mason", std::slice::from_ref(&edited));
    assert_eq!(sum.updated, 1);
    let t = task_for(&d, 1).unwrap();
    assert_eq!(
        (t.title.as_str(), t.body.as_str(), t.priority),
        ("New title", "new body", Priority::Urgent)
    );
    // Closed there: the task is cancelled; its state is the database's, but closure is the tracker's.
    edited.open = false;
    let sum = sync_items(&d, pid, "mason", &[edited]);
    assert_eq!(sum.closed, 1);
    assert_eq!(task_for(&d, 1).unwrap().state, State::Canceled);
    d.store.verify().unwrap();
}

#[test]
fn state_is_mirrored_out_best_effort_and_never_blocks_work() {
    let (_s, d, pid) = world();
    let owner = d.cfg.owner_names[0].clone();
    sync_items(
        &d,
        pid,
        "mason",
        &[item(1, "Lands", &owner, &[]), item(2, "Fails", &owner, &[])],
    );
    let (t1, t2) = (task_for(&d, 1).unwrap().id, task_for(&d, 2).unwrap().id);
    let fake = Fake::default();
    // Nothing to say yet.
    mirror_out(&d, pid, &fake);
    assert!(fake.sent.lock().unwrap().is_empty());
    d.emit(EventKind::Canceled {
        task: t2,
        delete_branch: false,
    })
    .unwrap();
    // Make t1 land and t2 fail through the real events: put both in a step first.
    let def = d.current_def(pid);
    d.emit(EventKind::Admitted {
        task: t1,
        entry: Some(Entry {
            def: def.hash.clone(),
            step: "land".into(),
            state: State::Landing,
            route: Route::Explore,
        }),
    })
    .unwrap();
    d.emit(EventKind::StepSucceeded {
        task: t1,
        next: Some(Next {
            step: String::new(),
            state: State::Landed,
        }),
    })
    .ok();
    // The tracker is down: mirroring fails, and nothing else is affected.
    *fake.fail_mirror.lock().unwrap() = true;
    mirror_out(&d, pid, &fake);
    assert!(fake.sent.lock().unwrap().is_empty());
    assert_eq!(
        d.store
            .tracker_items("scratch")
            .unwrap()
            .iter()
            .find(|r| r.number == 1)
            .unwrap()
            .mirrored,
        "",
        "not remembered, so it is tried again"
    );
    // It comes back: the landing closes the issue, once.
    *fake.fail_mirror.lock().unwrap() = false;
    mirror_out(&d, pid, &fake);
    mirror_out(&d, pid, &fake);
    let sent = fake.sent.lock().unwrap().clone();
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!((sent[0].0, sent[0].2), (1, true), "closed with a note");
    assert!(sent[0].1.contains("landed"));
}

#[test]
fn polling_is_conditional_a_permanent_error_stops_it_and_a_refresh_tries_once_more() {
    let (_s, d, pid) = world();
    let owner = d.cfg.owner_names[0].clone();
    let fake = Fake::default();
    fake.script.lock().unwrap().push(Ok(Listing::Items {
        items: vec![item(1, "One", &owner, &[])],
        etag: Some("\"e1\"".into()),
    }));
    let sum = poll_once(&d, pid, &fake, &cfg()).unwrap();
    assert_eq!(sum.imported, 1);
    // The next look carries the ETag and the answer is "unchanged".
    let sum = poll_once(&d, pid, &fake, &cfg()).unwrap();
    assert_eq!(sum, Default::default());
    assert_eq!(
        fake.seen_etags.lock().unwrap().clone(),
        vec![None, Some("\"e1\"".to_string())]
    );
    // The repository is gone: a permanent error. The poll stops and the owner is told at once.
    fake.script
        .lock()
        .unwrap()
        .push(Err(ApiError::new(ErrorKind::NotFound, "HTTP 404")));
    let e = poll_once(&d, pid, &fake, &cfg()).unwrap_err();
    assert!(e.contains("stopped"), "{e}");
    assert!(d.tracker_stopped(pid).is_some());
    assert!(
        mason_daemon::attention::alerts_now(&d)
            .iter()
            .any(|(_, k, t)| k == "tracker" && t.contains("stopped")),
        "attention is raised at once"
    );
    // A refresh asks once more; found again, the poll runs and the alert clears.
    d.tracker_refresh(pid);
    assert!(d.tracker_stopped(pid).is_none());
    poll_once(&d, pid, &fake, &cfg()).unwrap();
    assert!(!mason_daemon::attention::alerts_now(&d)
        .iter()
        .any(|(_, k, _)| k == "tracker"));
}

#[test]
fn an_outage_is_retried_with_backoff_not_stopped() {
    let (_s, d, pid) = world();
    let fake = Fake::default();
    fake.script
        .lock()
        .unwrap()
        .push(Err(ApiError::new(ErrorKind::Unavailable, "HTTP 503")));
    assert!(poll_once(&d, pid, &fake, &cfg()).is_err());
    assert!(
        d.tracker_stopped(pid).is_none(),
        "an outage is not permanent"
    );
    assert!(mason_daemon::attention::alerts_now(&d)
        .iter()
        .any(|(_, k, t)| k == "tracker" && t.contains("trying again")));
}

#[test]
fn a_webhook_is_acted_on_only_when_its_signature_verifies() {
    let s = Scratch::new("true", json!({"steps": {}}));
    let secret_file = s.root.path().join("secret");
    std::fs::write(&secret_file, "s3cret\n").unwrap();
    s.write_project(
        "scratch",
        "true",
        &format!("[tracker]\nkind = \"github\"\nrepo = \"o/r\"\ntoken = \"env:MASON_NO_SUCH_TOKEN\"\nwebhook_secret = \"file:{}\"\ninterval_s = 3600\n", secret_file.display()),
    );
    let d = s.start();
    let body = json!({"action": "labeled", "repository": {"full_name": "o/r"}}).to_string();
    let sig = format!("sha256={}", hex(&hmac_sha256(b"s3cret", body.as_bytes())));
    let post = |sig: &str, body: &str| {
        let d = d.clone();
        let (sig, body) = (sig.to_string(), body.to_string());
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async move {
                use tower::ServiceExt;
                let req = axum::http::Request::builder()
                    .method("POST")
                    .uri("/hooks/github")
                    .header("x-hub-signature-256", sig)
                    .body(axum::body::Body::from(body))
                    .unwrap();
                mason_daemon::http::api_router(d)
                    .oneshot(req)
                    .await
                    .unwrap()
                    .status()
                    .as_u16()
            })
    };
    assert_eq!(post(&sig, &body), 200);
    assert_eq!(post("sha256=00", &body), 401, "a bad signature is refused");
    assert_eq!(
        post(&sig, &body.replace("labeled", "closed")),
        401,
        "a changed body is refused"
    );
    let other = json!({"repository": {"full_name": "someone/else"}}).to_string();
    let sig2 = format!("sha256={}", hex(&hmac_sha256(b"s3cret", other.as_bytes())));
    assert_eq!(
        post(&sig2, &other),
        401,
        "a repository the project does not track is refused"
    );
}

// ---- dependencies ----------------------------------------------------------------------

use mason_daemon::tracker::{sync_with_deps, NativeDeps};

fn native(pairs: &[(u64, &[(u64, bool)])]) -> NativeDeps {
    pairs
        .iter()
        .map(|(n, bs)| {
            (
                *n,
                bs.iter()
                    .map(|(b, open)| Blocker {
                        number: *b,
                        open: *open,
                    })
                    .collect(),
            )
        })
        .collect()
}

fn with_body(mut i: Item, body: &str) -> Item {
    i.body = body.into();
    i
}

fn alerts(d: &Arc<Daemon>) -> Vec<String> {
    mason_daemon::attention::alerts_now(d)
        .into_iter()
        .filter(|(_, k, _)| k == "tracker")
        .map(|(_, _, t)| t)
        .collect()
}

#[test]
fn a_chain_is_imported_in_dependency_order_whatever_the_listing_order() {
    let (_s, d, pid) = world();
    let o = d.cfg.owner_names[0].clone();
    // Newest first, as GitHub lists: 103 (body line) on 102 (native) on 101.
    let items = vec![
        with_body(item(103, "Third", &o, &[]), "Depends on #102"),
        item(102, "Second", &o, &[]),
        item(101, "First", &o, &[]),
    ];
    let nat = native(&[(103, &[]), (102, &[(101, true)]), (101, &[])]);
    let sum = sync_with_deps(&d, pid, "mason", "o/r", &items, Some(&nat));
    assert_eq!(sum.imported, 3, "{sum:?}");
    let (t1, t2, t3) = (
        task_for(&d, 101).unwrap(),
        task_for(&d, 102).unwrap(),
        task_for(&d, 103).unwrap(),
    );
    assert!(t1.id < t2.id && t2.id < t3.id, "dependency order");
    assert!(t1.blockers.is_empty());
    assert_eq!(t2.blockers.iter().copied().collect::<Vec<_>>(), vec![t1.id]);
    assert_eq!(t3.blockers.iter().copied().collect::<Vec<_>>(), vec![t2.id]);
    assert!(matches!(t3.activity, Activity::Held(_)));
    assert!(alerts(&d).is_empty());
}

#[test]
fn an_issue_whose_open_blocker_is_not_a_task_waits_and_says_so() {
    let (_s, d, pid) = world();
    let o = d.cfg.owner_names[0].clone();
    let items = vec![
        with_body(item(2, "Dependent", &o, &[]), "Blocked by #1"),
        item(1, "A stranger's, unlabeled", "stranger", &[]),
    ];
    let sum = sync_with_deps(&d, pid, "mason", "o/r", &items, None);
    assert_eq!(sum.imported, 0);
    assert!(task_for(&d, 2).is_none());
    assert!(
        alerts(&d)
            .iter()
            .any(|t| t.contains("issue #2 is not imported") && t.contains("#1")),
        "{:?}",
        alerts(&d)
    );
    // The owner labels it: both come in, in order.
    let items = vec![
        with_body(item(2, "Dependent", &o, &[]), "Blocked by #1"),
        item(1, "A stranger's, labeled", "stranger", &["mason"]),
    ];
    assert_eq!(
        sync_with_deps(&d, pid, "mason", "o/r", &items, None).imported,
        2
    );
    assert!(alerts(&d).is_empty());
    assert!(task_for(&d, 1).unwrap().id < task_for(&d, 2).unwrap().id);
}

#[test]
fn a_closed_blocker_that_was_never_a_task_is_nothing_to_wait_for() {
    let (_s, d, pid) = world();
    let o = d.cfg.owner_names[0].clone();
    let items = vec![
        item(7, "Dependent", &o, &[]),
        Item {
            open: false,
            ..item(3, "Long closed", &o, &[])
        },
    ];
    // Native names #3 (closed) and #99 (not listed, closed): both ignored.
    let nat = native(&[(7, &[(3, false), (99, false)])]);
    assert_eq!(
        sync_with_deps(&d, pid, "mason", "o/r", &items, Some(&nat)).imported,
        1
    );
    assert!(task_for(&d, 7).unwrap().blockers.is_empty());
}

#[test]
fn a_cycle_among_issues_is_not_imported_and_the_alert_names_it() {
    let (_s, d, pid) = world();
    let o = d.cfg.owner_names[0].clone();
    let items = vec![
        with_body(item(4, "Four", &o, &[]), "Depends on #5"),
        with_body(item(5, "Five", &o, &[]), "Depends on #4"),
        item(6, "Fine", &o, &[]),
    ];
    let sum = sync_with_deps(&d, pid, "mason", "o/r", &items, None);
    assert_eq!(sum.imported, 1);
    assert!(task_for(&d, 4).is_none() && task_for(&d, 5).is_none());
    assert!(
        alerts(&d).iter().any(|t| t.contains("#4 -> #5 -> #4")),
        "{:?}",
        alerts(&d)
    );
}

#[test]
fn a_change_on_the_tracker_holds_or_releases_a_queued_task_but_never_pulls_back_a_started_one() {
    let (_s, d, pid) = world();
    let o = d.cfg.owner_names[0].clone();
    let items = vec![
        item(1, "A", &o, &[]),
        item(2, "B", &o, &[]),
        item(3, "C", &o, &[]),
    ];
    let nat = native(&[(1, &[]), (2, &[]), (3, &[])]);
    sync_with_deps(&d, pid, "mason", "o/r", &items, Some(&nat));
    assert!(task_for(&d, 2).unwrap().blockers.is_empty());
    // A blocker is added on the tracker: the queued task is held.
    let nat = native(&[(1, &[]), (2, &[(1, true)]), (3, &[(1, true)])]);
    sync_with_deps(&d, pid, "mason", "o/r", &items, Some(&nat));
    assert!(matches!(
        task_for(&d, 2).unwrap().activity,
        Activity::Held(_)
    ));
    // The tracker's blocker goes away again: the queued task is released.
    let nat = native(&[(1, &[]), (2, &[]), (3, &[(1, true)])]);
    sync_with_deps(&d, pid, "mason", "o/r", &items, Some(&nat));
    assert!(
        task_for(&d, 2).unwrap().blockers.is_empty(),
        "removing a blocker releases a queued task"
    );
    assert_eq!(
        task_for(&d, 2).unwrap().activity,
        Activity::Waiting(WaitReason::Slot)
    );
    // A started task: add a new blocker after it started.
    let t1 = task_for(&d, 1).unwrap().id;
    let t2 = task_for(&d, 2).unwrap().id;
    d.emit(EventKind::Admitted {
        task: t2,
        entry: None,
    })
    .unwrap();
    let nat = native(&[(1, &[]), (2, &[(1, true)]), (3, &[(1, true)])]);
    sync_with_deps(&d, pid, "mason", "o/r", &items, Some(&nat));
    let t = task_for(&d, 2).unwrap();
    assert!(
        !t.blockers.contains(&t1),
        "a started task is not pulled back"
    );
    assert!(
        alerts(&d)
            .iter()
            .any(|a| a.contains("task #") && a.contains("not pulled back")),
        "{:?}",
        alerts(&d)
    );
}

#[test]
fn a_canceled_blocker_keeps_the_dependent_held_and_raises_an_alert() {
    let (_s, d, pid) = world();
    let o = d.cfg.owner_names[0].clone();
    let items = vec![item(1, "A", &o, &[]), item(2, "B", &o, &[])];
    let nat = native(&[(1, &[]), (2, &[(1, true)])]);
    sync_with_deps(&d, pid, "mason", "o/r", &items, Some(&nat));
    // Issue 1 is closed on the tracker without landing: its task is canceled.
    let items = vec![
        Item {
            open: false,
            ..item(1, "A", &o, &[])
        },
        item(2, "B", &o, &[]),
    ];
    sync_with_deps(&d, pid, "mason", "o/r", &items, Some(&nat));
    assert_eq!(task_for(&d, 1).unwrap().state, State::Canceled);
    assert!(matches!(
        task_for(&d, 2).unwrap().activity,
        Activity::Held(_)
    ));
    assert!(
        alerts(&d)
            .iter()
            .any(|a| a.contains("was canceled") && a.contains("held")),
        "{:?}",
        alerts(&d)
    );
}

#[test]
fn items_over_a_self_limit_wait_for_a_slot_and_the_count_is_per_issue() {
    let s = Scratch::new("true", json!({"steps": {}}));
    s.write_project("scratch", "true", "[self]\nmax_open = 1\n");
    let d = s.start();
    mason_daemon::attention::pause(&d, "", "claims", "test", 3600).unwrap();
    let pid = d.project_id_by_name("scratch").unwrap();
    let owner = d.cfg.owner_names[0].clone();
    let items = vec![
        item(1, "First", &owner, &[]),
        item(2, "Second", &owner, &[]),
        item(3, "Third", &owner, &[]),
        item(4, "Stranger's", "stranger", &[]),
    ];
    let sum = sync_items(&d, pid, "mason", &items);
    assert_eq!(
        (sum.imported, sum.waiting_slot, sum.ignored),
        (1, 2, 1),
        "{sum:?}"
    );
    // Looking again does not count them twice, and the first one is unchanged.
    let sum = sync_items(&d, pid, "mason", &items);
    assert_eq!(
        (sum.imported, sum.waiting_slot, sum.unchanged),
        (0, 2, 1),
        "{sum:?}"
    );
    let a = alerts(&d);
    assert!(
        a.iter()
            .any(|t| t.contains("2 issue(s) wait for a slot (#2, #3)")),
        "{a:?}"
    );
    assert!(task_for(&d, 2).is_none() && task_for(&d, 3).is_none());
}

#[test]
fn the_last_poll_is_recorded_and_a_stop_keeps_the_counts_and_the_reason() {
    let (_s, d, pid) = world();
    let owner = d.cfg.owner_names[0].clone();
    assert!(d.tracker_record(pid).is_none());
    let fake = Fake::default();
    fake.script.lock().unwrap().push(Ok(Listing::Items {
        items: vec![
            item(1, "One", &owner, &[]),
            item(2, "Drive-by", "stranger", &[]),
        ],
        etag: None,
    }));
    poll_once(&d, pid, &fake, &cfg()).unwrap();
    let r = d.tracker_record(pid).unwrap();
    assert!(r.at_ms > 0 && r.error.is_none());
    assert_eq!((r.summary.imported, r.summary.ignored), (1, 1));
    fake.script
        .lock()
        .unwrap()
        .push(Err(ApiError::new(ErrorKind::NotFound, "HTTP 404")));
    poll_once(&d, pid, &fake, &cfg()).unwrap_err();
    let r = d.tracker_record(pid).unwrap();
    assert!(r.error.unwrap().contains("404"));
    assert_eq!(r.summary.imported, 1, "the last full look's counts stay");
    assert!(d.tracker_stopped(pid).is_some());
}
