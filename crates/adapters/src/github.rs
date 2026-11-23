// SPDX-License-Identifier: GPL-3.0-or-later

//! GitHub: Issues as the tracker, pull requests and checks as the git host,
//! Actions as the build system. All three speak REST (and one GraphQL
//! mutation for the merge queue) through an [`Http`], so tests replay recorded
//! responses and nothing here knows about the network.

use mason_adapter_api::http::{Http, Request, Response};
use mason_adapter_api::service::*;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;

/// Credentials are references, never values in config.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TokenRef {
    /// The name of an environment variable.
    Env(String),
    /// A protected file holding the token.
    File(std::path::PathBuf),
}

impl TokenRef {
    pub fn parse(s: &str) -> Result<TokenRef, String> {
        match s.split_once(':') {
            Some(("env", n)) if !n.is_empty() => Ok(TokenRef::Env(n.into())),
            Some(("file", p)) if !p.is_empty() => Ok(TokenRef::File(p.into())),
            _ => Err(format!(
                "a token is a reference: `env:NAME` or `file:/path`, not {s:?}"
            )),
        }
    }

    pub fn read(&self) -> Result<String, ApiError> {
        let v = match self {
            TokenRef::Env(n) => {
                std::env::var(n).map_err(|_| format!("the environment variable {n} is not set"))
            }
            TokenRef::File(p) => {
                std::fs::read_to_string(p).map_err(|e| format!("cannot read {}: {e}", p.display()))
            }
        };
        match v {
            Ok(t) if !t.trim().is_empty() => Ok(t.trim().to_string()),
            Ok(_) => Err(ApiError::new(ErrorKind::Auth, "the token is empty")),
            Err(e) => Err(ApiError::new(ErrorKind::Auth, e)),
        }
    }
}

#[derive(Clone)]
pub struct GitHub {
    http: Arc<dyn Http>,
    api: String,
    /// `owner/name`.
    repo: String,
    token: TokenRef,
    /// The workflow file a dispatch targets (`ci.yml`), if CI is dispatched.
    pub workflow: Option<String>,
    /// Judge each failed job as infrastructure or real (`[ci] infra_judge`). `None` is
    /// off: no log is fetched and nothing is reported per job. `Some` holds the
    /// project's extra log patterns, added to the built-in ones.
    pub infra: Option<Vec<String>>,
    /// The repository's workflows, by id and by file path, read once and again on a miss.
    names: Arc<std::sync::Mutex<WorkflowNames>>,
}

/// Workflow names by id and by file path (`.github/workflows/ci.yml`).
#[derive(Default)]
struct WorkflowNames {
    by_id: std::collections::HashMap<u64, String>,
    by_path: std::collections::HashMap<String, String>,
}

impl GitHub {
    /// Some run on the commit is still going. Read its jobs: one that has already failed (and
    /// is not judged infrastructure) makes the verdict red now, and the runs MASON started are
    /// cancelled so the runner is free. Otherwise it is still pending.
    fn pending_or_red_early(
        &self,
        runs: &[Value],
        read: &mut std::collections::HashMap<u64, Value>,
    ) -> Result<CiState, ApiError> {
        let mut run_names = vec![];
        let mut failed = vec![];
        let mut judged = vec![];
        for r in runs.iter().filter(|r| r["status"] != "completed") {
            let Some(id) = r["id"].as_u64() else { continue };
            // The latest attempt's jobs, read once: the progress detail reuses them.
            let jobs = self.json(Request::get(&self.url(&format!(
                "/actions/runs/{id}/jobs?filter=latest&per_page=100"
            ))))?;
            read.insert(id, jobs.clone());
            let mut hit = false;
            for j in jobs["jobs"].as_array().into_iter().flatten() {
                if j["conclusion"].as_str() != Some("failure") {
                    continue;
                }
                let steps_run = j["steps"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|st| matches!(st["conclusion"].as_str(), Some("success" | "failure")));
                let name = j["name"].as_str().unwrap_or("?");
                let jf = self.judge_job(
                    r["name"].as_str().unwrap_or("?"),
                    name,
                    "failure",
                    steps_run,
                    j["id"].as_u64(),
                    j["html_url"].as_str(),
                );
                if jf.infra.is_some() {
                    continue; // infrastructure: wait, the run may be retried
                }
                failed.push(name.to_string());
                judged.push(jf);
                hit = true;
            }
            if hit {
                run_names.push(r["name"].as_str().unwrap_or("?").to_string());
            }
        }
        if failed.is_empty() {
            return Ok(CiState::Pending);
        }
        let mut cancelled = 0;
        for r in runs.iter().filter(|r| r["status"] != "completed") {
            if let (true, Some(id)) = (may_cancel(r), r["id"].as_u64()) {
                // Best effort: the verdict does not depend on it.
                if self.cancel_run(id).is_ok() {
                    cancelled += 1;
                }
            }
        }
        Ok(CiState::Red {
            summary: early_red_summary(&run_names, &failed[0], cancelled),
            failed,
            jobs: judged,
        })
    }

    /// Cancel a run (`POST /actions/runs/{id}/cancel`).
    pub fn cancel_run(&self, run: u64) -> Result<(), ApiError> {
        self.send(Request::post(
            &self.url(&format!("/actions/runs/{run}/cancel")),
            "{}",
        ))
        .map(|_| ())
    }

    pub fn new(http: Arc<dyn Http>, api: &str, repo: &str, token: TokenRef) -> GitHub {
        GitHub {
            http,
            api: api.trim_end_matches('/').to_string(),
            repo: repo.to_string(),
            token,
            workflow: None,
            infra: None,
            names: Default::default(),
        }
    }

    /// The workflow's own name for a run (`CI`), not the run's `name`, which is its display
    /// title when the workflow sets `run-name:`. Found by the run's `workflow_id`, else its
    /// `path`, in the repository's workflow list (read again only on a miss). A run with
    /// neither, or a list that cannot be read, falls back to the run's `name`.
    fn workflow_name(&self, r: &Value) -> String {
        let id = r["workflow_id"].as_u64();
        let path = r["path"].as_str().filter(|p| !p.is_empty());
        // `path` on a run can carry `@ref` for a reusable workflow: keep the file part.
        let path = path.map(|p| p.split('@').next().unwrap_or(p));
        let find = |n: &WorkflowNames| {
            id.and_then(|i| n.by_id.get(&i))
                .or_else(|| path.and_then(|p| n.by_path.get(p)))
                .cloned()
        };
        if id.is_some() || path.is_some() {
            if let Ok(mut n) = self.names.lock() {
                if let Some(name) = find(&n) {
                    return name;
                }
                if let Ok(v) = self.json(Request::get(&self.url("/actions/workflows?per_page=100")))
                {
                    *n = WorkflowNames::default();
                    for w in v["workflows"].as_array().into_iter().flatten() {
                        let Some(name) = w["name"].as_str() else {
                            continue;
                        };
                        if let Some(i) = w["id"].as_u64() {
                            n.by_id.insert(i, name.to_string());
                        }
                        if let Some(p) = w["path"].as_str() {
                            n.by_path.insert(p.to_string(), name.to_string());
                        }
                    }
                    if let Some(name) = find(&n) {
                        return name;
                    }
                }
            }
        }
        r["name"].as_str().unwrap_or_default().to_string()
    }

    fn url(&self, path: &str) -> String {
        format!("{}/repos/{}{}", self.api, self.repo, path)
    }

    /// The request with the token, whatever the answer's status.
    fn send_raw(&self, mut r: Request) -> Result<Response, ApiError> {
        let token = self.token.read()?;
        r = r
            .with("Authorization", &format!("Bearer {token}"))
            .with("Accept", "application/vnd.github+json")
            .with("X-GitHub-Api-Version", "2022-11-28")
            .with("User-Agent", "mason");
        self.http
            .send(&r)
            .map_err(|e| ApiError::new(ErrorKind::Unavailable, e.to_string()))
    }

    fn send(&self, r: Request) -> Result<Response, ApiError> {
        let resp = self.send_raw(r)?;
        match classify(resp.status, &resp.headers, &resp.body) {
            None => Ok(resp),
            Some(e) => Err(e),
        }
    }

    fn json(&self, r: Request) -> Result<Value, ApiError> {
        let resp = self.send(r)?;
        serde_json::from_str(&resp.body).map_err(|e| {
            ApiError::new(
                ErrorKind::Unavailable,
                format!("the answer is not JSON: {e}"),
            )
        })
    }

    fn body(v: Value) -> String {
        v.to_string()
    }

    /// A job's log. GitHub answers with a redirect to a signed URL, which is fetched
    /// without the token (it is not GitHub's to receive there). Any failure is `None`:
    /// a log that cannot be read does not make a job infrastructure.
    fn job_log(&self, job_id: u64) -> Option<String> {
        let r = self
            .send_raw(Request::get(
                &self.url(&format!("/actions/jobs/{job_id}/logs")),
            ))
            .ok()?;
        let body = match r.status {
            200..=299 => r.body,
            301 | 302 | 303 | 307 | 308 => {
                let to = r.header("location")?.to_string();
                let r2 = self.http.send(&Request::get(&to)).ok()?;
                if !(200..300).contains(&r2.status) {
                    return None;
                }
                r2.body
            }
            _ => return None,
        };
        // The end of a log is where a job says why it failed.
        const KEEP: usize = 256 * 1024;
        if body.len() > KEEP {
            let mut at = body.len() - KEEP;
            while !body.is_char_boundary(at) {
                at += 1;
            }
            return Some(body[at..].to_string());
        }
        Some(body)
    }

    /// One failed job, judged. Only a `failure` has its log fetched.
    fn judge_job(
        &self,
        workflow: &str,
        job: &str,
        conclusion: &str,
        steps_run: bool,
        job_id: Option<u64>,
        url: Option<&str>,
    ) -> JobFailure {
        let extra = self.infra.clone().unwrap_or_default();
        // The log is read for a failure whether or not infrastructure is being judged: the
        // task is given its end, and the landing compares it with the base branch's.
        let log = if conclusion == "failure" {
            job_id.and_then(|id| self.job_log(id))
        } else {
            None
        };
        JobFailure {
            workflow: workflow.into(),
            job: job.into(),
            infra: if self.infra.is_some() {
                crate::ci_infra::judge(conclusion, steps_run, log.as_deref(), &extra)
            } else {
                None
            },
            log: log.map(|l| tail_lines(&l, KEEP_LOG_LINES)),
            url: url.filter(|u| !u.is_empty()).map(String::from),
        }
    }
}

/// May MASON cancel this run? Only one on a `mason/ci/**` or `mason/run/**` ref that is not a
/// pull request's: a run on the base branch or a PR is never cancelled.
fn may_cancel(run: &Value) -> bool {
    run["head_branch"]
        .as_str()
        .is_some_and(|b| b.starts_with("mason/ci/") || b.starts_with("mason/run/"))
        && run["event"].as_str() != Some("pull_request")
}

/// The summary of a run called red while it was still going.
fn early_red_summary(runs: &[String], first_job: &str, cancelled: usize) -> String {
    let mut s = format!("{} failed", runs.join(", "));
    if cancelled > 0 {
        s.push_str(&format!(
            "; {cancelled} run(s) cancelled by MASON after {first_job} failed"
        ));
    }
    s
}

/// Lines of a failed job's log kept in a `JobFailure`.
const KEEP_LOG_LINES: usize = 400;

/// The last `n` lines of `text`.
fn tail_lines(text: &str, n: usize) -> String {
    let all: Vec<&str> = text.lines().collect();
    all[all.len().saturating_sub(n)..].join("\n")
}

/// The job id in a check run's `details_url` (`.../runs/9/job/123`).
fn job_id_of(url: &str) -> Option<u64> {
    let (_, id) = url.rsplit_once("/job/")?;
    id.split(['?', '/', '#']).next()?.parse().ok()
}

// ---- git host ----------------------------------------------------------------------------

impl GitHostAdapter for GitHub {
    fn name(&self) -> &'static str {
        "github"
    }

    fn capabilities(&self) -> HostCaps {
        HostCaps {
            pull_requests: true,
            labels: true,
            checks: true,
            merge_queue: true,
        }
    }

    fn open_pr(&self, base: &str, head: &str, title: &str, body: &str) -> Result<PrRef, ApiError> {
        let v = self.json(Request::post(
            &self.url("/pulls"),
            &Self::body(json!({"title": title, "head": head, "base": base, "body": body})),
        ))?;
        Ok(PrRef {
            number: v["number"]
                .as_u64()
                .ok_or_else(|| ApiError::new(ErrorKind::Invalid, "no PR number in the answer"))?,
            url: v["html_url"].as_str().unwrap_or_default().to_string(),
        })
    }

    fn pr_status(&self, number: u64) -> Result<PrStatus, ApiError> {
        let pr = self.json(Request::get(&self.url(&format!("/pulls/{number}"))))?;
        let head = pr["head"]["sha"].as_str().unwrap_or_default().to_string();
        let state = if pr["merged"].as_bool() == Some(true) || !pr["merged_at"].is_null() {
            PrState::Merged
        } else if pr["state"] == "closed" {
            PrState::Closed
        } else {
            PrState::Open
        };
        let runs = self.json(Request::get(
            &self.url(&format!("/commits/{head}/check-runs")),
        ))?;
        let (checks, failed) = fold_checks(&runs["check_runs"]);
        let mut jobs = vec![];
        {
            for r in runs["check_runs"].as_array().into_iter().flatten() {
                if r["status"] != "completed"
                    || matches!(
                        r["conclusion"].as_str(),
                        Some("success" | "neutral" | "skipped")
                    )
                {
                    continue;
                }
                jobs.push(self.judge_job(
                    "checks",
                    r["name"].as_str().unwrap_or("?"),
                    r["conclusion"].as_str().unwrap_or(""),
                    // A check run lists no steps: a cancelled one is judged as cancelled.
                    true,
                    r["details_url"].as_str().and_then(job_id_of),
                    r["details_url"].as_str(),
                ));
            }
        }
        let counts = count_checks(&runs["check_runs"]);
        let reviews = self.json(Request::get(&self.url(&format!("/pulls/{number}/reviews"))))?;
        let review = fold_reviews(
            &reviews,
            pr["mergeable_state"].as_str().unwrap_or(""),
            checks,
        );
        Ok(PrStatus {
            state,
            checks,
            failed,
            review,
            head,
            counts,
            jobs,
        })
    }

    fn merge_pr(&self, number: u64) -> Result<(), ApiError> {
        self.send(
            Request::post(
                &self.url(&format!("/pulls/{number}/merge")),
                &Self::body(json!({"merge_method": "merge"})),
            )
            .method("PUT"),
        )
        .map(|_| ())
    }

    fn enqueue(&self, number: u64) -> Result<(), ApiError> {
        let pr = self.json(Request::get(&self.url(&format!("/pulls/{number}"))))?;
        let node = pr["node_id"]
            .as_str()
            .ok_or_else(|| ApiError::new(ErrorKind::Invalid, "the PR has no node id"))?;
        let q = json!({
            "query": "mutation($id: ID!) { enqueuePullRequest(input: {pullRequestId: $id}) { mergeQueueEntry { id } } }",
            "variables": {"id": node},
        });
        let v = self.json(Request::post(
            &format!("{}/graphql", self.api),
            &q.to_string(),
        ))?;
        // GraphQL answers 200 with errors in the body.
        if let Some(e) = v["errors"].as_array().and_then(|e| e.first()) {
            return Err(ApiError::new(
                ErrorKind::Invalid,
                e["message"].as_str().unwrap_or("refused"),
            ));
        }
        Ok(())
    }

    fn close_pr(&self, number: u64) -> Result<(), ApiError> {
        self.send(
            Request::post(
                &self.url(&format!("/pulls/{number}")),
                &Self::body(json!({"state": "closed"})),
            )
            .method("PATCH"),
        )
        .map(|_| ())
    }
}

/// Check runs finished out of check runs listed, and the name of one still running.
fn count_checks(runs: &Value) -> Option<CheckCounts> {
    let list = runs.as_array()?;
    if list.is_empty() {
        return None;
    }
    let running = list
        .iter()
        .find(|r| r["status"] == "in_progress")
        .or_else(|| list.iter().find(|r| r["status"] != "completed"))
        .and_then(|r| r["name"].as_str().map(String::from));
    let mut jobs = CiJobCounts::default();
    for r in list {
        jobs.add(
            r["name"].as_str().unwrap_or("?"),
            r["status"].as_str().unwrap_or(""),
            r["conclusion"].as_str(),
        );
    }
    Some(CheckCounts {
        done: list.iter().filter(|r| r["status"] == "completed").count() as u64,
        total: list.len() as u64,
        running,
        jobs: Some(jobs),
    })
}

/// All check runs of a commit: any red is red; anything unfinished is pending.
fn fold_checks(runs: &Value) -> (Checks, Vec<String>) {
    let mut failed = vec![];
    let mut pending = false;
    let list = runs.as_array().cloned().unwrap_or_default();
    for r in &list {
        if r["status"] != "completed" {
            pending = true;
            continue;
        }
        match r["conclusion"].as_str() {
            Some("success" | "neutral" | "skipped") => {}
            _ => failed.push(r["name"].as_str().unwrap_or("?").to_string()),
        }
    }
    let checks = if !failed.is_empty() {
        Checks::Red
    } else if pending || list.is_empty() {
        // No check has reported yet is not green.
        Checks::Pending
    } else {
        Checks::Green
    };
    (checks, failed)
}

/// Each reviewer's last verdict counts. A branch the host calls `blocked` while
/// every check is green is waiting for a review it requires.
fn fold_reviews(reviews: &Value, mergeable_state: &str, checks: Checks) -> Review {
    let mut last: std::collections::BTreeMap<String, String> = Default::default();
    for r in reviews.as_array().into_iter().flatten() {
        let state = r["state"].as_str().unwrap_or("");
        if matches!(state, "APPROVED" | "CHANGES_REQUESTED" | "DISMISSED") {
            last.insert(
                r["user"]["login"].as_str().unwrap_or("?").to_string(),
                state.to_string(),
            );
        }
    }
    if last.values().any(|s| s == "CHANGES_REQUESTED") {
        Review::ChangesRequested
    } else if mergeable_state == "blocked"
        && checks == Checks::Green
        && !last.values().any(|s| s == "APPROVED")
    {
        Review::Required
    } else {
        Review::Satisfied
    }
}

// ---- tracker -----------------------------------------------------------------------------

impl TrackerAdapter for GitHub {
    fn name(&self) -> &'static str {
        "github-issues"
    }

    fn capabilities(&self) -> TrackerCaps {
        TrackerCaps {
            conditional_polling: true,
            comments: true,
            labels: true,
            webhooks: true,
            dependencies: true,
        }
    }

    fn blocked_by(&self, number: u64) -> Result<Vec<Blocker>, ApiError> {
        let v = self.json(Request::get(&self.url(&format!(
            "/issues/{number}/dependencies/blocked_by?per_page=100"
        ))))?;
        let own = format!("/repos/{}", self.repo);
        Ok(v.as_array()
            .ok_or_else(|| {
                ApiError::new(
                    ErrorKind::Unavailable,
                    "a list of blocking issues was expected",
                )
            })?
            .iter()
            // A blocker in another repository is outside this tracker.
            .filter(|b| {
                b["repository_url"]
                    .as_str()
                    .is_none_or(|u| u.to_ascii_lowercase().ends_with(&own.to_ascii_lowercase()))
            })
            .filter_map(|b| {
                Some(Blocker {
                    number: b["number"].as_u64()?,
                    open: b["state"] == "open",
                })
            })
            .collect())
    }

    fn list(&self, etag: Option<&str>) -> Result<Listing, ApiError> {
        let mut r =
            Request::get(&self.url("/issues?state=all&per_page=100&sort=updated&direction=desc"));
        if let Some(e) = etag {
            r = r.with("If-None-Match", e);
        }
        let resp = self.send(r)?;
        if resp.status == 304 {
            return Ok(Listing::NotModified);
        }
        let v: Value = serde_json::from_str(&resp.body).map_err(|e| {
            ApiError::new(
                ErrorKind::Unavailable,
                format!("the answer is not JSON: {e}"),
            )
        })?;
        let items = v
            .as_array()
            .ok_or_else(|| ApiError::new(ErrorKind::Unavailable, "a list of issues was expected"))?
            .iter()
            .map(item_of)
            .collect();
        Ok(Listing::Items {
            items,
            etag: resp.header("etag").map(String::from),
        })
    }

    fn mirror(&self, item: &Item, note: &str) -> Result<(), ApiError> {
        self.send(Request::post(
            &self.url(&format!("/issues/{}/comments", item.number)),
            &Self::body(json!({"body": note})),
        ))
        .map(|_| ())
    }

    fn close(&self, item: &Item, note: &str) -> Result<(), ApiError> {
        self.mirror(item, note)?;
        self.send(
            Request::post(
                &self.url(&format!("/issues/{}", item.number)),
                &Self::body(json!({"state": "closed", "state_reason": "completed"})),
            )
            .method("PATCH"),
        )
        .map(|_| ())
    }

    fn set_priority(&self, number: u64, priority: &str) -> Result<(), ApiError> {
        for other in ["urgent", "high", "low"]
            .into_iter()
            .filter(|p| *p != priority)
        {
            match self.send(
                Request::get(&self.url(&format!("/issues/{number}/labels/priority:{other}")))
                    .method("DELETE"),
            ) {
                Ok(_) => {}
                // The issue did not carry that label.
                Err(e) if e.kind == ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
        if priority != "normal" {
            self.send(Request::post(
                &self.url(&format!("/issues/{number}/labels")),
                &Self::body(json!({"labels": [format!("priority:{priority}")]})),
            ))?;
        }
        Ok(())
    }
}

fn item_of(v: &Value) -> Item {
    let labels: Vec<String> = v["labels"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|l| l["name"].as_str().map(String::from))
        .collect();
    let priority = ["urgent", "high", "low"]
        .into_iter()
        .find(|p| labels.iter().any(|l| l == &format!("priority:{p}")))
        .unwrap_or("normal")
        .to_string();
    Item {
        id: v["node_id"].as_str().unwrap_or_default().to_string(),
        number: v["number"].as_u64().unwrap_or(0),
        title: v["title"].as_str().unwrap_or_default().to_string(),
        body: v["body"].as_str().unwrap_or_default().to_string(),
        priority,
        labels,
        author: v["user"]["login"].as_str().unwrap_or_default().to_string(),
        open: v["state"] == "open",
        is_pr: v.get("pull_request").is_some(),
    }
}

// ---- the base branch's own runs ----------------------------------------------------------

/// One workflow run on a branch, as the base-branch CI watch needs it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionsRun {
    pub id: u64,
    /// The workflow's name.
    pub workflow: String,
    pub sha: String,
    pub url: String,
    pub created_ms: u64,
    /// `None`: not finished.
    pub conclusion: Option<String>,
    /// `push`, `workflow_dispatch`, ...
    pub event: String,
}

/// One run of a named workflow, finished or not, as a scheduled CI run needs it (s175).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorkflowRun {
    pub id: u64,
    /// The workflow's name.
    pub workflow: String,
    pub sha: String,
    pub url: String,
    pub created_ms: u64,
    /// `queued`, `in_progress`, `completed`, ...
    pub status: String,
    pub conclusion: Option<String>,
    /// `workflow_dispatch`, `push`, ...
    pub event: String,
    /// The run-name the dispatch gave it.
    pub title: String,
    /// 1 for the first attempt; a rerun makes it 2.
    pub attempt: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionsJob {
    pub id: u64,
    pub name: String,
    pub conclusion: Option<String>,
    /// queued, in_progress or completed (and the other spellings of "not yet").
    pub status: String,
}

/// `2026-11-11T12:30:00Z` as milliseconds since the epoch; `0` if it is not that shape.
pub fn iso_ms(s: &str) -> u64 {
    let b = s.as_bytes();
    let num = |a: usize, z: usize| -> Option<i64> { s.get(a..z)?.parse().ok() };
    if b.len() < 19 {
        return 0;
    }
    let (Some(y), Some(m), Some(d), Some(h), Some(mi), Some(se)) = (
        num(0, 4),
        num(5, 7),
        num(8, 10),
        num(11, 13),
        num(14, 16),
        num(17, 19),
    ) else {
        return 0;
    };
    // Days from the civil date (Howard Hinnant's algorithm).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    u64::try_from(((days * 24 + h) * 60 + mi) * 60 + se).map_or(0, |t| t * 1000)
}

impl GitHub {
    /// The finished workflow runs on `branch`, newest first.
    pub fn base_runs(&self, branch: &str) -> Result<Vec<ActionsRun>, ApiError> {
        let b = branch
            .replace('%', "%25")
            .replace('/', "%2F")
            .replace(' ', "%20");
        let v = self.json(Request::get(&self.url(&format!(
            "/actions/runs?branch={b}&status=completed&exclude_pull_requests=true&per_page=100"
        ))))?;
        Ok(v["workflow_runs"]
            .as_array()
            .ok_or_else(|| ApiError::new(ErrorKind::Unavailable, "a list of runs was expected"))?
            .iter()
            .filter_map(|r| {
                Some(ActionsRun {
                    id: r["id"].as_u64()?,
                    workflow: self.workflow_name(r),
                    sha: r["head_sha"].as_str().unwrap_or_default().to_string(),
                    url: r["html_url"].as_str().unwrap_or_default().to_string(),
                    created_ms: iso_ms(r["created_at"].as_str().unwrap_or_default()),
                    conclusion: r["conclusion"].as_str().map(String::from),
                    event: r["event"].as_str().unwrap_or_default().to_string(),
                })
            })
            .collect())
    }

    /// Start a workflow (`gherkin.yml`) on `git_ref` with the given inputs. GitHub answers 204
    /// with no run id: find the run with [`GitHub::workflow_file_runs`].
    pub fn dispatch_workflow(
        &self,
        file: &str,
        git_ref: &str,
        inputs: &[(String, String)],
    ) -> Result<(), ApiError> {
        let inputs: serde_json::Map<String, Value> =
            inputs.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
        let mut body = json!({"ref": git_ref});
        if !inputs.is_empty() {
            body["inputs"] = Value::Object(inputs);
        }
        self.send(Request::post(
            &self.url(&format!("/actions/workflows/{file}/dispatches")),
            &Self::body(body),
        ))
        .map(|_| ())
    }

    /// The runs of one workflow file on `branch`, finished or not, newest first.
    pub fn workflow_file_runs(
        &self,
        file: &str,
        branch: &str,
    ) -> Result<Vec<WorkflowRun>, ApiError> {
        let b = branch
            .replace('%', "%25")
            .replace('/', "%2F")
            .replace(' ', "%20");
        let v = self.json(Request::get(&self.url(&format!(
            "/actions/workflows/{file}/runs?branch={b}&exclude_pull_requests=true&per_page=100"
        ))))?;
        let mut runs: Vec<WorkflowRun> = v["workflow_runs"]
            .as_array()
            .ok_or_else(|| ApiError::new(ErrorKind::Unavailable, "a list of runs was expected"))?
            .iter()
            .filter_map(|r| {
                Some(WorkflowRun {
                    id: r["id"].as_u64()?,
                    workflow: self.workflow_name(r),
                    sha: r["head_sha"].as_str().unwrap_or_default().to_string(),
                    url: r["html_url"].as_str().unwrap_or_default().to_string(),
                    created_ms: iso_ms(r["created_at"].as_str().unwrap_or_default()),
                    status: r["status"].as_str().unwrap_or_default().to_string(),
                    conclusion: r["conclusion"].as_str().map(String::from),
                    event: r["event"].as_str().unwrap_or_default().to_string(),
                    title: r["display_title"].as_str().unwrap_or_default().to_string(),
                    attempt: r["run_attempt"].as_u64().unwrap_or(1) as u32,
                })
            })
            .collect();
        runs.sort_by_key(|r| std::cmp::Reverse(r.id));
        Ok(runs)
    }

    /// Rerun the failed jobs of a finished run (a new attempt of the same run).
    pub fn rerun_failed(&self, run: u64) -> Result<(), ApiError> {
        self.send(Request::post(
            &self.url(&format!("/actions/runs/{run}/rerun-failed-jobs")),
            "{}",
        ))
        .map(|_| ())
    }

    /// The jobs of one run (the latest attempt).
    pub fn run_jobs(&self, run: u64) -> Result<Vec<ActionsJob>, ApiError> {
        let v = self.json(Request::get(&self.url(&format!(
            "/actions/runs/{run}/jobs?filter=latest&per_page=100"
        ))))?;
        Ok(v["jobs"]
            .as_array()
            .ok_or_else(|| ApiError::new(ErrorKind::Unavailable, "a list of jobs was expected"))?
            .iter()
            .filter_map(|j| {
                Some(ActionsJob {
                    id: j["id"].as_u64()?,
                    name: j["name"].as_str().unwrap_or_default().to_string(),
                    conclusion: j["conclusion"].as_str().map(String::from),
                    status: j["status"].as_str().unwrap_or_default().to_string(),
                })
            })
            .collect())
    }

    /// The messages of a job's check-run annotations, one per line. A job that hit its time
    /// limit carries "The job has exceeded the maximum execution time of 1h0m0s". Best
    /// effort: `None` if they cannot be read.
    pub fn job_annotations(&self, job: u64) -> Option<String> {
        let v = self
            .json(Request::get(
                &self.url(&format!("/check-runs/{job}/annotations?per_page=50")),
            ))
            .ok()?;
        let text: Vec<&str> = v
            .as_array()?
            .iter()
            .filter_map(|a| a["message"].as_str())
            .collect();
        Some(text.join("\n"))
    }

    /// The last `lines` lines of a job's log. GitHub answers with a redirect to a signed
    /// address, which is read without the token. Best effort: `None` if it cannot be read.
    pub fn job_log_tail(&self, job: u64, lines: usize) -> Option<String> {
        let body = self.job_log(job)?;
        let tail = tail_lines(&body, lines);
        (!tail.trim().is_empty()).then_some(tail)
    }
}

// ---- Actions -------------------------------------------------------------------------------

impl BuildSystemAdapter for GitHub {
    fn name(&self) -> &'static str {
        "github-actions"
    }

    fn capabilities(&self) -> BuildCaps {
        BuildCaps {
            arbitrary_ref: true,
            per_job_results: true,
        }
    }

    /// A push to `mason/ci/**` starts the workflow by itself; with a workflow
    /// configured it is also dispatched on the ref. Either way the run is
    /// found by its commit, so the id is the commit.
    fn trigger(&self, git_ref: &str, sha: &str) -> Result<String, ApiError> {
        if let Some(w) = &self.workflow {
            self.send(Request::post(
                &self.url(&format!("/actions/workflows/{w}/dispatches")),
                &Self::body(json!({"ref": git_ref})),
            ))?;
        }
        Ok(sha.to_string())
    }

    fn status(&self, run: &str) -> Result<CiState, ApiError> {
        self.status_detail(run).map(|(s, _)| s)
    }

    /// While a run is unfinished the jobs of every run of the commit are read (one request
    /// each, no logs) and told apart by state, one entry per workflow run. A failed read of
    /// the jobs only leaves the detail out; the verdict never depends on it.
    fn status_detail(&self, run: &str) -> Result<(CiState, Vec<CiRunJobs>), ApiError> {
        let v = self.json(Request::get(
            &self.url(&format!("/actions/runs?head_sha={run}&per_page=20")),
        ))?;
        let runs = v["workflow_runs"].as_array().cloned().unwrap_or_default();
        if runs.is_empty() {
            return Ok((CiState::Pending, vec![])); // not started yet
        }
        if runs.iter().any(|r| r["status"] != "completed") {
            let mut read = std::collections::HashMap::new();
            let state = self.pending_or_red_early(&runs, &mut read)?;
            if state != CiState::Pending {
                return Ok((state, vec![]));
            }
            let mut detail = vec![];
            for r in &runs {
                let Some(id) = r["id"].as_u64() else { continue };
                let mut jobs = CiJobCounts::default();
                for j in read
                    .get(&id)
                    .into_iter()
                    .flat_map(|v| v["jobs"].as_array().into_iter().flatten())
                {
                    jobs.add(
                        j["name"].as_str().unwrap_or("?"),
                        j["status"].as_str().unwrap_or_default(),
                        j["conclusion"].as_str(),
                    );
                }
                detail.push(CiRunJobs {
                    workflow: self.workflow_name(r),
                    jobs,
                });
            }
            return Ok((state, detail));
        }
        self.finished(&runs).map(|s| (s, vec![]))
    }

    fn progress(&self, run: &str) -> Result<CiProgress, ApiError> {
        let v = self.json(Request::get(
            &self.url(&format!("/actions/runs?head_sha={run}&per_page=20")),
        ))?;
        let mut out = CiProgress::default();
        for r in v["workflow_runs"].as_array().into_iter().flatten() {
            let Some(id) = r["id"].as_u64() else { continue };
            let jobs = self.json(Request::get(&self.url(&format!(
                "/actions/runs/{id}/jobs?filter=latest&per_page=100"
            ))))?;
            out.workflows.push(count_jobs(
                &self.workflow_name(r),
                r["html_url"].as_str().unwrap_or_default(),
                jobs["jobs"].as_array().map_or(&[][..], Vec::as_slice),
            ));
        }
        Ok(out)
    }

    fn doctor(&self, repo: &Path) -> Vec<Problem> {
        workflow_problems(repo)
    }
}

impl GitHub {
    /// The verdict of runs that have all finished.
    fn finished(&self, runs: &[Value]) -> Result<CiState, ApiError> {
        let bad: Vec<&Value> = runs
            .iter()
            .filter(|r| {
                !matches!(
                    r["conclusion"].as_str(),
                    Some("success" | "neutral" | "skipped")
                )
            })
            .collect();
        if bad.is_empty() {
            return Ok(CiState::Green);
        }
        let mut failed = vec![];
        let mut judged = vec![];
        for r in &bad {
            if let Some(id) = r["id"].as_u64() {
                let jobs =
                    self.json(Request::get(&self.url(&format!("/actions/runs/{id}/jobs"))))?;
                for j in jobs["jobs"].as_array().into_iter().flatten() {
                    if !matches!(
                        j["conclusion"].as_str(),
                        Some("success" | "neutral" | "skipped")
                    ) {
                        failed.push(j["name"].as_str().unwrap_or("?").to_string());
                        {
                            let steps_run = j["steps"].as_array().into_iter().flatten().any(|st| {
                                matches!(st["conclusion"].as_str(), Some("success" | "failure"))
                            });
                            judged.push(self.judge_job(
                                r["name"].as_str().unwrap_or("?"),
                                j["name"].as_str().unwrap_or("?"),
                                j["conclusion"].as_str().unwrap_or(""),
                                steps_run,
                                j["id"].as_u64(),
                                j["html_url"].as_str(),
                            ));
                        }
                    }
                }
            }
        }
        Ok(CiState::Red {
            summary: format!(
                "{} failed",
                bad.iter()
                    .filter_map(|r| r["name"].as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            failed,
            jobs: judged,
        })
    }
}

/// Count a run's jobs: not started is queued, `in_progress` is running, a finished job is
/// passed when it succeeded (or was neutral or skipped) and failed otherwise.
pub fn count_jobs(workflow: &str, url: &str, jobs: &[Value]) -> CiWorkflowJobs {
    let mut c = CiWorkflowJobs {
        workflow: workflow.into(),
        url: url.into(),
        ..Default::default()
    };
    for j in jobs {
        match (j["status"].as_str(), j["conclusion"].as_str()) {
            (Some("completed"), Some("success" | "neutral" | "skipped")) => c.passed += 1,
            (Some("completed"), _) => c.failed += 1,
            (Some("in_progress"), _) => {
                c.running += 1;
                c.running_names
                    .push(j["name"].as_str().unwrap_or("?").to_string());
            }
            _ => c.queued += 1,
        }
    }
    c
}

const NO_WORKFLOW: &str = "there is no workflow in .github/workflows; direct-ci needs one that runs on `mason/ci/**` or by `workflow_dispatch`";
const NO_TRIGGER: &str = "no workflow runs on `mason/ci/**` or `workflow_dispatch`; add `on: { push: { branches: [\"mason/ci/**\"] } }` (or `workflow_dispatch`) to the workflow that builds and tests";

/// Does any of these workflow files run on `mason/ci/**` or by dispatch? A
/// text scan: no YAML parser is needed to find the trigger. Comments do not count.
fn scan_workflows(texts: &[String]) -> Vec<Problem> {
    if texts.is_empty() {
        return vec![Problem(NO_WORKFLOW.into())];
    }
    let ok = texts.iter().any(|t| {
        t.lines()
            .filter(|l| !l.trim_start().starts_with('#'))
            .any(|l| l.contains("mason/ci/**") || l.contains("workflow_dispatch"))
    });
    if ok {
        vec![]
    } else {
        vec![Problem(NO_TRIGGER.into())]
    }
}

/// From the working tree.
pub fn workflow_problems(repo: &Path) -> Vec<Problem> {
    let dir = repo.join(".github/workflows");
    let texts: Vec<String> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| {
            matches!(
                e.path().extension().and_then(|x| x.to_str()),
                Some("yml" | "yaml")
            )
        })
        .map(|e| std::fs::read_to_string(e.path()).unwrap_or_default())
        .collect();
    scan_workflows(&texts)
}

/// From a commit, so the base branch is what is checked, not whatever is checked out.
pub fn workflow_problems_at(repo: &Path, git_ref: &str) -> Vec<Problem> {
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
    };
    let names = git(&["ls-tree", "--name-only", git_ref, ".github/workflows/"]).unwrap_or_default();
    let texts: Vec<String> = names
        .lines()
        .filter(|n| n.ends_with(".yml") || n.ends_with(".yaml"))
        .filter_map(|n| git(&["show", &format!("{git_ref}:{n}")]))
        .collect();
    scan_workflows(&texts)
}

/// The workflow files of `git_ref` as `(file name, text)`, for the remote-run checks.
pub fn workflow_files_at(repo: &Path, git_ref: &str) -> Vec<(String, String)> {
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
    };
    let names = git(&["ls-tree", "--name-only", git_ref, ".github/workflows/"]).unwrap_or_default();
    names
        .lines()
        .filter(|n| n.ends_with(".yml") || n.ends_with(".yaml"))
        .filter_map(|n| {
            let text = git(&["show", &format!("{git_ref}:{n}")])?;
            Some((n.rsplit('/').next().unwrap_or(n).to_string(), text))
        })
        .collect()
}

// ---- remote runs (a suite or check run by a workflow on a `mason/run/**` ref) ---------------

/// One workflow run, as a remote run reads it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ActionRun {
    pub id: u64,
    /// `queued`, `in_progress` or `completed`.
    pub status: String,
    /// `success`, `failure`, `cancelled`, `startup_failure`, ... once completed.
    pub conclusion: Option<String>,
    pub url: String,
    pub branch: String,
    /// `created_at`, as GitHub writes it (`2026-11-20T10:00:00Z`).
    pub created_at: String,
}

fn action_run(v: &Value) -> Option<ActionRun> {
    Some(ActionRun {
        id: v["id"].as_u64()?,
        status: v["status"].as_str().unwrap_or_default().to_string(),
        conclusion: v["conclusion"].as_str().map(String::from),
        url: v["html_url"].as_str().unwrap_or_default().to_string(),
        branch: v["head_branch"].as_str().unwrap_or_default().to_string(),
        created_at: v["created_at"].as_str().unwrap_or_default().to_string(),
    })
}

/// The jobs of a run and the ETag of the answer.
pub type JobsAnswer = (Vec<ActionJob>, Option<String>);

/// A job of a run: whether a runner has taken it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ActionJob {
    pub name: String,
    pub status: String,
    pub started_at: Option<String>,
    /// success, failure, cancelled... once the job is done.
    pub conclusion: Option<String>,
    /// The runner labels the job asked for (`macos-latest`, `ubuntu-24.04`).
    pub labels: Vec<String>,
    /// The job's steps, in order, as far as GitHub reports them.
    pub steps: Vec<ActionStep>,
}

/// A step of a job: what a runner is doing now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ActionStep {
    pub name: String,
    pub status: String,
    pub started_at: Option<String>,
}

fn action_jobs(v: &Value) -> Vec<ActionJob> {
    v["jobs"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|j| ActionJob {
            name: j["name"].as_str().unwrap_or_default().to_string(),
            status: j["status"].as_str().unwrap_or_default().to_string(),
            started_at: j["started_at"].as_str().map(String::from),
            conclusion: j["conclusion"].as_str().map(String::from),
            labels: j["labels"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|l| l.as_str().map(String::from))
                .collect(),
            steps: j["steps"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|s| ActionStep {
                    name: s["name"].as_str().unwrap_or_default().to_string(),
                    status: s["status"].as_str().unwrap_or_default().to_string(),
                    started_at: s["started_at"].as_str().map(String::from),
                })
                .collect(),
        })
        .collect()
}

impl GitHub {
    /// The repository (`GET /repos/{repo}`): what the token may do and its security settings.
    pub fn repository(&self) -> Result<Value, ApiError> {
        self.json(Request::get(&self.url("")))
    }

    /// Can the token read Actions (`GET /actions/permissions`)? Nothing is written.
    pub fn actions_readable(&self) -> Result<(), ApiError> {
        self.send(Request::get(&self.url("/actions/permissions")))
            .map(|_| ())
    }

    /// Does GitHub know the workflow file and is it active (`GET /actions/workflows/{file}`)?
    /// A dispatch needs both; whether the token may write Actions is only known by dispatching.
    pub fn workflow_active(&self, file: &str) -> Result<(), ApiError> {
        let v = self.json(Request::get(
            &self.url(&format!("/actions/workflows/{file}")),
        ))?;
        match v["state"].as_str() {
            Some("active") => Ok(()),
            other => Err(ApiError::new(
                ErrorKind::Unavailable,
                format!("the workflow is {}", other.unwrap_or("not listed")),
            )),
        }
    }

    /// Delete the Actions caches saved by `git_ref`, so a deleted run branch leaves none behind:
    /// list them (`GET /actions/caches?ref=`) and delete each by id
    /// (`DELETE /actions/caches/{id}`). GitHub's bulk `DELETE /actions/caches` needs a cache
    /// `key` as well as the ref, and refused every call without one (HTTP 422). A cache that is
    /// already gone (404) counts as deleted. Returns how many were deleted.
    pub fn delete_caches_for_ref(&self, git_ref: &str) -> Result<u64, ApiError> {
        // A run ref saves a few caches; the cap keeps a misbehaving listing from looping.
        const PAGES: u32 = 10;
        let mut ids: Vec<u64> = vec![];
        for page in 1..=PAGES {
            let v = self.json(Request::get(&self.url(&format!(
                "/actions/caches?ref={git_ref}&per_page=100&page={page}"
            ))))?;
            let caches = v["actions_caches"].as_array().cloned().unwrap_or_default();
            ids.extend(caches.iter().filter_map(|c| c["id"].as_u64()));
            if caches.len() < 100 {
                break;
            }
        }
        let mut deleted = 0;
        for id in ids {
            match self
                .send(Request::get(&self.url(&format!("/actions/caches/{id}"))).method("DELETE"))
            {
                Ok(_) => deleted += 1,
                Err(e) if e.kind == ErrorKind::NotFound => deleted += 1,
                Err(e) => return Err(e),
            }
        }
        Ok(deleted)
    }

    /// The bytes the repository's Actions caches use (`GET /actions/cache/usage`).
    pub fn cache_usage_bytes(&self) -> Result<u64, ApiError> {
        let v = self.json(Request::get(&self.url("/actions/cache/usage")))?;
        Ok(v["active_caches_size_in_bytes"].as_u64().unwrap_or(0))
    }

    /// The commit time of `sha` in seconds since the epoch (`GET /commits/{sha}`).
    pub fn commit_time(&self, sha: &str) -> Option<u64> {
        let v = self
            .json(Request::get(&self.url(&format!("/commits/{sha}"))))
            .ok()?;
        let t = v["commit"]["committer"]["date"].as_str()?;
        Some(iso_ms(t) / 1000).filter(|s| *s > 0)
    }

    /// The runs of a commit (`GET /actions/runs?head_sha=`), newest first.
    pub fn runs_for_sha(&self, sha: &str) -> Result<Vec<ActionRun>, ApiError> {
        let v = self.json(Request::get(
            &self.url(&format!("/actions/runs?head_sha={sha}&per_page=20")),
        ))?;
        Ok(v["workflow_runs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(action_run)
            .collect())
    }

    /// One run, with the ETag of the last look: `None` when GitHub answered 304 (nothing
    /// changed, which does not count against the rate limit). Otherwise the run and the new ETag.
    pub fn run_changed(
        &self,
        id: u64,
        etag: Option<&str>,
    ) -> Result<Option<(ActionRun, Option<String>)>, ApiError> {
        let mut r = Request::get(&self.url(&format!("/actions/runs/{id}")));
        if let Some(e) = etag {
            r = r.with("If-None-Match", e);
        }
        let resp = self.send(r)?;
        if resp.status == 304 {
            return Ok(None);
        }
        let v: Value = serde_json::from_str(&resp.body).map_err(|e| {
            ApiError::new(
                ErrorKind::Unavailable,
                format!("the answer is not JSON: {e}"),
            )
        })?;
        let run = action_run(&v)
            .ok_or_else(|| ApiError::new(ErrorKind::Unavailable, "no run id in the answer"))?;
        Ok(Some((run, resp.header("etag").map(String::from))))
    }

    /// The runs in progress (`GET /actions/runs?status=in_progress`), as `(id, workflow name,
    /// workflow file)`, with the ETag of the last look: `None` when GitHub answered 304.
    #[allow(clippy::type_complexity)]
    pub fn in_progress_runs(
        &self,
        etag: Option<&str>,
    ) -> Result<Option<(Vec<(u64, String, String)>, Option<String>)>, ApiError> {
        self.runs_with_status("in_progress", etag)
    }

    /// The runs with a status (`in_progress`, `queued`), as `in_progress_runs` gives them.
    #[allow(clippy::type_complexity)]
    pub fn runs_with_status(
        &self,
        status: &str,
        etag: Option<&str>,
    ) -> Result<Option<(Vec<(u64, String, String)>, Option<String>)>, ApiError> {
        let mut r = Request::get(&self.url(&format!("/actions/runs?status={status}&per_page=100")));
        if let Some(e) = etag {
            r = r.with("If-None-Match", e);
        }
        let resp = self.send(r)?;
        if resp.status == 304 {
            return Ok(None);
        }
        let v: Value = serde_json::from_str(&resp.body).map_err(|e| {
            ApiError::new(
                ErrorKind::Unavailable,
                format!("the answer is not JSON: {e}"),
            )
        })?;
        let runs = v["workflow_runs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|r| {
                Some((
                    r["id"].as_u64()?,
                    r["name"].as_str().unwrap_or_default().to_string(),
                    r["path"].as_str().unwrap_or_default().to_string(),
                ))
            })
            .collect();
        Ok(Some((runs, resp.header("etag").map(String::from))))
    }

    /// The latest attempt's jobs of a run.
    pub fn remote_run_jobs(&self, id: u64) -> Result<Vec<ActionJob>, ApiError> {
        let v = self.json(Request::get(&self.url(&format!(
            "/actions/runs/{id}/jobs?filter=latest&per_page=100"
        ))))?;
        Ok(action_jobs(&v))
    }

    /// The jobs of a run, with the ETag of the last look: `None` on a 304, which does not count
    /// against the rate limit.
    pub fn remote_run_jobs_changed(
        &self,
        id: u64,
        etag: Option<&str>,
    ) -> Result<Option<JobsAnswer>, ApiError> {
        let mut r = Request::get(&self.url(&format!(
            "/actions/runs/{id}/jobs?filter=latest&per_page=100"
        )));
        if let Some(e) = etag {
            r = r.with("If-None-Match", e);
        }
        let resp = self.send(r)?;
        if resp.status == 304 {
            return Ok(None);
        }
        let v: Value = serde_json::from_str(&resp.body).map_err(|e| {
            ApiError::new(
                ErrorKind::Unavailable,
                format!("the answer is not JSON: {e}"),
            )
        })?;
        Ok(Some((
            action_jobs(&v),
            resp.header("etag").map(String::from),
        )))
    }

    /// Cancel a run MASON started on a `mason/run/**` or `mason/ci/**` ref; any other is
    /// refused (`Invalid`).
    pub fn cancel_own_run(&self, run: &ActionRun) -> Result<(), ApiError> {
        if !(run.branch.starts_with("mason/run/") || run.branch.starts_with("mason/ci/")) {
            return Err(ApiError::new(
                ErrorKind::Invalid,
                format!(
                    "run {} is on {:?}: MASON cancels only its own",
                    run.id, run.branch
                ),
            ));
        }
        self.cancel_run(run.id)
    }

    /// Download the artifact `name` of a run to `dest` (a zip). `false`: the run has no such
    /// artifact. GitHub answers the download with a redirect to a signed address, which is
    /// fetched without the token.
    pub fn download_artifact(&self, run: u64, name: &str, dest: &Path) -> Result<bool, ApiError> {
        let v = self.json(Request::get(
            &self.url(&format!("/actions/runs/{run}/artifacts?name={name}")),
        ))?;
        let Some(url) = v["artifacts"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|a| a["name"].as_str() == Some(name) && a["expired"].as_bool() != Some(true))
            .find_map(|a| a["archive_download_url"].as_str().map(String::from))
        else {
            return Ok(false);
        };
        let first = self.download_raw(&url, dest, true)?;
        let resp = match first.status {
            301 | 302 | 303 | 307 | 308 => {
                let to = first.header("location").map(String::from).ok_or_else(|| {
                    ApiError::new(ErrorKind::Unavailable, "a redirect with no location")
                })?;
                self.download_raw(&to, dest, false)?
            }
            _ => first,
        };
        match classify(resp.status, &resp.headers, &resp.body) {
            None => Ok(true),
            Some(e) => Err(e),
        }
    }

    fn download_raw(&self, url: &str, dest: &Path, with_token: bool) -> Result<Response, ApiError> {
        let mut r = Request::get(url);
        if with_token {
            let token = self.token.read()?;
            r = r
                .with("Authorization", &format!("Bearer {token}"))
                .with("Accept", "application/vnd.github+json")
                .with("X-GitHub-Api-Version", "2022-11-28")
                .with("User-Agent", "mason");
        }
        self.http
            .download(&r, dest)
            .map_err(|e| ApiError::new(ErrorKind::Unavailable, e.to_string()))
    }
}
