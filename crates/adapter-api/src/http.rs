// SPDX-License-Identifier: GPL-3.0-or-later

//! The seam between the service adapters and the network. Adapters speak to
//! an [`Http`]; production uses [`Curl`] (the system's `curl`, the token never
//! on a command line), tests use [`Recorded`], which replays a script of
//! responses and fails loudly on any request it was not given.

use crate::github_budget;
use crate::netguard;
use std::collections::BTreeMap;
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::Mutex;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
}

impl Request {
    pub fn get(url: &str) -> Request {
        Request {
            method: "GET".into(),
            url: url.into(),
            headers: vec![],
            body: None,
        }
    }
    pub fn with(mut self, k: &str, v: &str) -> Request {
        self.headers.push((k.into(), v.into()));
        self
    }
    pub fn post(url: &str, body: &str) -> Request {
        Request {
            method: "POST".into(),
            url: url.into(),
            headers: vec![],
            body: Some(body.into()),
        }
    }
    pub fn method(mut self, m: &str) -> Request {
        self.method = m.into();
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    /// Lower-cased names.
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        let n = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(k, _)| *k == n)
            .map(|(_, v)| v.as_str())
    }
}

/// The request never got an answer (no route, DNS, a refused connection): an outage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transport(pub String);

impl std::fmt::Display for Transport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

pub trait Http: Send + Sync {
    fn send(&self, r: &Request) -> Result<Response, Transport>;

    /// Like [`Http::send`], but a 2xx answer's body is written byte for byte to `dest` (the
    /// response then carries an empty body): for a binary download such as an artifact's zip,
    /// which a `String` cannot hold. Any other answer is returned as `send` would.
    fn download(&self, r: &Request, dest: &std::path::Path) -> Result<Response, Transport> {
        let resp = self.send(r)?;
        if !(200..300).contains(&resp.status) {
            return Ok(resp);
        }
        std::fs::write(dest, resp.body.as_bytes())
            .map_err(|e| Transport(format!("cannot write {}: {e}", dest.display())))?;
        Ok(Response {
            body: String::new(),
            ..resp
        })
    }
}

// ---- the real thing ------------------------------------------------------------

/// `curl`, with the request's headers (the token among them) passed on stdin as a
/// config file, so nothing secret appears in the process list.
pub struct Curl {
    pub timeout_s: u32,
    /// Ask for HTTP/2 (`http2` in the config; APNs needs it).
    pub http2: bool,
}

impl Default for Curl {
    fn default() -> Self {
        Curl {
            timeout_s: 30,
            http2: false,
        }
    }
}

impl Curl {
    /// The curl to run. One that speaks HTTP/2 is needed when `http2` is set, and a `curl` found
    /// first on PATH may be an old one without it (MacPorts shipped 7.46, which answers
    /// "Unsupported protocol"), so the system's is preferred then.
    fn program(&self) -> &'static str {
        if self.http2 && std::path::Path::new("/usr/bin/curl").exists() {
            "/usr/bin/curl"
        } else {
            "curl"
        }
    }

    /// The config sent to curl on stdin: the URL, method, headers (the token among them) and
    /// body. Kept apart so a test can check what is, and is not, in it.
    pub fn config(&self, r: &Request) -> Result<String, Transport> {
        let mut cfg = String::new();
        let q = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
        cfg.push_str(&format!(
            "url = \"{}\"\nrequest = \"{}\"\n",
            q(&r.url),
            q(&r.method)
        ));
        if self.http2 {
            cfg.push_str("http2\n");
        }
        for (k, v) in &r.headers {
            if k.contains(['\n', '\r']) || v.contains(['\n', '\r']) {
                return Err(Transport("a header contains a newline".into()));
            }
            cfg.push_str(&format!("header = \"{}: {}\"\n", q(k), q(v)));
        }
        if let Some(b) = &r.body {
            cfg.push_str(&format!(
                "data-binary = \"{}\"\n",
                q(b).replace('\n', "\\n")
            ));
        }
        Ok(cfg)
    }
}

impl Http for Curl {
    fn send(&self, r: &Request) -> Result<Response, Transport> {
        self.fetch(r, None)
    }

    fn download(&self, r: &Request, dest: &std::path::Path) -> Result<Response, Transport> {
        // A non-2xx answer (a redirect, an error) is written to `dest` too: the caller reads
        // the headers and ignores the file then.
        self.fetch(r, Some(dest))
    }
}

impl Curl {
    /// One request. With `out`, curl writes the body there (binary-safe) and the response's
    /// own body is empty.
    fn fetch(
        &self,
        r: &Request,
        out_file: Option<&std::path::Path>,
    ) -> Result<Response, Transport> {
        netguard::check(&r.url).map_err(|e| Transport(e.to_string()))?;
        let github = is_github_api(&r.url);
        // `GET /rate_limit` is free (not charged to any quota) and says what the quota is, so it
        // is neither refused nor counted as a call.
        let rate_limit = github && github_budget::path_of(&r.url) == "/rate_limit";
        if github && !rate_limit {
            self.refresh_quota(r);
            // The core quota is used up: GitHub would refuse, so nothing is sent. The answer is
            // its own refusal, with the time left to the reset.
            if let Some(wait) = github_budget::wait_s() {
                let reset = github_budget::exhausted_until().unwrap_or_default();
                return Ok(Response {
                    status: 403,
                    headers: github_budget::refusal_headers(wait, reset),
                    body: format!(
                        "{{\"message\":\"API rate limit exceeded (not sent: MASON waits {wait} s for the reset)\"}}"
                    ),
                });
            }
        }
        let cfg = self.config(r)?;
        let hdr =
            std::env::temp_dir().join(format!("mason-curl-{}-{}", std::process::id(), nonce()));
        let mut cmd = Command::new(self.program());
        cmd.args(["-sS", "--max-time", &self.timeout_s.to_string(), "-D"])
            .arg(&hdr)
            .args(["-K", "-"]);
        if let Some(f) = out_file {
            cmd.arg("-o").arg(f);
        }
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd
            .spawn()
            .map_err(|e| Transport(format!("cannot run curl: {e}")))?;
        child
            .stdin
            .take()
            .ok_or_else(|| Transport("no stdin".into()))?
            .write_all(cfg.as_bytes())
            .map_err(|e| Transport(e.to_string()))?;
        let out = child
            .wait_with_output()
            .map_err(|e| Transport(e.to_string()))?;
        let head = std::fs::read_to_string(&hdr).unwrap_or_default();
        let _ = std::fs::remove_file(&hdr);
        if !out.status.success() {
            return Err(Transport(
                String::from_utf8_lossy(&out.stderr).trim().to_string(),
            ));
        }
        let (status, headers) = parse_head(&head)?;
        let body = String::from_utf8_lossy(&out.stdout).to_string();
        if rate_limit {
            if status == 200 {
                github_budget::note_rate_limit(&body);
            }
        } else if github {
            github_budget::note_request(&r.method, &r.url, status, &headers);
        }
        Ok(Response {
            status,
            headers,
            body,
        })
    }

    /// Read `GET /rate_limit` with the request's own credentials when the record is due for it
    /// ([`github_budget::claim_refresh`]: at most every few minutes, and on the first request
    /// after a start or a reset). Some endpoints answer with a quota window of their own, so
    /// the headers of answers alone cannot be trusted to name the window that counts; this read
    /// can. A failed read changes nothing and is not retried for a minute.
    fn refresh_quota(&self, r: &Request) {
        let Some(auth) = r
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("authorization"))
        else {
            return;
        };
        if !github_budget::claim_refresh() {
            return;
        }
        let req = Request::get(&format!("{}/rate_limit", GITHUB_API))
            .with("Authorization", &auth.1)
            .with("Accept", "application/vnd.github+json")
            .with("User-Agent", "mason");
        let _ = self.fetch(&req, None);
    }
}

/// The status and lower-cased headers of the last block of the header file curl wrote with `-D`
/// (earlier blocks are `100 Continue` or redirects; blocks end at a blank line, which is
/// `\r\n\r\n` for HTTP/1.1 and HTTP/2 alike).
fn parse_head(head: &str) -> Result<(u16, Vec<(String, String)>), Transport> {
    let block = head
        .split("\r\n\r\n")
        .flat_map(|b| b.split("\n\n"))
        .filter(|b| b.trim_start().starts_with("HTTP/"))
        .last()
        .unwrap_or("");
    let mut lines = block.trim_start().lines();
    let status = lines
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| Transport("no status line".into()))?;
    let headers = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    Ok((status, headers))
}

/// Is this a request to GitHub's REST API (the host whose quota is shared)?
fn is_github_api(url: &str) -> bool {
    url.strip_prefix("https://")
        .is_some_and(|r| r.split('/').next() == Some("api.github.com"))
}

const GITHUB_API: &str = "https://api.github.com";

fn nonce() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    N.fetch_add(1, Ordering::SeqCst)
}

// ---- recorded responses ----------------------------------------------------------

/// Replays a script: `[{"method": "GET", "url": "contains this", "response":
/// {"status": 200, "headers": {"ETag": "x"}, "body": {...} | "text"},
/// "repeat": true}]`. Each entry answers the first request that matches it
/// (method and a substring of the URL) and is then used up, unless it repeats.
/// A request nothing matches is a `Transport` error naming it.
pub struct Recorded {
    script: Mutex<Vec<Entry>>,
    seen: Mutex<Vec<Request>>,
}

struct Entry {
    method: String,
    url: String,
    status: Option<u16>,
    transport: Option<String>,
    headers: BTreeMap<String, String>,
    body: String,
    repeat: bool,
    used: bool,
}

impl Recorded {
    pub fn from_json(text: &str) -> Result<Recorded, String> {
        let v: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let mut script = vec![];
        for e in v.as_array().ok_or("a script is an array")? {
            let r = &e["response"];
            script.push(Entry {
                method: e["method"].as_str().unwrap_or("GET").to_string(),
                url: e["url"]
                    .as_str()
                    .ok_or("an entry needs a `url`")?
                    .to_string(),
                status: r["status"].as_u64().map(|s| s as u16),
                transport: e["transport_error"].as_str().map(String::from),
                headers: r["headers"]
                    .as_object()
                    .map(|o| {
                        o.iter()
                            .map(|(k, v)| {
                                (k.to_ascii_lowercase(), v.as_str().unwrap_or("").to_string())
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
                body: match &r["body"] {
                    serde_json::Value::String(s) => s.clone(),
                    serde_json::Value::Null => String::new(),
                    other => other.to_string(),
                },
                repeat: e["repeat"].as_bool().unwrap_or(false),
                used: false,
            });
        }
        Ok(Recorded {
            script: Mutex::new(script),
            seen: Mutex::new(vec![]),
        })
    }

    /// Every request made, in order.
    pub fn requests(&self) -> Vec<Request> {
        self.seen.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Entries never used (and not repeating): a script that expected more than happened.
    pub fn unused(&self) -> Vec<String> {
        self.script
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|e| !e.used && !e.repeat)
            .map(|e| format!("{} {}", e.method, e.url))
            .collect()
    }
}

impl Http for Recorded {
    fn send(&self, r: &Request) -> Result<Response, Transport> {
        self.seen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(r.clone());
        let mut s = self.script.lock().unwrap_or_else(|e| e.into_inner());
        let Some(e) = s
            .iter_mut()
            .find(|e| (!e.used || e.repeat) && e.method == r.method && r.url.contains(&e.url))
        else {
            return Err(Transport(format!(
                "no recorded response for {} {}",
                r.method, r.url
            )));
        };
        e.used = true;
        if let Some(t) = &e.transport {
            return Err(Transport(t.clone()));
        }
        Ok(Response {
            status: e.status.unwrap_or(200),
            headers: e
                .headers
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            body: e.body.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_header_block_is_the_answer() {
        // A 100-continue, a redirect and the answer, HTTP/1.1 then HTTP/2, as curl -D writes them.
        let head = "HTTP/1.1 100 Continue\r\n\r\n\
            HTTP/1.1 302 Found\r\nLocation: https://x.test/a\r\nX-RateLimit-Remaining: 9\r\n\r\n\
            HTTP/2 200 \r\nAccess-Control-Expose-Headers: ETag, X-RateLimit-Limit\r\n\
            X-RateLimit-Limit: 5000\r\nx-ratelimit-remaining: 3443\r\nX-RateLimit-Reset: 1795455576\r\n\
            X-RateLimit-Resource: core\r\n\r\n";
        let (status, h) = parse_head(head).unwrap();
        assert_eq!(status, 200);
        let q = github_budget::quota_of(&h).unwrap();
        assert_eq!((q.limit, q.remaining, q.reset), (5000, 3443, 1_795_455_576));
        // A 304 carries the headers in another order and no `used`.
        let (status, h) = parse_head(
            "HTTP/1.1 304 Not Modified\r\nX-RateLimit-Limit: 5000\r\nX-RateLimit-Reset: 1795455576\r\n\
             X-RateLimit-Resource: core\r\nX-RateLimit-Remaining: 3382\r\n\r\n",
        )
        .unwrap();
        assert_eq!(status, 304);
        assert_eq!(github_budget::quota_of(&h).unwrap().remaining, 3382);
        assert!(parse_head("").is_err());
    }

    #[test]
    fn only_the_github_api_host_is_budgeted() {
        assert!(is_github_api(
            "https://api.github.com/repos/a/b/actions/runs"
        ));
        assert!(is_github_api("https://api.github.com"));
        assert!(!is_github_api("http://127.0.0.1:8080/repos/a/b"));
        assert!(!is_github_api("https://api.github.com.evil.test/x"));
        assert!(!is_github_api("https://uploads.github.com/x"));
    }
}
