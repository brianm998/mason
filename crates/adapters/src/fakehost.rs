// SPDX-License-Identifier: GPL-3.0-or-later

//! A git host and a CI system for tests, backed by a bare repository and a
//! directory of small files. Nothing here talks to a network: a PR is a JSON
//! file, "checks" are a command run over the PR head's tree, and the person
//! who approves or breaks things does it by writing a file. The same adapters
//! run inside the real daemon, so the exit script exercises the real landing
//! code against them.

use mason_adapter_api::service::*;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct FakeHost {
    /// State directory: `prs/N.json`, `checks/SHA.json`.
    pub dir: PathBuf,
    /// The bare repository standing in for the host's.
    pub remote: PathBuf,
    /// Run over each PR head's tree; exit 0 is green. Lines `FAILED: name` name what broke.
    pub checks_cmd: Option<String>,
    pub require_approval: bool,
    pub merge_queue: bool,
}

fn git(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let o = Command::new("git")
        .args([
            "-c",
            "user.name=fake-host",
            "-c",
            "user.email=host@localhost",
        ])
        .args(args)
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|e| e.to_string())?;
    if o.status.success() {
        Ok(String::from_utf8_lossy(&o.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&o.stderr).trim().to_string())
    }
}

fn invalid(m: impl Into<String>) -> ApiError {
    ApiError::new(ErrorKind::Invalid, m)
}

impl FakeHost {
    fn pr_path(&self, n: u64) -> PathBuf {
        self.dir.join("prs").join(format!("{n}.json"))
    }

    fn read(&self, n: u64) -> Result<Value, ApiError> {
        let t = std::fs::read_to_string(self.pr_path(n))
            .map_err(|_| ApiError::new(ErrorKind::NotFound, format!("no PR #{n}")))?;
        serde_json::from_str(&t).map_err(|e| invalid(e.to_string()))
    }

    fn write(&self, n: u64, v: &Value) {
        let _ = std::fs::create_dir_all(self.dir.join("prs"));
        let _ = std::fs::write(self.pr_path(n), v.to_string());
    }

    /// The person approves, as a reviewer would on the host: a file of its own, so
    /// that approving never races with the host rewriting the PR's state.
    pub fn approve(dir: &Path, n: u64) {
        if dir.join("prs").join(format!("{n}.json")).exists() {
            let _ = std::fs::write(dir.join("prs").join(format!("{n}.approved")), "");
        }
    }

    fn head_sha(&self, branch: &str) -> Result<String, ApiError> {
        git(
            &self.remote,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}^{{commit}}"),
            ],
        )
        .map_err(|_| invalid(format!("the head branch {branch} is not on the host")))
    }

    /// Checks for a commit, run once and remembered.
    fn checks_of(&self, sha: &str) -> (Checks, Vec<String>) {
        let Some(cmd) = &self.checks_cmd else {
            return (Checks::Green, vec![]);
        };
        let cache = self.dir.join("checks").join(format!("{sha}.json"));
        if let Ok(t) = std::fs::read_to_string(&cache) {
            if let Ok(v) = serde_json::from_str::<Value>(&t) {
                let failed: Vec<String> = v["failed"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect();
                return (
                    if v["ok"] == true {
                        Checks::Green
                    } else {
                        Checks::Red
                    },
                    failed,
                );
            }
        }
        let work = self.dir.join("work").join(sha);
        let _ = std::fs::remove_dir_all(&work);
        let _ = std::fs::create_dir_all(&work);
        let tar = Command::new("sh")
            .args([
                "-c",
                &format!(
                    "git --git-dir='{}' archive {sha} | tar -x -C '{}'",
                    self.remote.display(),
                    work.display()
                ),
            ])
            .status();
        let out = if tar.is_ok_and(|s| s.success()) {
            Command::new("sh")
                .args(["-c", cmd])
                .current_dir(&work)
                .output()
                .ok()
        } else {
            None
        };
        let (ok, text) = match out {
            Some(o) => (
                o.status.success(),
                String::from_utf8_lossy(&o.stdout).to_string(),
            ),
            None => (false, String::new()),
        };
        let failed: Vec<String> = text
            .lines()
            .filter_map(|l| l.strip_prefix("FAILED: ").map(|s| s.trim().to_string()))
            .collect();
        let _ = std::fs::create_dir_all(self.dir.join("checks"));
        let _ = std::fs::write(&cache, json!({"ok": ok, "failed": failed}).to_string());
        let _ = std::fs::remove_dir_all(&work);
        (if ok { Checks::Green } else { Checks::Red }, failed)
    }

    /// How far the checks are, for a monitor. The host here runs its checks as one command,
    /// so the one who plays the host writes this: `prs/N.counts`
    /// `{"done": 4, "total": 7, "running": "lint"}`. No file, no counts.
    fn counts_of(&self, n: u64) -> Option<CheckCounts> {
        let t = std::fs::read_to_string(self.dir.join("prs").join(format!("{n}.counts"))).ok()?;
        let v: Value = serde_json::from_str(&t).ok()?;
        Some(CheckCounts {
            done: v["done"].as_u64()?,
            total: v["total"].as_u64()?,
            running: v["running"].as_str().map(String::from),
            jobs: None,
        })
    }

    fn do_merge(&self, n: u64, v: &mut Value) -> Result<(), ApiError> {
        let (base, head) = (
            v["base"].as_str().unwrap_or("").to_string(),
            v["head"].as_str().unwrap_or("").to_string(),
        );
        let scratch = self.dir.join("merge");
        let _ = std::fs::remove_dir_all(&scratch);
        git(
            &self.dir,
            &[
                "clone",
                "-q",
                &self.remote.display().to_string(),
                &scratch.display().to_string(),
            ],
        )
        .map_err(invalid)?;
        let r = (|| -> Result<(), String> {
            git(
                &scratch,
                &["checkout", "-q", "-B", &base, &format!("origin/{base}")],
            )?;
            git(
                &scratch,
                &[
                    "merge",
                    "--no-ff",
                    "--no-edit",
                    "-m",
                    &format!("Merge PR #{n}"),
                    &format!("origin/{head}"),
                ],
            )?;
            git(
                &scratch,
                &["push", "-q", "origin", &format!("HEAD:refs/heads/{base}")],
            )?;
            Ok(())
        })();
        let _ = std::fs::remove_dir_all(&scratch);
        r.map_err(|e| invalid(format!("not mergeable: {e}")))?;
        v["state"] = json!("merged");
        Ok(())
    }

    fn review_of(&self, n: u64, v: &Value) -> Review {
        let _ = v;
        if self.require_approval && !self.dir.join("prs").join(format!("{n}.approved")).exists() {
            Review::Required
        } else {
            Review::Satisfied
        }
    }
}

impl GitHostAdapter for FakeHost {
    fn name(&self) -> &'static str {
        "fake-host"
    }

    fn capabilities(&self) -> HostCaps {
        HostCaps {
            pull_requests: true,
            labels: false,
            checks: true,
            merge_queue: self.merge_queue,
        }
    }

    fn open_pr(&self, base: &str, head: &str, title: &str, body: &str) -> Result<PrRef, ApiError> {
        self.head_sha(head)?;
        self.head_sha(base)?;
        let n = std::fs::read_dir(self.dir.join("prs")).map_or(0, |d| {
            d.flatten()
                .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
                .count()
        }) as u64
            + 1;
        self.write(n, &json!({"base": base, "head": head, "title": title, "body": body, "state": "open", "approved": false, "queued": false}));
        Ok(PrRef {
            number: n,
            url: format!("fake://pr/{n}"),
        })
    }

    fn pr_status(&self, n: u64) -> Result<PrStatus, ApiError> {
        let mut v = self.read(n)?;
        let head = self.head_sha(v["head"].as_str().unwrap_or(""))?;
        let (checks, failed) = self.checks_of(&head);
        let review = self.review_of(n, &v);
        // The "queue": merges whatever is queued once it is green and reviewed, drops it if red.
        if v["state"] == "open" && v["queued"] == true {
            if checks == Checks::Green && review == Review::Satisfied {
                self.do_merge(n, &mut v)?;
            } else if checks == Checks::Red {
                v["state"] = json!("closed");
            }
            self.write(n, &v);
        }
        Ok(PrStatus {
            state: match v["state"].as_str() {
                Some("merged") => PrState::Merged,
                Some("closed") => PrState::Closed,
                _ => PrState::Open,
            },
            checks,
            failed,
            review,
            head,
            counts: self.counts_of(n),
            jobs: vec![],
        })
    }

    fn merge_pr(&self, n: u64) -> Result<(), ApiError> {
        let mut v = self.read(n)?;
        if v["state"] != "open" {
            return Err(invalid("the PR is not open"));
        }
        let head = self.head_sha(v["head"].as_str().unwrap_or(""))?;
        if self.checks_of(&head).0 != Checks::Green {
            return Err(invalid("checks have not passed"));
        }
        if self.review_of(n, &v) != Review::Satisfied {
            return Err(invalid("a review is required"));
        }
        self.do_merge(n, &mut v)?;
        self.write(n, &v);
        Ok(())
    }

    fn enqueue(&self, n: u64) -> Result<(), ApiError> {
        if !self.merge_queue {
            return Err(invalid("this repository has no merge queue"));
        }
        let mut v = self.read(n)?;
        v["queued"] = json!(true);
        self.write(n, &v);
        Ok(())
    }

    fn close_pr(&self, n: u64) -> Result<(), ApiError> {
        let mut v = self.read(n)?;
        if v["state"] == "open" {
            v["state"] = json!("closed");
        }
        self.write(n, &v);
        Ok(())
    }
}

/// A build system that is two commands: one starts a run at a commit, one
/// says how it is going. Part 11's generic adapter; with scripts it is also the
/// fake CI of the exit test.
///
/// `trigger` gets `MASON_REF` and `MASON_SHA` and prints the run's id (or
/// nothing: the sha is the id). `status` gets `MASON_RUN` and prints `pending`,
/// `green`, or `red` (a second line may list failed jobs, comma separated).
pub struct CommandCi {
    pub trigger: String,
    pub status: String,
    pub cwd: PathBuf,
}

/// Run `cmd` with `sh -c` in `cwd` with extra environment; its trimmed stdout, or
/// `Unavailable` when it fails. Shared by the command CI and the generic adapter.
pub fn run_command(cwd: &Path, cmd: &str, env: &[(&str, &str)]) -> Result<String, ApiError> {
    let mut c = Command::new("sh");
    c.args(["-c", cmd]).current_dir(cwd);
    for (k, v) in env {
        c.env(k, v);
    }
    let o = c
        .output()
        .map_err(|e| ApiError::new(ErrorKind::Unavailable, e.to_string()))?;
    if o.status.success() {
        Ok(String::from_utf8_lossy(&o.stdout).trim().to_string())
    } else {
        Err(ApiError::new(
            ErrorKind::Unavailable,
            format!(
                "`{cmd}` failed: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            ),
        ))
    }
}

/// The status protocol: first line `pending`, `green` or `red` (anything else is
/// pending); after `red`, a second line may list failed jobs, comma separated.
pub fn parse_status(out: &str) -> CiState {
    let mut lines = out.lines();
    match lines.next().map(str::trim) {
        Some("green") => CiState::Green,
        Some("red") => {
            let names = |l: &str| -> Vec<String> {
                l.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            };
            let failed = names(lines.next().unwrap_or(""));
            // An optional `infra: a, b` line names the jobs that are infrastructure; the
            // rest are real. No such line: nothing is judged.
            let jobs = match lines
                .next()
                .map(str::trim)
                .and_then(|l| l.strip_prefix("infra:"))
            {
                Some(l) => {
                    let infra = names(l);
                    failed
                        .iter()
                        .map(|f| JobFailure {
                            workflow: "ci".into(),
                            job: f.clone(),
                            infra: infra.contains(f).then(|| "runner lost".to_string()),
                            log: None,
                        })
                        .collect()
                }
                None => vec![],
            };
            CiState::Red {
                jobs,
                summary: if failed.is_empty() {
                    "CI is red".into()
                } else {
                    format!("{} failed", failed.join(", "))
                },
                failed,
            }
        }
        _ => CiState::Pending,
    }
}

impl CommandCi {
    fn run(&self, cmd: &str, env: &[(&str, &str)]) -> Result<String, ApiError> {
        run_command(&self.cwd, cmd, env)
    }
}

impl BuildSystemAdapter for CommandCi {
    fn name(&self) -> &'static str {
        "command"
    }

    fn capabilities(&self) -> BuildCaps {
        BuildCaps {
            arbitrary_ref: true,
            per_job_results: false,
        }
    }

    fn trigger(&self, git_ref: &str, sha: &str) -> Result<String, ApiError> {
        let out = self.run(&self.trigger, &[("MASON_REF", git_ref), ("MASON_SHA", sha)])?;
        Ok(out
            .lines()
            .next()
            .filter(|l| !l.trim().is_empty())
            .unwrap_or(sha)
            .trim()
            .to_string())
    }

    fn status(&self, run: &str) -> Result<CiState, ApiError> {
        let out = self.run(&self.status, &[("MASON_RUN", run)])?;
        Ok(parse_status(&out))
    }

    fn doctor(&self, _repo: &Path) -> Vec<Problem> {
        vec![]
    }
}
