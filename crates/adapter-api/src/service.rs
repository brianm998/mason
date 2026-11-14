// SPDX-License-Identifier: GPL-3.0-or-later

//! What the service adapters share: classified errors, conditional polling
//! and the three traits (Part 11): issue tracker, git host, build system.
//! Capabilities are declared, so the wizard, monitor and validator offer only
//! what a service can do.

use std::path::Path;

// ---- errors ------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    /// 404: what we ask about is not there. Will not change.
    NotFound,
    /// 422 and other refusals of the request itself. Will not change.
    Invalid,
    /// 401, or 403 that is not a rate limit: the credential cannot do this.
    Auth,
    /// Slow down: retry after this many seconds if the service said.
    RateLimited { retry_after_s: Option<u64> },
    /// 5xx or no answer: an outage. Retry with backoff.
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiError {
    pub kind: ErrorKind,
    pub detail: String,
}

impl ApiError {
    pub fn new(kind: ErrorKind, detail: impl Into<String>) -> ApiError {
        ApiError {
            kind,
            detail: detail.into(),
        }
    }

    /// Will retrying change anything? A permanent error stops the poll and
    /// raises attention at once, rather than retrying for a day.
    pub fn permanent(&self) -> bool {
        matches!(
            self.kind,
            ErrorKind::NotFound | ErrorKind::Invalid | ErrorKind::Auth
        )
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.detail)
    }
}

impl std::error::Error for ApiError {}

/// Classify an HTTP answer. `None` for success (2xx) and 304.
pub fn classify(status: u16, headers: &[(String, String)], body: &str) -> Option<ApiError> {
    if (200..300).contains(&status) || status == 304 {
        return None;
    }
    let h = |n: &str| {
        headers
            .iter()
            .find(|(k, _)| k == n)
            .map(|(_, v)| v.as_str())
    };
    let msg = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v["message"].as_str().map(String::from))
        .unwrap_or_else(|| body.chars().take(200).collect());
    let detail = format!("HTTP {status}: {msg}");
    let retry = h("retry-after").and_then(|v| v.trim().parse().ok());
    let limited = status == 429
        || (status == 403 && (retry.is_some() || h("x-ratelimit-remaining") == Some("0")));
    Some(if limited {
        ApiError::new(
            ErrorKind::RateLimited {
                retry_after_s: retry,
            },
            detail,
        )
    } else {
        match status {
            404 => ApiError::new(ErrorKind::NotFound, detail),
            401 | 403 => ApiError::new(ErrorKind::Auth, detail),
            500..=599 => ApiError::new(ErrorKind::Unavailable, detail),
            _ => ApiError::new(ErrorKind::Invalid, detail),
        }
    })
}

// ---- conditional polling -----------------------------------------------------------

/// What one poll found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Observed {
    Changed {
        etag: Option<String>,
    },
    /// 304: nothing changed, and it did not count against the rate limit.
    Unchanged,
    Failed(ApiError),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PollEvent {
    /// New content: read it.
    Changed,
    Quiet,
    /// A transient problem: next try at this time (seconds on the caller's clock).
    Backoff {
        until: u64,
        why: String,
    },
    /// A permanent problem: the poll stops and the owner is told at once.
    Stopped {
        why: String,
    },
}

/// One thing being watched, with its ETag, its schedule and its backoff.
/// Pure: the caller supplies the clock and does the request.
#[derive(Clone, Debug)]
pub struct Poller {
    pub etag: Option<String>,
    pub interval_s: u64,
    pub max_backoff_s: u64,
    failures: u32,
    next_at: u64,
    stopped: Option<String>,
}

impl Poller {
    pub fn new(interval_s: u64) -> Poller {
        Poller {
            etag: None,
            interval_s,
            max_backoff_s: 3600,
            failures: 0,
            next_at: 0,
            stopped: None,
        }
    }

    pub fn stopped(&self) -> Option<&str> {
        self.stopped.as_deref()
    }

    pub fn failures(&self) -> u32 {
        self.failures
    }

    /// Is it time to ask? Never once stopped (until [`Poller::refresh`]).
    pub fn due(&self, now: u64) -> bool {
        self.stopped.is_none() && now >= self.next_at
    }

    /// The owner opened the task: ask now, even if stopped (one try; a
    /// permanent error stops it again at once).
    pub fn refresh(&mut self) {
        self.stopped = None;
        self.next_at = 0;
    }

    pub fn observe(&mut self, now: u64, o: Observed) -> PollEvent {
        match o {
            Observed::Changed { etag } => {
                self.failures = 0;
                self.etag = etag;
                self.next_at = now + self.interval_s;
                PollEvent::Changed
            }
            Observed::Unchanged => {
                self.failures = 0;
                self.next_at = now + self.interval_s;
                PollEvent::Quiet
            }
            Observed::Failed(e) if e.permanent() => {
                self.stopped = Some(e.to_string());
                PollEvent::Stopped { why: e.to_string() }
            }
            Observed::Failed(e) => {
                self.failures += 1;
                let exp = self
                    .interval_s
                    .max(1)
                    .saturating_mul(1u64 << self.failures.min(20))
                    .min(self.max_backoff_s);
                let wait = match e.kind {
                    // The service said how long; never come back sooner.
                    ErrorKind::RateLimited {
                        retry_after_s: Some(s),
                    } => s.max(1),
                    _ => exp,
                };
                self.next_at = now + wait;
                PollEvent::Backoff {
                    until: self.next_at,
                    why: e.to_string(),
                }
            }
        }
    }
}

// ---- git host ----------------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HostCaps {
    pub pull_requests: bool,
    pub labels: bool,
    pub checks: bool,
    pub merge_queue: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrRef {
    pub number: u64,
    pub url: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrState {
    Open,
    Merged,
    /// Closed without merging (or dropped from a merge queue).
    Closed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Checks {
    Pending,
    Green,
    Red,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Review {
    /// No approval is required, or it was given.
    Satisfied,
    Required,
    ChangesRequested,
}

/// How far a host's checks are: `done` of `total` finished, and the name of one that is
/// running. Given only by a host that says (`HostCaps::checks`) and has them listed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CheckCounts {
    pub done: u64,
    pub total: u64,
    pub running: Option<String>,
    /// The checks told apart by state, when the host can say.
    pub jobs: Option<CiJobCounts>,
}

/// CI jobs told apart by state. "Succeeded" is success, neutral and skipped (what counts
/// as green); "failed" is any other finished conclusion; "queued" is queued, waiting,
/// pending and requested; "running" is in progress.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CiJobCounts {
    pub total: u64,
    pub queued: u64,
    pub running: u64,
    pub succeeded: u64,
    pub failed: u64,
    pub running_names: Vec<String>,
}

impl CiJobCounts {
    /// Count one job by its `status` and `conclusion` as GitHub spells them.
    pub fn add(&mut self, name: &str, status: &str, conclusion: Option<&str>) {
        self.total += 1;
        match status {
            "completed" => match conclusion {
                Some("success" | "neutral" | "skipped") => self.succeeded += 1,
                _ => self.failed += 1,
            },
            "in_progress" => {
                self.running += 1;
                self.running_names.push(name.to_string());
            }
            _ => self.queued += 1,
        }
    }

    /// Add another set of counts into this one.
    pub fn merge(&mut self, o: &CiJobCounts) {
        self.total += o.total;
        self.queued += o.queued;
        self.running += o.running;
        self.succeeded += o.succeeded;
        self.failed += o.failed;
        self.running_names.extend(o.running_names.iter().cloned());
    }
}

/// The jobs of one workflow run that CI has not finished.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CiRunJobs {
    pub workflow: String,
    pub jobs: CiJobCounts,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrStatus {
    pub state: PrState,
    pub checks: Checks,
    /// Names of the checks that failed.
    pub failed: Vec<String>,
    pub review: Review,
    /// The head commit the status is about.
    pub head: String,
    /// Checks finished out of checks listed, when the host lists them.
    pub counts: Option<CheckCounts>,
    /// The failed checks one by one, with whether each looks like infrastructure. Empty
    /// when the host does not say (nothing is judged then).
    pub jobs: Vec<JobFailure>,
}

/// A failed CI job and, when the adapter judged it, why it looks like infrastructure
/// (`None`: a real failure of the change).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobFailure {
    pub workflow: String,
    pub job: String,
    pub infra: Option<String>,
    /// The end of the job's log (a few hundred lines), when the adapter could read it.
    pub log: Option<String>,
}

pub trait GitHostAdapter: Send + Sync {
    fn name(&self) -> &'static str;
    fn capabilities(&self) -> HostCaps;
    fn open_pr(&self, base: &str, head: &str, title: &str, body: &str) -> Result<PrRef, ApiError>;
    fn pr_status(&self, number: u64) -> Result<PrStatus, ApiError>;
    /// Merge once checks and review allow. Refused (`Invalid`) otherwise.
    fn merge_pr(&self, number: u64) -> Result<(), ApiError>;
    /// Hand the PR to the host's own merge queue.
    fn enqueue(&self, number: u64) -> Result<(), ApiError>;
    fn close_pr(&self, number: u64) -> Result<(), ApiError>;
}

// ---- tracker -----------------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrackerCaps {
    pub conditional_polling: bool,
    pub comments: bool,
    pub labels: bool,
    pub webhooks: bool,
    /// Can say what an issue is blocked by (`TrackerAdapter::blocked_by`).
    pub dependencies: bool,
}

/// An issue another issue is blocked by, as the tracker's own relationship says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Blocker {
    pub number: u64,
    pub open: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub id: String,
    pub number: u64,
    pub title: String,
    pub body: String,
    /// `urgent`, `high`, `normal` or `low`, from the tracker's labels.
    pub priority: String,
    pub labels: Vec<String>,
    pub author: String,
    pub open: bool,
    /// A pull request, not an issue (trackers list both together).
    pub is_pr: bool,
}

/// One poll of a list: new content with its ETag, or "unchanged".
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Listing {
    Items {
        items: Vec<Item>,
        etag: Option<String>,
    },
    NotModified,
}

pub trait TrackerAdapter: Send + Sync {
    fn name(&self) -> &'static str;
    fn capabilities(&self) -> TrackerCaps;
    /// Open and recently closed items, conditionally on `etag`.
    fn list(&self, etag: Option<&str>) -> Result<Listing, ApiError>;
    /// Mirror a state change out (best effort: the caller ignores failure).
    fn mirror(&self, item: &Item, note: &str) -> Result<(), ApiError>;
    fn close(&self, item: &Item, note: &str) -> Result<(), ApiError>;
    /// The issues of this repository that `number` is blocked by (the tracker's
    /// own relationship; text in the body is read by the caller). A tracker
    /// without the relationship says so in its capabilities and returns none.
    fn blocked_by(&self, _number: u64) -> Result<Vec<Blocker>, ApiError> {
        Ok(vec![])
    }
}

// ---- build system ------------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BuildCaps {
    /// Can run on an arbitrary ref (the candidate refs of `direct-ci`).
    pub arbitrary_ref: bool,
    pub per_job_results: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CiState {
    Pending,
    Green,
    Red {
        /// The jobs (or suites) that failed, when the system says.
        failed: Vec<String>,
        summary: String,
        /// The failed jobs judged one by one; empty when the system does not say.
        jobs: Vec<JobFailure>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem(pub String);

/// One workflow's jobs, counted by where they are.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CiWorkflowJobs {
    pub workflow: String,
    pub queued: u32,
    pub running: u32,
    pub passed: u32,
    pub failed: u32,
    /// Names of the running jobs.
    pub running_names: Vec<String>,
    /// The run's address (`html_url`), when the system has one.
    pub url: String,
}

/// Where a CI run is, per workflow. Read only for display: it never decides a verdict.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CiProgress {
    pub workflows: Vec<CiWorkflowJobs>,
}

pub trait BuildSystemAdapter: Send + Sync {
    fn name(&self) -> &'static str;
    fn capabilities(&self) -> BuildCaps;
    /// Start CI on `git_ref` at `sha`. Returns the run's id.
    fn trigger(&self, git_ref: &str, sha: &str) -> Result<String, ApiError>;
    fn status(&self, run: &str) -> Result<CiState, ApiError>;
    /// The state, and while it is pending the jobs of each workflow told apart by state.
    /// Reporting only: the state is what `status` gives. A system that cannot say gives none.
    fn status_detail(&self, run: &str) -> Result<(CiState, Vec<CiRunJobs>), ApiError> {
        Ok((self.status(run)?, vec![]))
    }
    /// Job counts per workflow, for display. Systems that cannot say answer empty.
    fn progress(&self, _run: &str) -> Result<CiProgress, ApiError> {
        Ok(CiProgress::default())
    }
    /// Does the repository at `repo` let CI run on a candidate ref?
    /// (`mason doctor` and, later, `mason init` use this.)
    fn doctor(&self, repo: &Path) -> Vec<Problem>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(v: &[(&str, &str)]) -> Vec<(String, String)> {
        v.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn statuses_are_classified() {
        assert!(classify(200, &[], "").is_none() && classify(304, &[], "").is_none());
        assert_eq!(
            classify(404, &[], "{\"message\":\"Not Found\"}")
                .unwrap()
                .kind,
            ErrorKind::NotFound
        );
        assert_eq!(classify(422, &[], "{}").unwrap().kind, ErrorKind::Invalid);
        assert_eq!(classify(401, &[], "").unwrap().kind, ErrorKind::Auth);
        assert_eq!(classify(403, &[], "").unwrap().kind, ErrorKind::Auth);
        assert_eq!(
            classify(403, &h(&[("x-ratelimit-remaining", "0")]), "")
                .unwrap()
                .kind,
            ErrorKind::RateLimited {
                retry_after_s: None
            }
        );
        assert_eq!(
            classify(429, &h(&[("retry-after", "30")]), "")
                .unwrap()
                .kind,
            ErrorKind::RateLimited {
                retry_after_s: Some(30)
            }
        );
        assert_eq!(classify(503, &[], "").unwrap().kind, ErrorKind::Unavailable);
        assert!(classify(404, &[], "").unwrap().permanent());
        assert!(classify(422, &[], "").unwrap().permanent());
        assert!(!classify(503, &[], "").unwrap().permanent());
        assert!(!classify(429, &[], "").unwrap().permanent());
    }

    #[test]
    fn a_permanent_error_stops_the_poll_at_once() {
        let mut p = Poller::new(60);
        assert!(p.due(0));
        let e = p.observe(
            0,
            Observed::Failed(ApiError::new(ErrorKind::NotFound, "gone")),
        );
        assert!(matches!(e, PollEvent::Stopped { .. }));
        assert!(
            !p.due(10_000_000),
            "a stopped poll never asks again by itself"
        );
        assert_eq!(p.failures(), 0, "no retry was scheduled");
        // Opening the task asks once more; a permanent error stops it again.
        p.refresh();
        assert!(p.due(0));
        assert!(matches!(
            p.observe(
                0,
                Observed::Failed(ApiError::new(ErrorKind::Invalid, "still"))
            ),
            PollEvent::Stopped { .. }
        ));
    }

    #[test]
    fn outages_back_off_and_a_good_answer_resets() {
        let mut p = Poller::new(10);
        let out = || Observed::Failed(ApiError::new(ErrorKind::Unavailable, "down"));
        let mut waits = vec![];
        let mut now = 0;
        for _ in 0..4 {
            let PollEvent::Backoff { until, .. } = p.observe(now, out()) else {
                panic!()
            };
            waits.push(until - now);
            now = until;
        }
        assert_eq!(waits, [20, 40, 80, 160], "doubling");
        assert!(matches!(
            p.observe(now, Observed::Unchanged),
            PollEvent::Quiet
        ));
        assert_eq!(p.failures(), 0);
        assert!(!p.due(now + 5) && p.due(now + 10));
    }

    #[test]
    fn backoff_is_capped_and_a_rate_limit_waits_as_told() {
        let mut p = Poller::new(60);
        p.max_backoff_s = 300;
        let mut last = 0;
        for _ in 0..10 {
            if let PollEvent::Backoff { until, .. } = p.observe(
                0,
                Observed::Failed(ApiError::new(ErrorKind::Unavailable, "x")),
            ) {
                last = until;
            }
        }
        assert_eq!(last, 300);
        let e = ApiError::new(
            ErrorKind::RateLimited {
                retry_after_s: Some(900),
            },
            "slow down",
        );
        let PollEvent::Backoff { until, .. } = p.observe(1000, Observed::Failed(e)) else {
            panic!()
        };
        assert_eq!(until, 1900, "never sooner than the service said");
    }

    #[test]
    fn changed_stores_the_etag() {
        let mut p = Poller::new(30);
        assert_eq!(
            p.observe(
                5,
                Observed::Changed {
                    etag: Some("\"a\"".into())
                }
            ),
            PollEvent::Changed
        );
        assert_eq!(p.etag.as_deref(), Some("\"a\""));
        assert!(!p.due(34) && p.due(35));
    }
}
