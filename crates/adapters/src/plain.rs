// SPDX-License-Identifier: GPL-3.0-or-later

//! Plain git as the host: a bare repository reached over a git transport (ssh,
//! file, git) and nothing else. There are no pull requests, labels, checks or
//! merge queue, so every capability is false and every PR call is refused for
//! good. Tasks land by the direct modes, which push a branch; this type exists
//! so the host interface is shown to need nothing a bare repository lacks.

use mason_adapter_api::service::*;
use std::process::Command;

pub struct PlainGit {
    /// The remote's URL, as git takes it: `ssh://user@host/srv/repo.git`.
    pub url: String,
}

impl PlainGit {
    pub fn new(url: impl Into<String>) -> Self {
        PlainGit { url: url.into() }
    }

    /// Can git reach the remote and list it? (`git ls-remote`, no prompts.)
    pub fn reachable(&self) -> Result<(), ApiError> {
        let o = Command::new("git")
            .args(["ls-remote", "--exit-code", "--heads", &self.url])
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .map_err(|e| ApiError::new(ErrorKind::Unavailable, e.to_string()))?;
        // Exit 2 means reachable with no matching refs, which is still reachable.
        if o.status.success() || o.status.code() == Some(2) {
            Ok(())
        } else {
            Err(ApiError::new(
                ErrorKind::Unavailable,
                String::from_utf8_lossy(&o.stderr).trim().to_string(),
            ))
        }
    }
}

fn none(what: &str) -> ApiError {
    ApiError::new(
        ErrorKind::Invalid,
        format!("plain git has no {what}: land by a direct mode"),
    )
}

impl GitHostAdapter for PlainGit {
    fn name(&self) -> &'static str {
        "plain-git"
    }

    fn capabilities(&self) -> HostCaps {
        HostCaps::default()
    }

    fn open_pr(&self, _: &str, _: &str, _: &str, _: &str) -> Result<PrRef, ApiError> {
        Err(none("pull requests"))
    }

    fn pr_status(&self, n: u64) -> Result<PrStatus, ApiError> {
        Err(ApiError::new(
            ErrorKind::NotFound,
            format!("plain git has no PR #{n}"),
        ))
    }

    fn merge_pr(&self, _: u64) -> Result<(), ApiError> {
        Err(none("pull requests to merge"))
    }

    fn enqueue(&self, _: u64) -> Result<(), ApiError> {
        Err(none("merge queue"))
    }

    fn close_pr(&self, _: u64) -> Result<(), ApiError> {
        Err(none("pull requests to close"))
    }
}
