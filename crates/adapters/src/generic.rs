// SPDX-License-Identifier: GPL-3.0-or-later

//! The generic build adapter (Part 11): one half starts a run at a commit, the
//! other says how it is going, and each half is a shell command or a URL, so any
//! CI that can be driven by either needs no code of its own.
//!
//! Both halves speak one small protocol. The trigger prints (or answers with)
//! the run's id on its first line, or nothing, and the commit is the id. The
//! status prints `pending`, `green` or `red`, with an optional second line of
//! failed jobs, comma separated; a URL may answer the same text or JSON
//! `{"state": "green"|"red"|"pending", "failed": [...]}`. A command gets
//! `MASON_REF`, `MASON_SHA` and `MASON_RUN`; a URL has `{ref}`, `{sha}` and
//! `{run}` replaced, percent-encoded.

use crate::fakehost::{parse_status, run_command};
use crate::github::TokenRef;
use crate::jenkins::enc;
use mason_adapter_api::http::{Http, Request};
use mason_adapter_api::service::*;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Half {
    Command(String),
    /// A URL: trigger is POSTed to, status is GET.
    Url(String),
}

pub struct Generic {
    pub trigger: Half,
    pub status: Half,
    pub cwd: PathBuf,
    http: Option<Arc<dyn Http>>,
    /// A bearer token for the URL halves, if the service wants one.
    pub token: Option<TokenRef>,
}

impl Generic {
    pub fn new(trigger: Half, status: Half, cwd: impl Into<PathBuf>) -> Generic {
        Generic {
            trigger,
            status,
            cwd: cwd.into(),
            http: None,
            token: None,
        }
    }

    pub fn with_http(mut self, http: Arc<dyn Http>) -> Generic {
        self.http = Some(http);
        self
    }

    fn call(&self, half: &Half, vars: &[(&str, &str)], post: bool) -> Result<String, ApiError> {
        match half {
            Half::Command(c) => {
                let env: Vec<(String, &str)> = vars
                    .iter()
                    .map(|(k, v)| (format!("MASON_{}", k.to_uppercase()), *v))
                    .collect();
                let env: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), *v)).collect();
                run_command(&self.cwd, c, &env)
            }
            Half::Url(u) => {
                let mut url = u.clone();
                for (k, v) in vars {
                    url = url.replace(&format!("{{{k}}}"), &enc(v));
                }
                let http = self.http.as_ref().ok_or_else(|| {
                    ApiError::new(ErrorKind::Invalid, "a URL half needs an HTTP client")
                })?;
                let mut r = if post {
                    Request::post(&url, "")
                } else {
                    Request::get(&url)
                };
                r = r.with("User-Agent", "mason");
                if let Some(t) = &self.token {
                    r = r.with("Authorization", &format!("Bearer {}", t.read()?));
                }
                let resp = http
                    .send(&r)
                    .map_err(|e| ApiError::new(ErrorKind::Unavailable, e.to_string()))?;
                match classify(resp.status, &resp.headers, &resp.body) {
                    Some(e) => Err(e),
                    None => Ok(resp.body.trim().to_string()),
                }
            }
        }
    }
}

impl BuildSystemAdapter for Generic {
    fn name(&self) -> &'static str {
        "generic"
    }

    fn capabilities(&self) -> BuildCaps {
        BuildCaps {
            arbitrary_ref: true,
            per_job_results: false,
        }
    }

    fn trigger(&self, git_ref: &str, sha: &str) -> Result<String, ApiError> {
        let out = self.call(&self.trigger, &[("ref", git_ref), ("sha", sha)], true)?;
        Ok(out
            .lines()
            .next()
            .filter(|l| !l.trim().is_empty())
            .unwrap_or(sha)
            .trim()
            .to_string())
    }

    fn status(&self, run: &str) -> Result<CiState, ApiError> {
        let out = self.call(&self.status, &[("run", run)], false)?;
        if let Ok(v) = serde_json::from_str::<Value>(&out) {
            if let Some(state) = v["state"].as_str() {
                let failed: Vec<String> = v["failed"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect();
                return Ok(parse_status(&format!("{state}\n{}", failed.join(","))));
            }
        }
        Ok(parse_status(&out))
    }

    fn doctor(&self, _repo: &Path) -> Vec<Problem> {
        vec![]
    }
}
