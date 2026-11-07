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
    let (mut len, mut inm) = (0usize, None);
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h).unwrap_or(0) == 0 || h.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            match k.to_ascii_lowercase().as_str() {
                "content-length" => len = v.trim().parse().unwrap_or(0),
                "if-none-match" => inm = Some(v.trim().to_string()),
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
