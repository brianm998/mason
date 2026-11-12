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
}

impl GitHub {
    pub fn new(http: Arc<dyn Http>, api: &str, repo: &str, token: TokenRef) -> GitHub {
        GitHub {
            http,
            api: api.trim_end_matches('/').to_string(),
            repo: repo.to_string(),
            token,
            workflow: None,
            infra: None,
        }
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
        }
    }
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
    Some(CheckCounts {
        done: list.iter().filter(|r| r["status"] == "completed").count() as u64,
        total: list.len() as u64,
        running,
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
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionsJob {
    pub id: u64,
    pub name: String,
    pub conclusion: Option<String>,
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
                    workflow: r["name"].as_str().unwrap_or_default().to_string(),
                    sha: r["head_sha"].as_str().unwrap_or_default().to_string(),
                    url: r["html_url"].as_str().unwrap_or_default().to_string(),
                    created_ms: iso_ms(r["created_at"].as_str().unwrap_or_default()),
                    conclusion: r["conclusion"].as_str().map(String::from),
                })
            })
            .collect())
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
                })
            })
            .collect())
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
        let v = self.json(Request::get(
            &self.url(&format!("/actions/runs?head_sha={run}&per_page=20")),
        ))?;
        let runs = v["workflow_runs"].as_array().cloned().unwrap_or_default();
        if runs.is_empty() {
            return Ok(CiState::Pending); // not started yet
        }
        if runs.iter().any(|r| r["status"] != "completed") {
            return Ok(CiState::Pending);
        }
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

    fn doctor(&self, repo: &Path) -> Vec<Problem> {
        workflow_problems(repo)
    }
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
