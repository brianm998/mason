// SPDX-License-Identifier: GPL-3.0-or-later

//! Add Project over HTTP: candidates, plan and apply, owner only, against a temp config dir and
//! temp repositories.

use crate::support;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use support::*;

fn call(d: &Arc<mason_daemon::Daemon>, method: &str, who: &str, path: &str, body: Value) -> (u16, Value) {
    let d = d.clone();
    let (method, who, path) = (method.to_string(), who.to_string(), path.to_string());
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async move {
            use tower::ServiceExt;
            let req = axum::http::Request::builder()
                .method(method.as_str())
                .uri(path)
                .header("content-type", "application/json")
                .header("x-mason-as", who)
                .body(axum::body::Body::from(body.to_string()))
                .unwrap();
            let resp = mason_daemon::http::api_router(d)
                .oneshot(req)
                .await
                .unwrap();
            let status = resp.status().as_u16();
            let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
                .await
                .unwrap();
            (
                status,
                serde_json::from_slice(&bytes).unwrap_or(Value::Null),
            )
        })
}

fn git(dir: &Path, args: &[&str]) -> String {
    let o = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(o.status.success(), "git {args:?}");
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn repo_at(root: &Path, name: &str) -> PathBuf {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("README.md"), "hello\n").unwrap();
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-qm", "init"]);
    dir
}

fn answers() -> Value {
    json!({"tracker": "local", "agent": "claude", "sessions": "1", "workflow": "solo-fast",
        "suites": "skip", "base": "main", "landing": "direct", "alerts": "", "land": "no"})
}

struct T {
    s: Scratch,
    d: Arc<mason_daemon::Daemon>,
    root: tempfile::TempDir,
    owner: String,
}

fn start() -> T {
    let s = Scratch::new("true", happy_script());
    s.write_project("scratch", "true", "");
    let root = tempfile::tempdir().unwrap();
    let roots = vec![root.path().to_path_buf(), root.path().join("missing")];
    let d = s.start_with(|c| c.onboarding.roots = roots);
    let owner = d.cfg.owner_names[0].clone();
    T { s, d, root, owner }
}

#[test]
fn candidates_skip_registered_and_non_repos_and_report_an_unreadable_root() {
    let t = start();
    repo_at(t.root.path(), "beta");
    repo_at(t.root.path(), "alpha");
    std::fs::create_dir_all(t.root.path().join("plain")).unwrap();
    let (code, v) = call(&t.d, "GET", &t.owner, "/onboarding/candidates", json!(null));
    assert_eq!(code, 200, "{v}");
    let names: Vec<&str> = v["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["alpha", "beta"], "{v}");
    assert_eq!(v["candidates"][0]["has_mason"], false);
    assert!(v["candidates"][0]["remote"].is_null());
    assert_eq!(v["errors"].as_array().unwrap().len(), 1, "{v}");
    assert!(v["errors"][0]["path"].as_str().unwrap().ends_with("missing"));

    // Once registered, it is gone from the list.
    let (code, v) = call(
        &t.d,
        "POST",
        &t.owner,
        "/onboarding/apply",
        json!({"path": t.root.path().join("alpha"), "answers": answers()}),
    );
    assert_eq!(code, 200, "{v}");
    let (_, v) = call(&t.d, "GET", &t.owner, "/onboarding/candidates", json!(null));
    assert_eq!(v["candidates"].as_array().unwrap().len(), 1, "{v}");
    assert_eq!(v["candidates"][0]["name"], "beta");
}

#[test]
fn plan_asks_the_first_question_then_lists_the_files_and_adopts_a_mason_folder() {
    let t = start();
    let fresh = repo_at(t.root.path(), "fresh");
    let (code, v) = call(
        &t.d,
        "POST",
        &t.owner,
        "/onboarding/plan",
        json!({"path": fresh, "answers": {}}),
    );
    assert_eq!(code, 200, "{v}");
    assert_eq!(v["kind"], "init");
    assert_eq!(v["next"]["id"], "tracker", "{v}");
    assert_eq!(v["next"]["default"], "local");
    assert_eq!(v["next"]["flag"], "--tracker");

    let (_, v) = call(
        &t.d,
        "POST",
        &t.owner,
        "/onboarding/plan",
        json!({"path": fresh, "answers": answers()}),
    );
    assert!(v["next"].is_null(), "{v}");
    assert!(v["errors"].as_array().unwrap().is_empty(), "{v}");
    let files: Vec<&str> = v["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap())
        .collect();
    assert!(files.contains(&".mason/mason.toml"), "{files:?}");
    assert!(files.contains(&"projects/fresh.toml"), "{files:?}");
    assert!(v["files"][0]["contents"].as_str().is_some());
    assert!(!fresh.join(".mason").exists(), "plan writes nothing");
    assert!(!t.s.config_dir.join("projects/fresh.toml").exists());

    let adopt = repo_at(t.root.path(), "adopt");
    std::fs::create_dir_all(adopt.join(".mason")).unwrap();
    std::fs::write(adopt.join(".mason/mason.toml"), "[step.develop]\ntype = \"agent\"\n").unwrap();
    git(&adopt, &["add", "-A"]);
    git(&adopt, &["commit", "-qm", "mason"]);
    let (_, v) = call(
        &t.d,
        "POST",
        &t.owner,
        "/onboarding/plan",
        json!({"path": adopt, "answers": {}}),
    );
    assert_eq!(v["kind"], "adopt", "{v}");
}

#[test]
fn apply_registers_files_the_proof_task_and_leaves_the_base_alone() {
    let t = start();
    let r = repo_at(t.root.path(), "newone");
    let before = git(&r, &["rev-parse", "main"]);
    let (code, v) = call(
        &t.d,
        "POST",
        &t.owner,
        "/onboarding/apply",
        json!({"path": r, "answers": answers()}),
    );
    assert_eq!(code, 200, "{v}");
    assert_eq!(v["id"], "newone");
    assert_eq!(v["registered"], true, "{v}");
    assert_eq!(v["landed"]["merged"], false);
    assert!(v["proof_task"].is_u64(), "{v}");
    assert!(!v["progress"].as_array().unwrap().is_empty());
    assert_eq!(git(&r, &["rev-parse", "main"]), before);
    assert!(t.s.config_dir.join("projects/newone.toml").is_file());
    let (_, st) = call(&t.d, "GET", &t.owner, "/status", json!(null));
    assert!(
        st["projects"].as_array().unwrap().iter().any(|p| p == "newone"),
        "{st}"
    );

    let r2 = repo_at(t.root.path(), "quiet");
    let mut a = answers();
    a["no_prove"] = json!(true);
    let (code, v) = call(
        &t.d,
        "POST",
        &t.owner,
        "/onboarding/apply",
        json!({"path": r2, "answers": a}),
    );
    assert_eq!(code, 200, "{v}");
    assert_eq!(v["registered"], true);
    assert!(v["proof_task"].is_null(), "{v}");
}

#[test]
fn a_viewer_is_refused_on_all_three() {
    let t = start();
    let r = repo_at(t.root.path(), "x");
    for who in ["viewer:7", ""] {
        let (c, _) = call(&t.d, "GET", who, "/onboarding/candidates", json!(null));
        assert_eq!(c, 403);
        let b = json!({"path": r, "answers": {}});
        let (c, _) = call(&t.d, "POST", who, "/onboarding/plan", b.clone());
        assert_eq!(c, 403);
        let (c, _) = call(&t.d, "POST", who, "/onboarding/apply", b);
        assert_eq!(c, 403);
    }
    assert!(!r.join(".mason").exists());
    assert!(!t.s.config_dir.join("projects/x.toml").exists());
}

#[test]
fn a_path_inside_a_registered_project_is_refused_and_nothing_is_written() {
    let t = start();
    let inside = t.s.repo.join("sub");
    std::fs::create_dir_all(&inside).unwrap();
    let before: Vec<_> = std::fs::read_dir(t.s.config_dir.join("projects"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    let (code, v) = call(
        &t.d,
        "POST",
        &t.owner,
        "/onboarding/apply",
        json!({"path": inside, "answers": answers()}),
    );
    assert_eq!(code, 409, "{v}");
    assert!(v["error"].as_str().unwrap().contains("scratch"), "{v}");
    let after: Vec<_> = std::fs::read_dir(t.s.config_dir.join("projects"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    assert_eq!(before, after);
    let (code, _) = call(
        &t.d,
        "POST",
        &t.owner,
        "/onboarding/apply",
        json!({"path": t.s.repo, "answers": answers()}),
    );
    assert_eq!(code, 409, "the registered path itself");
}
