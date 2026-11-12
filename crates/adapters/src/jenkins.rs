// SPDX-License-Identifier: GPL-3.0-or-later

//! Jenkins as the build system. A parameterised job is triggered with the ref
//! and commit; the queue item it returns becomes a build number; the build's
//! result is the status. Everything goes through an [`Http`], so tests replay
//! recorded responses. The credential is a reference, sent as a header and never
//! on a command line.

use crate::github::TokenRef;
use mason_adapter_api::http::{Http, Request, Response};
use mason_adapter_api::service::*;
use serde_json::Value;
use std::path::Path;
use std::sync::Arc;

pub struct Jenkins {
    http: Arc<dyn Http>,
    base: String,
    /// `team/app` for a job in a folder.
    job: String,
    token: TokenRef,
    /// With a user, the credential is an API token for HTTP basic auth; without,
    /// a bearer token.
    pub user: Option<String>,
    /// Fetch a CSRF crumb before each POST (Jenkins installs that require one).
    pub crumb: bool,
}

/// Percent-encode one URL component.
pub fn enc(s: &str) -> String {
    let mut o = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            o.push(b as char);
        } else {
            o.push_str(&format!("%{b:02X}"));
        }
    }
    o
}

fn base64(b: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut o = String::new();
    for c in b.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= c.len() {
                o.push(T[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                o.push('=');
            }
        }
    }
    o
}

impl Jenkins {
    pub fn new(http: Arc<dyn Http>, base: &str, job: &str, token: TokenRef) -> Jenkins {
        Jenkins {
            http,
            base: base.trim_end_matches('/').to_string(),
            job: job.trim_matches('/').to_string(),
            token,
            user: None,
            crumb: false,
        }
    }

    fn job_url(&self) -> String {
        let path: Vec<String> = self
            .job
            .split('/')
            .map(|p| format!("job/{}", enc(p)))
            .collect();
        format!("{}/{}", self.base, path.join("/"))
    }

    fn send(&self, mut r: Request) -> Result<Response, ApiError> {
        let t = self.token.read()?;
        let auth = match &self.user {
            Some(u) => format!("Basic {}", base64(format!("{u}:{t}").as_bytes())),
            None => format!("Bearer {t}"),
        };
        r = r.with("Authorization", &auth).with("User-Agent", "mason");
        let resp = self
            .http
            .send(&r)
            .map_err(|e| ApiError::new(ErrorKind::Unavailable, e.to_string()))?;
        match classify(resp.status, &resp.headers, &resp.body) {
            None => Ok(resp),
            Some(e) => Err(e),
        }
    }

    fn json(&self, url: &str) -> Result<Value, ApiError> {
        let resp = self.send(Request::get(url))?;
        serde_json::from_str(&resp.body).map_err(|e| {
            ApiError::new(
                ErrorKind::Unavailable,
                format!("the answer is not JSON: {e}"),
            )
        })
    }

    fn crumb_header(&self) -> Result<Option<(String, String)>, ApiError> {
        if !self.crumb {
            return Ok(None);
        }
        let v = self.json(&format!("{}/crumbIssuer/api/json", self.base))?;
        match (v["crumbRequestField"].as_str(), v["crumb"].as_str()) {
            (Some(f), Some(c)) => Ok(Some((f.to_string(), c.to_string()))),
            _ => Err(ApiError::new(
                ErrorKind::Unavailable,
                "the crumb issuer's answer has no crumb",
            )),
        }
    }

    fn build_status(&self, number: u64) -> Result<CiState, ApiError> {
        let b = self.json(&format!("{}/{number}/api/json", self.job_url()))?;
        if b["building"].as_bool().unwrap_or(false) {
            return Ok(CiState::Pending);
        }
        Ok(match b["result"].as_str() {
            Some("SUCCESS") => CiState::Green,
            // An unstable build (tests failed) is not a pass.
            Some(r @ ("FAILURE" | "UNSTABLE" | "ABORTED" | "NOT_BUILT")) => CiState::Red {
                failed: vec![],
                jobs: vec![],
                summary: format!("Jenkins build #{number} is {r}"),
            },
            // No result yet and not building: still being set up.
            _ => CiState::Pending,
        })
    }
}

impl BuildSystemAdapter for Jenkins {
    fn name(&self) -> &'static str {
        "jenkins"
    }

    fn capabilities(&self) -> BuildCaps {
        BuildCaps {
            arbitrary_ref: true,
            per_job_results: false,
        }
    }

    /// The job takes `REF` and `SHA` parameters. The run id is the queue item
    /// (`queue:N`) until the build starts, then the build is found from it.
    fn trigger(&self, git_ref: &str, sha: &str) -> Result<String, ApiError> {
        let mut r = Request::post(
            &format!(
                "{}/buildWithParameters?REF={}&SHA={}",
                self.job_url(),
                enc(git_ref),
                enc(sha)
            ),
            "",
        );
        if let Some((f, c)) = self.crumb_header()? {
            r = r.with(&f, &c);
        }
        let resp = self.send(r)?;
        let item = resp
            .header("location")
            .and_then(|l| l.trim_end_matches('/').rsplit('/').next())
            .and_then(|n| n.parse::<u64>().ok())
            .ok_or_else(|| {
                ApiError::new(
                    ErrorKind::Unavailable,
                    "Jenkins did not say which queue item the build is",
                )
            })?;
        Ok(format!("queue:{item}"))
    }

    fn status(&self, run: &str) -> Result<CiState, ApiError> {
        if let Some(item) = run.strip_prefix("queue:") {
            let q = self.json(&format!("{}/queue/item/{item}/api/json", self.base))?;
            if q["cancelled"].as_bool().unwrap_or(false) {
                return Ok(CiState::Red {
                    failed: vec![],
                    jobs: vec![],
                    summary: "the Jenkins queue item was cancelled".into(),
                });
            }
            return match q["executable"]["number"].as_u64() {
                Some(n) => self.build_status(n),
                None => Ok(CiState::Pending), // still waiting for an executor
            };
        }
        match run.parse::<u64>() {
            Ok(n) => self.build_status(n),
            Err(_) => Err(ApiError::new(
                ErrorKind::Invalid,
                format!("{run:?} is not a Jenkins run id"),
            )),
        }
    }

    fn doctor(&self, _repo: &Path) -> Vec<Problem> {
        match self.json(&format!("{}/api/json?tree=name", self.job_url())) {
            Ok(_) => vec![],
            Err(e) => vec![Problem(format!(
                "Jenkins job {} cannot be read: {e}",
                self.job
            ))],
        }
    }
}
