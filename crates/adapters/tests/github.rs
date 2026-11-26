// SPDX-License-Identifier: GPL-3.0-or-later

//! The GitHub adapters against recorded responses (shaped as GitHub's REST API
//! documents them; no network, no real token), and the fake host and command CI
//! against the same conformance kits.

use mason_adapter_api::http::Recorded;
use mason_adapter_api::kit::*;
use mason_adapter_api::service::*;
use mason_adapters::fakehost::{CommandCi, FakeHost};
use mason_adapters::github::{workflow_problems, GitHub, TokenRef};
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn fixture(name: &str) -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/github")
            .join(name),
    )
    .unwrap()
}

struct Gh {
    gh: GitHub,
    http: Arc<Recorded>,
    _d: tempfile::TempDir,
}

fn gh(script: &str) -> Gh {
    let d = tempfile::tempdir().unwrap();
    let tok = d.path().join("token");
    std::fs::write(&tok, "ghp_recorded_test_token\n").unwrap();
    let http = Arc::new(Recorded::from_json(&fixture(script)).unwrap());
    let gh = GitHub::new(
        http.clone(),
        "https://api.github.com",
        "o/r",
        TokenRef::File(tok),
    );
    Gh { gh, http, _d: d }
}

#[test]
fn the_host_passes_its_conformance_kit_on_recorded_responses() {
    let g = gh("host_kit.json");
    let problems = host_kit(
        &g.gh,
        &HostScenario {
            base: "develop".into(),
            good_head: "mason/pr/x-b1".into(),
            missing_head: "nope".into(),
            reviewer: None,
        },
    );
    assert!(problems.is_empty(), "{problems:?}");
    assert!(
        g.http.unused().is_empty(),
        "the script expected more: {:?}",
        g.http.unused()
    );
}

#[test]
fn the_tracker_passes_its_conformance_kit_and_sees_prs_as_prs() {
    let g = gh("tracker_kit.json");
    let problems = tracker_kit(&g.gh);
    assert!(problems.is_empty(), "{problems:?}");
    let g = gh("tracker_kit.json");
    let Listing::Items { items, etag } = g.gh.list(None).unwrap() else {
        panic!()
    };
    assert_eq!(etag.as_deref(), Some("\"v1\""));
    assert_eq!(items.len(), 4);
    assert_eq!(items[0].priority, "high");
    assert_eq!(items[3].priority, "urgent");
    assert!(items[2].is_pr && !items[1].is_pr);
    assert!(!items[3].open);
    // The conditional request carried the ETag.
    g.gh.list(Some("\"v1\"")).unwrap();
    let reqs = g.http.requests();
    assert!(reqs[1]
        .headers
        .iter()
        .any(|(k, v)| k == "If-None-Match" && v == "\"v1\""));
    // Native "blocked by": the other repository's issue is outside this tracker.
    let blockers = g.gh.blocked_by(3).unwrap();
    assert_eq!(
        blockers,
        vec![
            Blocker {
                number: 2,
                open: false
            },
            Blocker {
                number: 9,
                open: true
            }
        ]
    );
}

#[test]
fn the_build_system_passes_its_conformance_kit() {
    let mut g = gh("build_kit.json");
    g.gh.workflow = Some("ci.yml".into());
    assert!(build_kit(&g.gh, "mason/ci/x-b1", "ccc333").is_empty());
    assert!(g.http.unused().is_empty());
    // The token went in a header, never in a URL.
    for r in g.http.requests() {
        assert!(!r.url.contains("ghp_"), "{}", r.url);
        assert!(r
            .headers
            .iter()
            .any(|(k, v)| k == "Authorization" && v == "Bearer ghp_recorded_test_token"));
    }
}

#[test]
fn ci_red_names_the_failed_jobs_and_pending_is_pending() {
    let g = gh("ci_red.json");
    let CiState::Red {
        failed, summary, ..
    } = g.gh.status("ddd444").unwrap()
    else {
        panic!()
    };
    assert_eq!(failed, ["integration"]);
    assert!(summary.contains("ci"));
    let g = gh("ci_pending.json");
    assert_eq!(
        g.gh.status("eee555").unwrap(),
        CiState::Pending,
        "no run yet is pending, not green"
    );
    assert_eq!(
        g.gh.status("eee555").unwrap(),
        CiState::Pending,
        "a run in progress is pending"
    );
}

#[test]
fn a_failed_checks_details_url_is_carried_into_its_job_failure() {
    let g = gh("pr_red_urls.json");
    let st = g.gh.pr_status(9).unwrap();
    assert_eq!(
        st.jobs.len(),
        1,
        "only the completed failure is a job: {:?}",
        st.jobs
    );
    assert_eq!(st.jobs[0].job, "build");
    assert_eq!(
        st.jobs[0].url.as_deref(),
        Some("https://github.com/o/r/actions/runs/9/job/not-a-number")
    );
    // A check run with no `details_url` has no URL.
    let g = gh("pr_red.json");
    assert_eq!(g.gh.pr_status(9).unwrap().jobs[0].url, None);
}

#[test]
fn a_pr_waits_for_a_required_review_and_a_requested_change_is_not_a_review() {
    let g = gh("pr_review_required.json");
    let st = g.gh.pr_status(9).unwrap();
    assert_eq!(
        (st.checks, st.review, st.state),
        (Checks::Green, Review::Required, PrState::Open)
    );
    let g = gh("pr_changes_requested.json");
    assert_eq!(g.gh.pr_status(9).unwrap().review, Review::ChangesRequested);
    let g = gh("pr_red.json");
    let st = g.gh.pr_status(9).unwrap();
    assert_eq!(
        (st.checks, st.failed),
        (Checks::Red, vec!["build".to_string()])
    );
}

#[test]
fn the_merge_queue_is_a_graphql_mutation_and_its_refusal_is_read_from_the_body() {
    let g = gh("enqueue.json");
    g.gh.enqueue(7).unwrap();
    let e = g.gh.enqueue(8).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Invalid);
    assert!(e.detail.contains("merge queue"));
}

fn poll_once(script: &str, p: &mut Poller, now: u64) -> (PollEvent, usize) {
    let g = gh(script);
    let o = match g.gh.list(p.etag.as_deref()) {
        Ok(Listing::NotModified) => Observed::Unchanged,
        Ok(Listing::Items { etag, .. }) => Observed::Changed { etag },
        Err(e) => Observed::Failed(e),
    };
    (p.observe(now, o), g.http.requests().len())
}

#[test]
fn a_404_stops_polling_at_once_and_nothing_more_is_asked() {
    let mut p = Poller::new(30);
    let (e, n) = poll_once("err_404.json", &mut p, 0);
    assert!(
        matches!(e, PollEvent::Stopped { ref why } if why.contains("404")),
        "{e:?}"
    );
    assert_eq!(n, 1);
    for t in [1, 60, 3600, 86_400] {
        assert!(!p.due(t), "a stopped poll must not ask again (t={t})");
    }
}

#[test]
fn a_bad_credential_is_permanent_too() {
    let mut p = Poller::new(30);
    let (e, _) = poll_once("err_401.json", &mut p, 0);
    assert!(matches!(e, PollEvent::Stopped { .. }));
}

#[test]
fn a_rate_limit_waits_as_told_and_an_outage_backs_off() {
    let mut p = Poller::new(30);
    let (e, _) = poll_once("err_ratelimit.json", &mut p, 1000);
    assert_eq!(
        e,
        PollEvent::Backoff {
            until: 1120,
            why: "RateLimited { retry_after_s: Some(120) }: HTTP 403: API rate limit exceeded"
                .into()
        }
    );
    let mut p = Poller::new(30);
    let (e, _) = poll_once("err_503.json", &mut p, 0);
    assert!(matches!(e, PollEvent::Backoff { until: 60, .. }), "{e:?}");
    let mut p = Poller::new(30);
    let (e, _) = poll_once("err_outage.json", &mut p, 0);
    assert!(matches!(e, PollEvent::Backoff { .. }), "{e:?}");
    assert!(p.due(10_000), "an outage is retried");
}

#[test]
fn a_token_is_a_reference_never_a_value() {
    assert!(TokenRef::parse("ghp_abcdef").is_err());
    assert!(TokenRef::parse("env:").is_err());
    assert_eq!(
        TokenRef::parse("env:GH_TOKEN").unwrap(),
        TokenRef::Env("GH_TOKEN".into())
    );
    assert_eq!(
        TokenRef::parse("file:/p/t").unwrap(),
        TokenRef::File("/p/t".into())
    );
    // A missing token is a permanent (Auth) error, found before any request.
    let http = Arc::new(Recorded::from_json("[]").unwrap());
    let g = GitHub::new(
        http.clone(),
        "https://api.github.com",
        "o/r",
        TokenRef::File("/nonexistent/token".into()),
    );
    let e = g.list(None).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Auth);
    assert!(http.requests().is_empty());
}

#[test]
fn doctor_finds_a_workflow_that_can_run_on_a_candidate_ref() {
    let d = tempfile::tempdir().unwrap();
    assert!(workflow_problems(d.path())[0].0.contains("no workflow"));
    let wf = d.path().join(".github/workflows");
    std::fs::create_dir_all(&wf).unwrap();
    std::fs::write(
        wf.join("ci.yml"),
        "on:\n  push:\n    branches: [main]\njobs: {}\n",
    )
    .unwrap();
    assert!(workflow_problems(d.path())[0].0.contains("mason/ci/**"));
    std::fs::write(
        wf.join("ci.yml"),
        "on:\n  # push: mason/ci/**\n  push:\n    branches: [main]\n",
    )
    .unwrap();
    assert!(
        !workflow_problems(d.path()).is_empty(),
        "a comment does not count"
    );
    std::fs::write(
        wf.join("ci.yml"),
        "on:\n  push:\n    branches: [\"mason/ci/**\"]\n",
    )
    .unwrap();
    assert!(workflow_problems(d.path()).is_empty());
    std::fs::write(wf.join("ci.yml"), "on:\n  workflow_dispatch:\n").unwrap();
    assert!(workflow_problems(d.path()).is_empty());
}

// ---- the fake host and the command CI satisfy the same kits ----------------------------------

fn sh(cwd: &Path, args: &[&str]) -> String {
    mason_landing_git(cwd, args)
}

fn mason_landing_git(cwd: &Path, args: &[&str]) -> String {
    let o = std::process::Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    assert!(
        o.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

/// `canonicalize` returns a verbatim `\\?\C:\...` path on Windows. Git reads that as scp-style
/// `host:path` and fails with "hostname contains invalid characters"; plain `C:\...` is local.
/// Strip the prefix when a drive letter follows it. Unchanged elsewhere.
fn plain_canonical(p: &Path) -> PathBuf {
    let c = std::fs::canonicalize(p).unwrap();
    let s = c.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        let b = rest.as_bytes();
        if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
            return PathBuf::from(rest);
        }
    }
    c
}

fn fake_world() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let root = plain_canonical(d.path());
    let bare = root.join("host.git");
    std::fs::create_dir_all(&bare).unwrap();
    sh(&bare, &["init", "-q", "--bare", "-b", "develop"]);
    let w = root.join("w");
    sh(&root, &["clone", "-q", bare.to_str().unwrap(), "w"]);
    sh(&w, &["checkout", "-q", "-b", "develop"]);
    std::fs::write(w.join("a"), "1").unwrap();
    sh(&w, &["add", "-A"]);
    sh(&w, &["commit", "-q", "-m", "init"]);
    sh(&w, &["push", "-q", "origin", "develop"]);
    sh(&w, &["checkout", "-q", "-b", "feature"]);
    std::fs::write(w.join("b"), "2").unwrap();
    sh(&w, &["add", "-A"]);
    sh(&w, &["commit", "-q", "-m", "feature"]);
    sh(&w, &["push", "-q", "origin", "feature"]);
    (d, bare, root)
}

#[test]
fn the_fake_host_passes_the_host_kit() {
    for require_approval in [false, true] {
        let (_d, bare, root) = fake_world();
        let host = FakeHost {
            dir: root.join("state"),
            remote: bare,
            checks_cmd: Some("test -f b".into()),
            require_approval,
            merge_queue: false,
        };
        let dir = root.join("state");
        let problems = host_kit(
            &host,
            &HostScenario {
                base: "develop".into(),
                good_head: "feature".into(),
                missing_head: "nope".into(),
                // The kit expects a reviewed PR: the reviewer approves it once it exists.
                reviewer: require_approval.then(|| {
                    Box::new(move |n| FakeHost::approve(&dir, n)) as Box<dyn Fn(u64) + Send + Sync>
                }),
            },
        );
        assert!(
            problems.is_empty(),
            "approval={require_approval}: {problems:?}"
        );
    }
}

#[test]
fn the_command_ci_passes_the_build_kit() {
    let d = tempfile::tempdir().unwrap();
    let ci = CommandCi {
        cancel: None,
        trigger: "echo run-$MASON_SHA".into(),
        status: "echo green".into(),
        cwd: d.path().into(),
    };
    assert!(build_kit(&ci, "mason/ci/x", "abc").is_empty());
    assert_eq!(ci.trigger("r", "abc").unwrap(), "run-abc");
    let red = CommandCi {
        cancel: None,
        trigger: "true".into(),
        status: "printf 'red\\nunit, e2e\\n'".into(),
        cwd: d.path().into(),
    };
    assert_eq!(red.trigger("r", "abc").unwrap(), "abc");
    assert_eq!(
        red.status("abc").unwrap(),
        CiState::Red {
            failed: vec!["unit".into(), "e2e".into()],
            summary: "unit, e2e failed".into(),
            jobs: vec![],
        }
    );
    let pend = CommandCi {
        cancel: None,
        trigger: "true".into(),
        status: "echo".into(),
        cwd: d.path().into(),
    };
    assert_eq!(pend.status("x").unwrap(), CiState::Pending);
}

#[test]
fn the_fake_host_merges_what_is_queued_when_it_is_green_and_drops_what_is_red() {
    let (_d, bare, root) = fake_world();
    let host = FakeHost {
        dir: root.join("state"),
        remote: bare.clone(),
        checks_cmd: Some("test -f b".into()),
        require_approval: false,
        merge_queue: true,
    };
    let pr = host.open_pr("develop", "feature", "t", "b").unwrap();
    host.enqueue(pr.number).unwrap();
    assert_eq!(host.pr_status(pr.number).unwrap().state, PrState::Merged);
    assert_eq!(sh(&bare, &["show", "develop:b"]), "2");
    // A red PR is dropped from the queue rather than merged.
    let w = root.join("w");
    sh(&w, &["checkout", "-q", "-b", "broken", "develop"]);
    sh(&w, &["fetch", "-q", "origin"]);
    sh(&w, &["reset", "-q", "--hard", "origin/develop"]);
    std::fs::remove_file(w.join("b")).unwrap();
    std::fs::write(w.join("c"), "3").unwrap();
    sh(&w, &["add", "-A"]);
    sh(&w, &["commit", "-q", "-m", "broken"]);
    sh(&w, &["push", "-q", "origin", "broken"]);
    let pr2 = host.open_pr("develop", "broken", "t", "b").unwrap();
    host.enqueue(pr2.number).unwrap();
    let st = host.pr_status(pr2.number).unwrap();
    assert_eq!((st.state, st.checks), (PrState::Closed, Checks::Red));
}

#[test]
fn base_branch_runs_jobs_and_the_log_tail_come_from_recorded_responses() {
    let g = gh("base_runs_green.json");
    let runs = g.gh.base_runs("develop").unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(
        (runs[0].workflow.as_str(), runs[0].conclusion.as_deref()),
        ("CI", Some("success"))
    );
    // 2026-11-11T10:00:00Z
    assert_eq!(runs[0].created_ms, 1_794_391_200_000);
    let jobs = g.gh.run_jobs(11).unwrap();
    assert_eq!(jobs[1].conclusion.as_deref(), Some("skipped"));
    assert!(g.http.unused().is_empty());

    let g = gh("base_runs_red.json");
    let runs = g.gh.base_runs("develop").unwrap();
    assert_eq!(runs[0].conclusion.as_deref(), Some("cancelled"));
    assert_eq!(runs[1].id, 13);
    assert_eq!(g.gh.run_jobs(13).unwrap()[0].name, "pr-gate");
    // The redirect is followed, and the signed address gets no token.
    assert_eq!(
        g.gh.job_log_tail(131, 2).as_deref(),
        Some("three\nFAILED test_x")
    );
    let reqs = g.http.requests();
    assert!(!reqs[3].headers.iter().any(|(k, _)| k == "Authorization"));
    assert!(g.http.unused().is_empty());
}

#[test]
fn iso_times_read_as_milliseconds() {
    use mason_adapters::github::iso_ms;
    assert_eq!(iso_ms("1970-01-01T00:00:01Z"), 1000);
    assert_eq!(iso_ms("2000-03-01T00:00:00Z"), 951_868_800_000);
    assert_eq!(iso_ms("garbage"), 0);
}

// ---- the infrastructure judge (stage 153) ------------------------------------------------------

fn judged_gh(entries: serde_json::Value, judge: bool) -> Gh {
    let d = tempfile::tempdir().unwrap();
    let tok = d.path().join("token");
    std::fs::write(&tok, "ghp_recorded_test_token\n").unwrap();
    let http = Arc::new(Recorded::from_json(&entries.to_string()).unwrap());
    let mut gh = GitHub::new(
        http.clone(),
        "https://api.github.com",
        "o/r",
        TokenRef::File(tok),
    );
    if judge {
        gh.infra = Some(vec![]);
    }
    Gh { gh, http, _d: d }
}

fn ok(body: serde_json::Value) -> serde_json::Value {
    serde_json::json!({"status": 200, "headers": {}, "body": body})
}

/// A completed failed run `pr-gate` with the given jobs `(id, name, conclusion)`, and logs
/// `(job id, answer)`.
fn red_script(
    jobs: &[(u64, &str, &str)],
    logs: Vec<(u64, serde_json::Value)>,
) -> serde_json::Value {
    use serde_json::json;
    let mut v = vec![
        json!({"method": "GET", "url": "/actions/runs?head_sha=sha1", "response": ok(json!({
            "workflow_runs": [{"id": 5, "name": "pr-gate", "status": "completed", "conclusion": "failure"}]}))}),
        json!({"method": "GET", "url": "/actions/runs/5/jobs", "response": ok(json!({
            "jobs": jobs.iter().map(|(id, n, c)| json!({
                "id": id, "name": n, "conclusion": c,
                "steps": [{"name": "build", "conclusion": if *c == "cancelled" { json!(null) } else { json!("failure") }}]
            })).collect::<Vec<_>>()}))}),
    ];
    for (id, resp) in logs {
        v.push(
            json!({"method": "GET", "url": format!("/actions/jobs/{id}/logs"), "response": resp}),
        );
    }
    serde_json::Value::Array(v)
}

fn redirect(to: &str) -> serde_json::Value {
    serde_json::json!({"status": 302, "headers": {"Location": to}, "body": ""})
}

fn jobs_of(s: CiState) -> Vec<JobFailure> {
    let CiState::Red { jobs, .. } = s else {
        panic!("{s:?}")
    };
    jobs
}

const LOG_URL: &str = "https://blob.example/logs/1";

fn log_entry(text: &str) -> serde_json::Value {
    serde_json::json!({"method": "GET", "url": LOG_URL,
        "response": {"status": 200, "headers": {}, "body": text}})
}

#[test]
fn a_timed_out_job_is_infrastructure_without_fetching_its_log() {
    let g = judged_gh(red_script(&[(11, "macOS", "timed_out")], vec![]), true);
    let jobs = jobs_of(g.gh.status("sha1").unwrap());
    assert_eq!(jobs.len(), 1);
    assert_eq!(
        (jobs[0].workflow.as_str(), jobs[0].job.as_str()),
        ("pr-gate", "macOS")
    );
    assert!(jobs[0].infra.is_some());
    assert!(g.http.requests().iter().all(|r| !r.url.contains("/logs")));
}

#[test]
fn a_failure_whose_log_says_the_runner_was_shut_down_is_infrastructure() {
    for text in [
        "##[error]The runner has received a shutdown signal.",
        "ping: Temporary failure resolving 'github.com'",
    ] {
        let mut script = red_script(&[(11, "macOS", "failure")], vec![(11, redirect(LOG_URL))]);
        script.as_array_mut().unwrap().push(log_entry(text));
        let g = judged_gh(script, true);
        let jobs = jobs_of(g.gh.status("sha1").unwrap());
        assert!(jobs[0].infra.is_some(), "{text}");
        assert!(g.http.unused().is_empty(), "{:?}", g.http.unused());
        // The redirect target is not GitHub's: it never sees the token.
        let last = g.http.requests().pop().unwrap();
        assert_eq!(last.url, LOG_URL);
        assert!(last
            .headers
            .iter()
            .all(|(k, _)| !k.eq_ignore_ascii_case("authorization")));
    }
}

#[test]
fn a_compile_error_and_a_failed_xctest_assertion_are_real() {
    for text in [
        "error[E0308]: mismatched types",
        "XCTAssertEqual failed: (\"1\") is not equal to (\"2\")\nTest Case '-[T testX]' failed",
    ] {
        let mut script = red_script(&[(11, "Linux", "failure")], vec![(11, redirect(LOG_URL))]);
        script.as_array_mut().unwrap().push(log_entry(text));
        let g = judged_gh(script, true);
        let jobs = jobs_of(g.gh.status("sha1").unwrap());
        assert_eq!(jobs[0].infra, None, "{text}");
    }
}

#[test]
fn a_mix_names_each_job_and_only_the_failure_job_has_its_log_fetched() {
    // A direct 200 answer is a log too.
    let script = red_script(
        &[(11, "macOS", "timed_out"), (12, "Linux", "failure")],
        vec![(12, ok(serde_json::json!("error: cannot find 'x' in scope")))],
    );
    let g = judged_gh(script, true);
    let jobs = jobs_of(g.gh.status("sha1").unwrap());
    assert!(jobs[0].infra.is_some());
    assert_eq!(jobs[1].infra, None);
    let logs: Vec<_> = g
        .http
        .requests()
        .into_iter()
        .filter(|r| r.url.contains("/logs"))
        .collect();
    assert_eq!(logs.len(), 1);
    assert!(logs[0].url.ends_with("/actions/jobs/12/logs"));
}

#[test]
fn a_job_cancelled_with_no_step_run_never_got_a_runner() {
    let g = judged_gh(red_script(&[(11, "macOS", "cancelled")], vec![]), true);
    let jobs = jobs_of(g.gh.status("sha1").unwrap());
    assert_eq!(jobs[0].infra.as_deref(), Some("never got a runner"));
}

#[test]
fn a_log_that_cannot_be_fetched_counts_as_a_real_failure() {
    let g = judged_gh(
        red_script(
            &[(11, "macOS", "failure")],
            vec![(
                11,
                serde_json::json!({"status": 404, "headers": {}, "body": {"message": "gone"}}),
            )],
        ),
        true,
    );
    assert_eq!(jobs_of(g.gh.status("sha1").unwrap())[0].infra, None);
}

#[test]
fn with_the_judge_off_nothing_is_judged_but_the_log_is_kept_for_the_task() {
    let g = judged_gh(
        red_script(
            &[(11, "macOS", "failure")],
            vec![(
                11,
                serde_json::json!({"status": 200, "headers": {}, "body": "step\nFAILED t.py::test_x - assert 0"}),
            )],
        ),
        false,
    );
    let jobs = jobs_of(g.gh.status("sha1").unwrap());
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].infra, None, "nothing is judged with the judge off");
    assert!(jobs[0]
        .log
        .as_deref()
        .unwrap()
        .contains("FAILED t.py::test_x"));
}

// ---- scheduled CI runs (stage 175) -------------------------------------------------------------

#[test]
fn a_workflow_is_dispatched_with_inputs_and_its_runs_and_rerun_come_from_recorded_responses() {
    use serde_json::json;
    let entries = json!([
        {"method": "POST", "url": "/actions/workflows/gherkin.yml/dispatches",
         "response": {"status": 204, "headers": {}, "body": ""}},
        {"method": "POST", "url": "/actions/workflows/gherkin.yml/dispatches",
         "response": {"status": 204, "headers": {}, "body": ""}},
        {"method": "GET", "url": "/actions/workflows/gherkin.yml/runs?branch=develop&exclude_pull_requests=true&per_page=100",
         "response": ok(json!({"workflow_runs": [
            {"id": 8, "name": "Gherkin", "head_sha": "aaa", "html_url": "https://github.com/o/r/actions/runs/8",
             "created_at": "2026-11-11T10:00:00Z", "status": "completed", "conclusion": "failure",
             "event": "workflow_dispatch", "display_title": "Gherkin: full", "run_attempt": 2},
            {"id": 9, "name": "Gherkin", "head_sha": "bbb", "created_at": "2026-11-11T11:00:00Z",
             "status": "in_progress", "conclusion": null, "event": "push", "display_title": "x"}
         ]}))},
        {"method": "POST", "url": "/actions/runs/8/rerun-failed-jobs",
         "response": {"status": 201, "headers": {}, "body": "{}"}},
        {"method": "POST", "url": "/actions/runs/9/rerun-failed-jobs",
         "response": {"status": 403, "headers": {}, "body": "{\"message\": \"Resource not accessible\"}"}},
    ]);
    let g = judged_gh(entries, false);
    g.gh.dispatch_workflow(
        "gherkin.yml",
        "develop",
        &[("features".to_string(), String::new())],
    )
    .unwrap();
    g.gh.dispatch_workflow("gherkin.yml", "develop", &[])
        .unwrap();
    let runs = g.gh.workflow_file_runs("gherkin.yml", "develop").unwrap();
    // Newest first, finished or not, with the event, run-name and attempt.
    assert_eq!(runs.iter().map(|r| r.id).collect::<Vec<_>>(), [9, 8]);
    assert_eq!(
        (
            runs[0].status.as_str(),
            runs[0].conclusion.as_deref(),
            runs[0].attempt
        ),
        ("in_progress", None, 1)
    );
    assert_eq!(
        (
            runs[1].event.as_str(),
            runs[1].title.as_str(),
            runs[1].attempt
        ),
        ("workflow_dispatch", "Gherkin: full", 2)
    );
    assert_eq!(runs[1].created_ms, 1_794_391_200_000);
    g.gh.rerun_failed(8).unwrap();
    assert!(g.gh.rerun_failed(9).is_err());
    let reqs = g.http.requests();
    let body = |i: usize| -> serde_json::Value {
        serde_json::from_str(reqs[i].body.as_deref().unwrap()).unwrap()
    };
    assert_eq!(
        body(0),
        json!({"ref": "develop", "inputs": {"features": ""}})
    );
    assert_eq!(
        body(1),
        json!({"ref": "develop"}),
        "no inputs, no inputs key"
    );
    assert!(g.http.unused().is_empty());
}

// ---- CI progress: jobs told apart by state (stage 205) ------------------------------------------

#[test]
fn a_pending_ci_counts_its_jobs_by_state_one_entry_per_workflow() {
    use serde_json::json;
    let g = judged_gh(
        json!([
            {"method": "GET", "url": "/actions/runs?head_sha=sha1", "response": ok(json!({"workflow_runs": [
                {"id": 5, "name": "pr-gate", "status": "in_progress", "conclusion": null},
                {"id": 6, "name": "macOS", "status": "queued", "conclusion": null}]}))},
            {"method": "GET", "url": "/actions/runs/5/jobs?filter=latest&per_page=100", "response": ok(json!({"jobs": [
                {"id": 51, "name": "lint", "status": "completed", "conclusion": "success"},
                {"id": 52, "name": "docs", "status": "completed", "conclusion": "skipped"},
                {"id": 53, "name": "unit", "status": "completed", "conclusion": "cancelled"},
                {"id": 54, "name": "build", "status": "in_progress", "conclusion": null},
                {"id": 55, "name": "e2e", "status": "queued", "conclusion": null}]}))},
            {"method": "GET", "url": "/actions/runs/6/jobs?filter=latest&per_page=100", "response": ok(json!({"jobs": []}))},
        ]),
        true, // judged: a cancelled job with no step run is infrastructure, so the run stays pending
    );
    let (state, detail) = g.gh.status_detail("sha1").unwrap();
    assert_eq!(state, CiState::Pending);
    assert_eq!(detail.len(), 2);
    assert_eq!(detail[0].workflow, "pr-gate");
    assert_eq!(
        detail[0].jobs,
        CiJobCounts {
            total: 5,
            queued: 1,
            running: 1,
            succeeded: 2,
            failed: 1,
            running_names: vec!["build".into()],
        }
    );
    assert_eq!(detail[1].workflow, "macOS");
    assert_eq!(
        detail[1].jobs,
        CiJobCounts::default(),
        "no jobs yet is zeros"
    );
    assert!(g.http.unused().is_empty());
}

#[test]
fn progress_right_after_a_pending_status_reads_nothing_again() {
    use serde_json::json;
    // One list of runs and one read of each unfinished run's jobs; the finished run's jobs are
    // read once, by `progress`. A second `progress` has nothing to reuse and reads all again.
    let g = judged_gh(
        json!([
            {"method": "GET", "url": "/actions/runs?head_sha=sha1", "response": ok(json!({"workflow_runs": [
                {"id": 5, "name": "pr-gate", "status": "in_progress", "conclusion": null},
                {"id": 7, "name": "docs", "status": "completed", "conclusion": "success"}]}))},
            {"method": "GET", "url": "/actions/runs/5/jobs?filter=latest&per_page=100", "response": ok(json!({"jobs": [
                {"id": 54, "name": "build", "status": "in_progress", "conclusion": null}]}))},
            {"method": "GET", "url": "/actions/runs/7/jobs?filter=latest&per_page=100", "response": ok(json!({"jobs": [
                {"id": 71, "name": "lint", "status": "completed", "conclusion": "success"}]}))},
        ]),
        true,
    );
    let (state, _) = g.gh.status_detail("sha1").unwrap();
    assert_eq!(state, CiState::Pending);
    let p = g.gh.progress("sha1").unwrap();
    assert_eq!(p.workflows.len(), 2);
    assert_eq!(p.workflows[0].running, 1);
    assert_eq!(p.workflows[1].passed, 1);
    assert!(g.http.unused().is_empty());
    assert_eq!(g.http.requests().len(), 3, "each request made once");
    // Nothing is left over to reuse: a progress of another commit, or a second one, asks again.
    assert!(
        g.gh.progress("sha1").is_err(),
        "the script has no more answers"
    );
}

#[test]
fn check_runs_give_the_same_breakdown() {
    use serde_json::json;
    let list = json!([
        {"name": "a", "status": "completed", "conclusion": "success"},
        {"name": "b", "status": "completed", "conclusion": "cancelled"},
        {"name": "c", "status": "in_progress", "conclusion": null},
        {"name": "d", "status": "waiting", "conclusion": null}]);
    let mut j = CiJobCounts::default();
    for r in list.as_array().unwrap() {
        j.add(
            r["name"].as_str().unwrap(),
            r["status"].as_str().unwrap(),
            r["conclusion"].as_str(),
        );
    }
    assert_eq!(
        (j.total, j.queued, j.running, j.succeeded, j.failed),
        (4, 1, 1, 1, 1)
    );
    assert_eq!(j.running_names, ["c"]);
}

// ---- red as soon as a required job fails (cancel the rest) ------------------------------------

fn live_script(branch: &str, jobs: serde_json::Value) -> serde_json::Value {
    use serde_json::json;
    json!([
        {"method": "GET", "url": "/actions/runs?head_sha=sha1", "response": ok(json!({
            "workflow_runs": [{"id": 5, "name": "pr-gate", "status": "in_progress", "conclusion": null,
                               "head_branch": branch, "event": "workflow_dispatch"}]}))},
        {"method": "GET", "url": "/actions/runs/5/jobs", "response": ok(json!({"jobs": jobs}))},
        {"method": "POST", "url": "/actions/runs/5/cancel", "response": {"status": 202, "headers": {}, "body": "{}"}},
    ])
}

fn cancels(g: &Gh) -> usize {
    g.http
        .requests()
        .iter()
        .filter(|r| r.method == "POST" && r.url.ends_with("/actions/runs/5/cancel"))
        .count()
}

#[test]
fn a_failed_job_makes_a_running_run_red_at_once_and_the_run_is_cancelled() {
    use serde_json::json;
    let g = judged_gh(
        live_script(
            "mason/ci/star-b1-1",
            json!([
                {"id": 1, "name": "static checks", "conclusion": "failure",
                 "steps": [{"name": "ruff", "conclusion": "failure"}]},
                {"id": 2, "name": "macOS", "conclusion": null, "steps": []},
            ]),
        ),
        false,
    );
    let CiState::Red {
        failed, summary, ..
    } = g.gh.status("sha1").unwrap()
    else {
        panic!("expected red")
    };
    assert_eq!(failed, vec!["static checks".to_string()]);
    assert!(summary.contains("cancelled by MASON after static checks failed"));
    assert_eq!(cancels(&g), 1);
}

#[test]
fn a_cancelled_job_with_no_step_run_is_not_red_while_the_run_is_going() {
    use serde_json::json;
    let g = judged_gh(
        live_script(
            "mason/ci/star-b1-1",
            json!([{"id": 2, "name": "macOS", "conclusion": "cancelled", "steps": []}]),
        ),
        true,
    );
    assert_eq!(g.gh.status("sha1").unwrap(), CiState::Pending);
    assert_eq!(cancels(&g), 0);
}

#[test]
fn a_run_off_a_mason_ref_is_called_red_but_never_cancelled() {
    use serde_json::json;
    let g = judged_gh(
        live_script(
            "develop",
            json!([{"id": 1, "name": "build", "conclusion": "failure",
                    "steps": [{"name": "b", "conclusion": "failure"}]}]),
        ),
        false,
    );
    assert!(matches!(g.gh.status("sha1").unwrap(), CiState::Red { .. }));
    assert_eq!(cancels(&g), 0);
}

#[test]
fn running_and_passed_jobs_leave_a_run_pending() {
    use serde_json::json;
    let g = judged_gh(
        live_script(
            "mason/ci/star-b1-1",
            json!([
                {"id": 1, "name": "a", "conclusion": "success", "steps": []},
                {"id": 2, "name": "b", "conclusion": null, "steps": []},
            ]),
        ),
        false,
    );
    assert_eq!(g.gh.status("sha1").unwrap(), CiState::Pending);
    assert_eq!(cancels(&g), 0);
}

#[test]
fn jobs_are_counted_by_where_they_are() {
    use serde_json::json;
    let jobs = vec![
        json!({"name": "a", "status": "queued", "conclusion": null}),
        json!({"name": "b", "status": "in_progress", "conclusion": null}),
        json!({"name": "c", "status": "completed", "conclusion": "success"}),
        json!({"name": "d", "status": "completed", "conclusion": "skipped"}),
        json!({"name": "e", "status": "completed", "conclusion": "failure"}),
    ];
    let c = mason_adapters::github::count_jobs("pr-gate", "https://x/1", &jobs);
    assert_eq!(
        (c.queued, c.running, c.passed, c.failed),
        (1, 1, 2, 1),
        "{c:?}"
    );
    assert_eq!(c.running_names, vec!["b".to_string()]);
}

// ---- remote runs (#697) ----------------------------------------------------------------------

fn inline_gh(script: &str) -> Gh {
    let d = tempfile::tempdir().unwrap();
    let tok = d.path().join("token");
    std::fs::write(&tok, "ghp_recorded_test_token\n").unwrap();
    let http = Arc::new(Recorded::from_json(script).unwrap());
    let gh = GitHub::new(
        http.clone(),
        "https://api.github.com",
        "o/r",
        TokenRef::File(tok),
    );
    Gh { gh, http, _d: d }
}

#[test]
fn a_remote_run_is_found_by_sha_polled_with_its_etag_and_cancelled_only_on_masons_refs() {
    let g = inline_gh(
        r#"[
        {"method": "GET", "url": "/actions/runs?head_sha=abc", "response": {"status": 200,
          "body": {"workflow_runs": [{"id": 7, "status": "queued", "head_branch": "mason/run/p/x",
                   "html_url": "https://github.com/o/r/actions/runs/7", "created_at": "2026-11-20T10:00:00Z"}]}}},
        {"method": "GET", "url": "/actions/runs/7", "response": {"status": 304}},
        {"method": "POST", "url": "/actions/runs/7/cancel", "response": {"status": 202, "body": {}}}
    ]"#,
    );
    let runs = g.gh.runs_for_sha("abc").unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].id, 7);
    assert_eq!(runs[0].branch, "mason/run/p/x");
    // 304: nothing changed.
    assert!(g.gh.run_changed(7, Some("\"e\"")).unwrap().is_none());
    let sent = g.http.requests();
    assert!(sent[1]
        .headers
        .iter()
        .any(|(k, v)| k == "If-None-Match" && v == "\"e\""));
    // A run on the base branch (or a PR) is never cancelled; MASON's own refs are.
    let other = mason_adapters::github::ActionRun {
        id: 9,
        branch: "main".into(),
        ..Default::default()
    };
    assert_eq!(
        g.gh.cancel_own_run(&other).unwrap_err().kind,
        ErrorKind::Invalid
    );
    g.gh.cancel_own_run(&runs[0]).unwrap();
    assert_eq!(
        g.http.requests().len(),
        3,
        "the refused cancel sent nothing"
    );
}

#[test]
fn an_artifact_is_downloaded_through_the_signed_redirect_without_the_token() {
    let g = inline_gh(
        r#"[
        {"method": "GET", "url": "/actions/runs/7/artifacts?name=mason-run", "response": {"status": 200,
          "body": {"artifacts": [{"name": "mason-run", "expired": false,
                   "archive_download_url": "https://api.github.com/repos/o/r/actions/artifacts/3/zip"}]}}},
        {"method": "GET", "url": "/actions/artifacts/3/zip", "response": {"status": 302,
          "headers": {"Location": "https://signed.example/blob"}}},
        {"method": "GET", "url": "https://signed.example/blob", "response": {"status": 200, "body": "zipbytes"}}
    ]"#,
    );
    let dest = g._d.path().join("a.zip");
    assert!(g.gh.download_artifact(7, "mason-run", &dest).unwrap());
    assert_eq!(std::fs::read_to_string(&dest).unwrap(), "zipbytes");
    let sent = g.http.requests();
    let has_auth = |i: usize| sent[i].headers.iter().any(|(k, _)| k == "Authorization");
    assert!(
        has_auth(0) && has_auth(1),
        "GitHub's own addresses get the token"
    );
    assert!(!has_auth(2), "the signed address does not");
    // No artifact of that name: false, not an error.
    let g = inline_gh(
        r#"[{"method": "GET", "url": "/artifacts?name=mason-run", "response": {"status": 200,
            "body": {"artifacts": []}}}]"#,
    );
    assert!(!g
        .gh
        .download_artifact(7, "mason-run", &g._d.path().join("b.zip"))
        .unwrap());
}

#[test]
fn a_base_run_is_named_by_its_workflow_not_its_display_title() {
    let g = gh("base_runs_titled.json");
    let runs = g.gh.base_runs("develop").unwrap();
    assert_eq!(runs.len(), 2);
    assert!(runs.iter().all(|r| r.workflow == "CI"));
    // One read of the workflow list serves both runs.
    assert!(g.http.unused().is_empty());
}

#[test]
fn a_refs_caches_are_listed_and_deleted_one_by_one() {
    // GitHub's bulk DELETE /actions/caches needs a cache key as well as the ref (HTTP 422
    // without one), so the caches of a run ref are listed and deleted by id; one already
    // gone (404) counts as deleted.
    let g = gh("caches_for_ref.json");
    assert_eq!(
        g.gh.delete_caches_for_ref("refs/heads/mason/run/x")
            .unwrap(),
        2
    );
    assert!(
        g.http.unused().is_empty(),
        "the script expected more: {:?}",
        g.http.unused()
    );
}

// ---- cancelling the runs MASON stops waiting on (task 1108) -----------------------------------

#[test]
fn cancel_takes_only_the_unfinished_runs_of_that_commit_on_that_ref() {
    let g = inline_gh(
        r#"[
        {"method": "GET", "url": "/actions/runs?head_sha=abc&per_page=100", "response": {"status": 200,
          "body": {"workflow_runs": [
            {"id": 1, "status": "in_progress", "head_branch": "mason/ci/p-b1-0", "event": "push"},
            {"id": 2, "status": "completed", "head_branch": "mason/ci/p-b1-0", "event": "push"},
            {"id": 3, "status": "in_progress", "head_branch": "mason/ci/p-b2-0", "event": "push"},
            {"id": 4, "status": "queued", "head_branch": "main", "event": "push"},
            {"id": 5, "status": "queued", "head_branch": "mason/ci/p-b1-0", "event": "pull_request"},
            {"id": 6, "status": "queued", "head_branch": "mason/ci/p-b1-0", "event": "workflow_dispatch"}]}}},
        {"method": "POST", "url": "/actions/runs/1/cancel", "response": {"status": 202, "body": {}}},
        {"method": "POST", "url": "/actions/runs/6/cancel", "response": {"status": 202, "body": {}}}
    ]"#,
    );
    assert_eq!(g.gh.cancel("abc", "mason/ci/p-b1-0").unwrap(), 2);
    assert!(g.http.unused().is_empty(), "{:?}", g.http.unused());
    assert_eq!(g.http.requests().len(), 3);
    // A ref that is not MASON's is not even looked at.
    for r in ["main", "feature/x", "mason/run/p/x"] {
        assert_eq!(g.gh.cancel("abc", r).unwrap(), 0);
    }
    assert_eq!(g.http.requests().len(), 3);
}

#[test]
fn cancel_without_a_commit_finds_a_pull_requests_runs_by_its_branch() {
    let g = inline_gh(
        r#"[
        {"method": "GET", "url": "/actions/runs?branch=mason/pr/p-b1-0&per_page=100", "response": {"status": 200,
          "body": {"workflow_runs": [
            {"id": 8, "status": "in_progress", "head_branch": "mason/pr/p-b1-0", "event": "pull_request"},
            {"id": 9, "status": "completed", "head_branch": "mason/pr/p-b1-0", "event": "pull_request"}]}}},
        {"method": "POST", "url": "/actions/runs/8/cancel", "response": {"status": 202, "body": {}}}
    ]"#,
    );
    assert_eq!(g.gh.cancel("", "mason/pr/p-b1-0").unwrap(), 1);
    assert!(g.http.unused().is_empty(), "{:?}", g.http.unused());
}

#[test]
fn a_run_that_cannot_be_cancelled_is_reported_after_the_others_are() {
    let g = inline_gh(
        r#"[
        {"method": "GET", "url": "/actions/runs?head_sha=abc&per_page=100", "response": {"status": 200,
          "body": {"workflow_runs": [
            {"id": 1, "status": "queued", "head_branch": "mason/ci/p-b1-0"},
            {"id": 2, "status": "queued", "head_branch": "mason/ci/p-b1-0"}]}}},
        {"method": "POST", "url": "/actions/runs/1/cancel", "response": {"status": 409, "body": {"message": "Cannot cancel"}}},
        {"method": "POST", "url": "/actions/runs/2/cancel", "response": {"status": 202, "body": {}}}
    ]"#,
    );
    let e = g.gh.cancel("abc", "mason/ci/p-b1-0").unwrap_err();
    assert!(e.detail.starts_with("1 run(s) cancelled; run 1 "), "{e:?}");
    assert!(g.http.unused().is_empty(), "{:?}", g.http.unused());
}
