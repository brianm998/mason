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
        TrackerCaps { conditional_polling: true, comments: true, labels: true, webhooks: true }
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
        self.sent.lock().unwrap().push((item.number, note.into(), false));
        Ok(())
    }
    fn close(&self, item: &Item, note: &str) -> Result<(), ApiError> {
        if *self.fail_mirror.lock().unwrap() {
            return Err(ApiError::new(ErrorKind::Unavailable, "down"));
        }
        self.sent.lock().unwrap().push((item.number, note.into(), true));
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
    let row = d.store.tracker_items("scratch").unwrap().into_iter().find(|r| r.number == n)?;
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
        Item { open: false, ..item(4, "Closed before we met", &owner, &[]) },
    ];
    let sum = sync_items(&d, pid, "mason", &items);
    assert_eq!((sum.imported, sum.ignored), (2, 2), "{sum:?}");
    assert_eq!(task_for(&d, 1).unwrap().trust, TrustSource::Owner);
    assert!(task_for(&d, 2).is_none(), "an unlabeled stranger's issue is not a task");
    assert_eq!(task_for(&d, 3).unwrap().trust, TrustSource::Outside, "labeled, but the words are still untrusted");
    assert!(task_for(&d, 5).is_none(), "a pull request is never a task");
    // The owner labels the drive-by: the next look imports it.
    let sum = sync_items(&d, pid, "mason", &[item(2, "A drive-by", "stranger", &["mason"])]);
    assert_eq!(sum.imported, 1);
    assert_eq!(task_for(&d, 2).unwrap().trust, TrustSource::Outside);
    // Looking again changes nothing.
    assert_eq!(sync_items(&d, pid, "mason", &items[..3]), mason_daemon::tracker::Summary { ignored: 0, ..Default::default() });
}

#[test]
fn the_tracker_owns_title_body_priority_and_closure() {
    let (_s, d, pid) = world();
    let owner = d.cfg.owner_names[0].clone();
    sync_items(&d, pid, "mason", &[item(1, "Old title", &owner, &[])]);
    let t = task_for(&d, 1).unwrap();
    assert_eq!((t.title.as_str(), t.priority), ("Old title", Priority::Normal));
    let mut edited = item(1, "New title", &owner, &[]);
    edited.body = "new body".into();
    edited.priority = "urgent".into();
    let sum = sync_items(&d, pid, "mason", std::slice::from_ref(&edited));
    assert_eq!(sum.updated, 1);
    let t = task_for(&d, 1).unwrap();
    assert_eq!((t.title.as_str(), t.body.as_str(), t.priority), ("New title", "new body", Priority::Urgent));
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
    sync_items(&d, pid, "mason", &[item(1, "Lands", &owner, &[]), item(2, "Fails", &owner, &[])]);
    let (t1, t2) = (task_for(&d, 1).unwrap().id, task_for(&d, 2).unwrap().id);
    let fake = Fake::default();
    // Nothing to say yet.
    mirror_out(&d, pid, &fake);
    assert!(fake.sent.lock().unwrap().is_empty());
    d.emit(EventKind::Canceled { task: t2, delete_branch: false }).unwrap();
    // Make t1 land and t2 fail through the real events: put both in a step first.
    let def = d.current_def(pid);
    d.emit(EventKind::Admitted {
        task: t1,
        entry: Some(Entry { def: def.hash.clone(), step: "land".into(), state: State::Landing, route: Route::Explore }),
    })
    .unwrap();
    d.emit(EventKind::StepSucceeded { task: t1, next: Some(Next { step: String::new(), state: State::Landed }) }).ok();
    // The tracker is down: mirroring fails, and nothing else is affected.
    *fake.fail_mirror.lock().unwrap() = true;
    mirror_out(&d, pid, &fake);
    assert!(fake.sent.lock().unwrap().is_empty());
    assert_eq!(d.store.tracker_items("scratch").unwrap().iter().find(|r| r.number == 1).unwrap().mirrored, "", "not remembered, so it is tried again");
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
    fake.script.lock().unwrap().push(Ok(Listing::Items { items: vec![item(1, "One", &owner, &[])], etag: Some("\"e1\"".into()) }));
    let sum = poll_once(&d, pid, &fake, &cfg()).unwrap();
    assert_eq!(sum.imported, 1);
    // The next look carries the ETag and the answer is "unchanged".
    let sum = poll_once(&d, pid, &fake, &cfg()).unwrap();
    assert_eq!(sum, Default::default());
    assert_eq!(fake.seen_etags.lock().unwrap().clone(), vec![None, Some("\"e1\"".to_string())]);
    // The repository is gone: a permanent error. The poll stops and the owner is told at once.
    fake.script.lock().unwrap().push(Err(ApiError::new(ErrorKind::NotFound, "HTTP 404")));
    let e = poll_once(&d, pid, &fake, &cfg()).unwrap_err();
    assert!(e.contains("stopped"), "{e}");
    assert!(d.tracker_stopped(pid).is_some());
    assert!(
        mason_daemon::attention::alerts_now(&d).iter().any(|(_, k, t)| k == "tracker" && t.contains("stopped")),
        "attention is raised at once"
    );
    // A refresh asks once more; found again, the poll runs and the alert clears.
    d.tracker_refresh(pid);
    assert!(d.tracker_stopped(pid).is_none());
    poll_once(&d, pid, &fake, &cfg()).unwrap();
    assert!(!mason_daemon::attention::alerts_now(&d).iter().any(|(_, k, _)| k == "tracker"));
}

#[test]
fn an_outage_is_retried_with_backoff_not_stopped() {
    let (_s, d, pid) = world();
    let fake = Fake::default();
    fake.script.lock().unwrap().push(Err(ApiError::new(ErrorKind::Unavailable, "HTTP 503")));
    assert!(poll_once(&d, pid, &fake, &cfg()).is_err());
    assert!(d.tracker_stopped(pid).is_none(), "an outage is not permanent");
    assert!(mason_daemon::attention::alerts_now(&d).iter().any(|(_, k, t)| k == "tracker" && t.contains("trying again")));
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
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async move {
            use tower::ServiceExt;
            let req = axum::http::Request::builder()
                .method("POST")
                .uri("/hooks/github")
                .header("x-hub-signature-256", sig)
                .body(axum::body::Body::from(body))
                .unwrap();
            mason_daemon::http::api_router(d).oneshot(req).await.unwrap().status().as_u16()
        })
    };
    assert_eq!(post(&sig, &body), 200);
    assert_eq!(post("sha256=00", &body), 401, "a bad signature is refused");
    assert_eq!(post(&sig, &body.replace("labeled", "closed")), 401, "a changed body is refused");
    let other = json!({"repository": {"full_name": "someone/else"}}).to_string();
    let sig2 = format!("sha256={}", hex(&hmac_sha256(b"s3cret", other.as_bytes())));
    assert_eq!(post(&sig2, &other), 401, "a repository the project does not track is refused");
}
