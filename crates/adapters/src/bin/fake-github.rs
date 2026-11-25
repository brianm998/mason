// SPDX-License-Identifier: GPL-3.0-or-later

//! A stand-in for GitHub's REST API, for exit tests (no network beyond loopback).
//!
//! `fake-github DIR PORT-FILE` serves `127.0.0.1` on a free port, writes the port
//! to PORT-FILE, and answers from files in DIR, which a test edits between polls:
//!
//! - `issues.json`: the array `GET /repos/o/r/issues` returns, in GitHub's shape.
//!   The ETag is a hash of the file, so editing it changes the ETag.
//! - `deps.json`: `{"102": [101]}`, the issues each issue is blocked by, answered
//!   at `GET /repos/o/r/issues/N/dependencies/blocked_by`. Editing it does not
//!   change the listing's ETag (a dependency edit may not touch it on GitHub).
//!
//! - `listing-status`: when present, an HTTP status code (e.g. `404`) the issue listing
//!   answers with instead of the issues, to show a stopped poll.
//!
//! - `runs.json`: the array of workflow runs on the base branch, in GitHub's shape, answered at
//!   `GET /repos/o/r/actions/runs`.
//! - `jobs-N.json`: the array of jobs of run N, answered at `.../actions/runs/N/jobs`.
//! - `log-N.txt`: the log of job N, answered at `.../actions/jobs/N/logs`.
//!
//! - `wf-runs.json`: the array of runs of a workflow file (finished or not), answered at
//!   `GET .../actions/workflows/F/runs`.
//! - `POST .../actions/workflows/F/dispatches` appends its body to `dispatch.log` and, if
//!   `dispatch-run.json` exists, moves that run (an object) to the front of `wf-runs.json`.
//! - `POST .../actions/runs/N/rerun-failed-jobs` appends `N` to `rerun.log`, makes
//!   `jobs-N-2.json` (if present) the jobs of run N and sets its `run_attempt` to 2.
//!
//! Remote runs (#697), when `run-artifact.zip` exists in DIR: `GET .../actions/runs?head_sha=SHA`
//! answers with one completed, successful run (id 9001, or one per commit when `run-artifact-red.zip` exists: its runs whose commit holds a file `red.marker` get that zip instead) whose branch is the `mason/run/**` ref of
//! the bare repository named in `origin-path` that points at SHA (none: no run). Its jobs, its
//! artifact list, the run itself (`GET .../actions/runs/N`) and the artifact's zip (the file's
//! bytes) are answered, and
//! `POST .../actions/runs/9001/cancel` is appended to `cancel.log`. While a file `run-hold`
//! exists in DIR the run and its job are `queued`, not completed.
//!
//! A comment is appended to `comments.log`; closing an issue (`PATCH`) marks it
//! closed in `issues.json`. Every request is appended to `requests.log`.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};

fn read_json(p: &Path, empty: Value) -> Value {
    std::fs::read_to_string(p)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(empty)
}

fn hash(s: &str) -> String {
    // FNV-1a: enough to tell two contents apart.
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("\"{h:016x}\"")
}

fn append(p: &Path, line: &str) {
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(p)
    {
        let _ = writeln!(f, "{line}");
    }
}

fn reply_bytes(out: &mut impl Write, bytes: &[u8]) {
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/zip\r\nConnection: close\r\n\r\n",
        bytes.len()
    );
    let _ = out.write_all(head.as_bytes());
    let _ = out.write_all(bytes);
}

/// The id of the run for `sha`. 9001 for every run, unless `run-artifact-red.zip` exists in DIR:
/// then each pushed commit has a run of its own (so that runs side by side are told apart), and
/// the sha is written to `run-<id>.sha` for the artifact to be chosen by.
fn run_id(dir: &Path, sha: &str) -> u64 {
    if !dir.join("run-artifact-red.zip").exists() {
        return 9001;
    }
    let n = 100_000 + u64::from_str_radix(sha.get(..5).unwrap_or("0"), 16).unwrap_or(0);
    let _ = std::fs::write(dir.join(format!("run-{n}.sha")), sha);
    n
}

/// Is `n` a run of this fake: 9001, or one made by [`run_id`]?
fn is_run(dir: &Path, n: &str) -> bool {
    n == "9001" || dir.join(format!("run-{n}.sha")).exists()
}

/// The artifact of run `n`: `run-artifact-red.zip` for a run whose commit holds a file named
/// `red.marker`, else `run-artifact.zip`.
fn artifact_for(dir: &Path, n: &str) -> Option<Vec<u8>> {
    let red = std::fs::read_to_string(dir.join(format!("run-{n}.sha")))
        .ok()
        .zip(std::fs::read_to_string(dir.join("origin-path")).ok())
        .is_some_and(|(sha, bare)| {
            std::process::Command::new("git")
                .args(["cat-file", "-e", &format!("{}:red.marker", sha.trim())])
                .current_dir(bare.trim())
                .output()
                .is_ok_and(|o| o.status.success())
        });
    let file = if red {
        "run-artifact-red.zip"
    } else {
        "run-artifact.zip"
    };
    std::fs::read(dir.join(file)).ok()
}

/// The `mason/run/**` branch of the repository in `origin-path` that points at `sha`.
fn run_branch(dir: &Path, sha: &str) -> Option<String> {
    let bare = std::fs::read_to_string(dir.join("origin-path")).ok()?;
    let o = std::process::Command::new("git")
        .args([
            "for-each-ref",
            "--format=%(refname:short)",
            "--points-at",
            sha,
        ])
        .arg("refs/heads/mason/run")
        .current_dir(bare.trim())
        .output()
        .ok()?;
    String::from_utf8_lossy(&o.stdout)
        .lines()
        .next()
        .map(String::from)
}

fn reply(out: &mut impl Write, status: &str, headers: &[(&str, String)], body: &str) {
    let mut r = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n",
        body.len()
    );
    for (k, v) in headers {
        r.push_str(&format!("{k}: {v}\r\n"));
    }
    r.push_str("\r\n");
    r.push_str(body);
    let _ = out.write_all(r.as_bytes());
}

fn serve(dir: &Path, stream: std::net::TcpStream) {
    let mut reader = BufReader::new(&stream);
    let mut line = String::new();
    if reader.read_line(&mut line).unwrap_or(0) == 0 {
        return;
    }
    let mut parts = line.split_whitespace();
    let (method, target) = (
        parts.next().unwrap_or("").to_string(),
        parts.next().unwrap_or("").to_string(),
    );
    let (mut len, mut inm, mut host) = (0usize, None, String::from("127.0.0.1"));
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h).unwrap_or(0) == 0 || h.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            match k.to_ascii_lowercase().as_str() {
                "content-length" => len = v.trim().parse().unwrap_or(0),
                "if-none-match" => inm = Some(v.trim().to_string()),
                "host" => host = v.trim().to_string(),
                _ => {}
            }
        }
    }
    let mut body = vec![0u8; len];
    let _ = reader.read_exact(&mut body);
    let body = String::from_utf8_lossy(&body).to_string();
    append(&dir.join("requests.log"), &format!("{method} {target}"));
    let path = target.split('?').next().unwrap_or("");
    let segs: Vec<&str> = path.trim_matches('/').split('/').collect();
    let mut out = &stream;
    let issues_path = dir.join("issues.json");
    match (method.as_str(), segs.as_slice()) {
        ("GET", ["repos", _, _, "issues"]) => {
            if let Some(code) = std::fs::read_to_string(dir.join("listing-status"))
                .ok()
                .and_then(|t| t.trim().parse::<u16>().ok())
            {
                let status = match code {
                    404 => "404 Not Found".to_string(),
                    401 => "401 Unauthorized".to_string(),
                    c => format!("{c} Error"),
                };
                return reply(&mut out, &status, &[], "{\"message\":\"recorded error\"}");
            }
            let text = std::fs::read_to_string(&issues_path).unwrap_or_else(|_| "[]".into());
            let etag = hash(&text);
            if inm.as_deref() == Some(etag.as_str()) {
                return reply(&mut out, "304 Not Modified", &[("ETag", etag)], "");
            }
            reply(&mut out, "200 OK", &[("ETag", etag)], &text);
        }
        ("GET", ["repos", owner, repo, "issues", n, "dependencies", "blocked_by"]) => {
            let deps = read_json(&dir.join("deps.json"), json!({}));
            let issues = read_json(&issues_path, json!([]));
            let list: Vec<Value> = deps[*n]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_u64)
                .map(|b| {
                    let state = issues
                        .as_array()
                        .into_iter()
                        .flatten()
                        .find(|i| i["number"] == b)
                        .map_or("open", |i| i["state"].as_str().unwrap_or("open"));
                    json!({"number": b, "state": state,
                           "repository_url": format!("https://api.github.com/repos/{owner}/{repo}")})
                })
                .collect();
            reply(&mut out, "200 OK", &[], &Value::Array(list).to_string());
        }
        ("POST", ["repos", _, _, "issues", n, "comments"]) => {
            append(&dir.join("comments.log"), &format!("#{n}: {body}"));
            reply(&mut out, "201 Created", &[], "{}");
        }
        ("PATCH", ["repos", _, _, "issues", n]) => {
            let mut issues = read_json(&issues_path, json!([]));
            if let Some(i) = issues.as_array_mut().and_then(|a| {
                a.iter_mut()
                    .find(|i| i["number"].as_u64() == n.parse().ok())
            }) {
                i["state"] = json!("closed");
            }
            let _ = std::fs::write(&issues_path, issues.to_string());
            reply(&mut out, "200 OK", &[], "{}");
        }
        ("GET", ["repos", _, _, "actions", "runs"])
            if target.contains("head_sha=") && dir.join("run-artifact.zip").exists() =>
        {
            let sha = target
                .split("head_sha=")
                .nth(1)
                .and_then(|r| r.split('&').next())
                .unwrap_or("");
            // While `run-hold` exists in DIR the run is still queued (#700).
            let held = dir.join("run-hold").exists();
            let (status, conclusion) = if held {
                ("queued", Value::Null)
            } else {
                ("completed", json!("success"))
            };
            let runs: Vec<Value> = run_branch(dir, sha)
                .map(|branch| {
                    let id = run_id(dir, sha);
                    json!({"id": id, "status": status, "conclusion": conclusion,
                           "head_branch": branch, "event": "push",
                           "html_url": format!("https://github.com/o/r/actions/runs/{id}"),
                           "created_at": "2026-11-20T10:00:00Z"})
                })
                .into_iter()
                .collect();
            reply(
                &mut out,
                "200 OK",
                &[],
                &json!({"total_count": runs.len(), "workflow_runs": runs}).to_string(),
            );
        }
        // One run, as the remote executor polls it once it knows the id (`run_changed`).
        ("GET", ["repos", _, _, "actions", "runs", n])
            if is_run(dir, n) && dir.join("run-artifact.zip").exists() =>
        {
            let held = dir.join("run-hold").exists();
            let (status, conclusion) = if held {
                ("queued", Value::Null)
            } else {
                ("completed", json!("success"))
            };
            let branch = std::fs::read_to_string(dir.join(format!("run-{n}.sha")))
                .ok()
                .and_then(|sha| run_branch(dir, sha.trim()));
            let id = n.parse::<u64>().unwrap_or(9001);
            reply(
                &mut out,
                "200 OK",
                &[],
                &json!({"id": id, "status": status, "conclusion": conclusion,
                        "head_branch": branch, "event": "push",
                        "html_url": format!("https://github.com/o/r/actions/runs/{id}"),
                        "created_at": "2026-11-20T10:00:00Z"})
                .to_string(),
            );
        }
        ("GET", ["repos", _, _, "actions", "runs", n, "artifacts"]) if is_run(dir, n) => {
            let url = format!("http://{host}/repos/o/r/actions/artifacts/{n}/zip");
            reply(
                &mut out,
                "200 OK",
                &[],
                &json!({"total_count": 1, "artifacts": [{"id": n.parse::<u64>().unwrap_or(1), "name": "mason-run",
                    "expired": false, "archive_download_url": url}]})
                .to_string(),
            );
        }
        ("GET", ["repos", _, _, "actions", "artifacts", n, "zip"]) => {
            let bytes = if *n == "1" {
                std::fs::read(dir.join("run-artifact.zip")).ok()
            } else {
                artifact_for(dir, n)
            };
            match bytes {
                Some(b) => reply_bytes(&mut out, &b),
                None => reply(
                    &mut out,
                    "404 Not Found",
                    &[],
                    "{\"message\":\"Not Found\"}",
                ),
            }
        }
        ("POST", ["repos", _, _, "actions", "runs", n, "cancel"]) => {
            append(&dir.join("cancel.log"), n);
            reply(&mut out, "202 Accepted", &[], "{}");
        }
        ("GET", ["repos", _, _, "actions", "runs"]) => {
            let runs = read_json(&dir.join("runs.json"), json!([]));
            let n = runs.as_array().map_or(0, Vec::len);
            reply(
                &mut out,
                "200 OK",
                &[],
                &json!({"total_count": n, "workflow_runs": runs}).to_string(),
            );
        }
        ("GET", ["repos", _, _, "actions", "workflows", _, "runs"]) => {
            let runs = read_json(&dir.join("wf-runs.json"), json!([]));
            let n = runs.as_array().map_or(0, Vec::len);
            reply(
                &mut out,
                "200 OK",
                &[],
                &json!({"total_count": n, "workflow_runs": runs}).to_string(),
            );
        }
        ("POST", ["repos", _, _, "actions", "workflows", _, "dispatches"]) => {
            append(&dir.join("dispatch.log"), &body);
            let new = dir.join("dispatch-run.json");
            if let Some(run) = std::fs::read_to_string(&new)
                .ok()
                .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            {
                let mut runs = read_json(&dir.join("wf-runs.json"), json!([]));
                if let Some(a) = runs.as_array_mut() {
                    a.insert(0, run);
                }
                let _ = std::fs::write(dir.join("wf-runs.json"), runs.to_string());
                let _ = std::fs::remove_file(&new);
            }
            reply(&mut out, "204 No Content", &[], "");
        }
        ("POST", ["repos", _, _, "actions", "runs", n, "rerun-failed-jobs"]) => {
            append(&dir.join("rerun.log"), n);
            if let Ok(t) = std::fs::read_to_string(dir.join(format!("jobs-{n}-2.json"))) {
                let _ = std::fs::write(dir.join(format!("jobs-{n}.json")), t);
            }
            let mut runs = read_json(&dir.join("wf-runs.json"), json!([]));
            if let Some(r) = runs
                .as_array_mut()
                .and_then(|a| a.iter_mut().find(|r| r["id"].as_u64() == n.parse().ok()))
            {
                r["run_attempt"] = json!(2);
            }
            let _ = std::fs::write(dir.join("wf-runs.json"), runs.to_string());
            reply(&mut out, "201 Created", &[], "{}");
        }
        ("GET", ["repos", _, _, "actions", "runs", n, "jobs"]) => {
            let mut jobs = read_json(&dir.join(format!("jobs-{n}.json")), json!([]));
            if is_run(dir, n) && jobs.as_array().is_some_and(Vec::is_empty) {
                // A held run's job has no runner yet.
                jobs = if dir.join("run-hold").exists() {
                    json!([{"name": "run", "status": "queued", "started_at": null}])
                } else {
                    json!([{"name": "run", "status": "completed",
                            "started_at": "2026-11-20T10:00:06Z"}])
                };
            }
            let c = jobs.as_array().map_or(0, Vec::len);
            reply(
                &mut out,
                "200 OK",
                &[],
                &json!({"total_count": c, "jobs": jobs}).to_string(),
            );
        }
        ("GET", ["repos", _, _, "actions", "jobs", n, "logs"]) => {
            match std::fs::read_to_string(dir.join(format!("log-{n}.txt"))) {
                Ok(t) => reply(&mut out, "200 OK", &[], &t),
                Err(_) => reply(
                    &mut out,
                    "404 Not Found",
                    &[],
                    "{\"message\":\"Not Found\"}",
                ),
            }
        }
        _ => reply(
            &mut out,
            "404 Not Found",
            &[],
            "{\"message\":\"Not Found\"}",
        ),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [dir, port_file] = args.as_slice() else {
        eprintln!("usage: fake-github DIR PORT-FILE");
        std::process::exit(2);
    };
    let dir = PathBuf::from(dir);
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    std::fs::write(port_file, port.to_string()).expect("port file");
    for s in listener.incoming().flatten() {
        serve(&dir, s);
    }
}
