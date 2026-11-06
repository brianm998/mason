// SPDX-License-Identifier: GPL-3.0-or-later

//! The seam between the service adapters and the network. Adapters speak to
//! an [`Http`]; production uses [`Curl`] (the system's `curl`, the token never
//! on a command line), tests use [`Recorded`], which replays a script of
//! responses and fails loudly on any request it was not given.

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
}

// ---- the real thing ------------------------------------------------------------

/// `curl`, with the request's headers (the token among them) passed on stdin as a
/// config file, so nothing secret appears in the process list.
pub struct Curl {
    pub timeout_s: u32,
}

impl Default for Curl {
    fn default() -> Self {
        Curl { timeout_s: 30 }
    }
}

impl Http for Curl {
    fn send(&self, r: &Request) -> Result<Response, Transport> {
        netguard::check(&r.url).map_err(|e| Transport(e.to_string()))?;
        let mut cfg = String::new();
        let q = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
        cfg.push_str(&format!(
            "url = \"{}\"\nrequest = \"{}\"\n",
            q(&r.url),
            q(&r.method)
        ));
        for (k, v) in &r.headers {
            if k.contains(['\n', '\r']) || v.contains(['\n', '\r']) {
                return Err(Transport("a header contains a newline".into()));
            }
            cfg.push_str(&format!("header = \"{}: {}\"\n", q(k), q(v)));
        }
        let hdr =
            std::env::temp_dir().join(format!("mason-curl-{}-{}", std::process::id(), nonce()));
        let mut cmd = Command::new("curl");
        cmd.args(["-sS", "--max-time", &self.timeout_s.to_string(), "-D"])
            .arg(&hdr)
            .args(["-K", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(b) = &r.body {
            cfg.push_str(&format!(
                "data-binary = \"{}\"\n",
                q(b).replace('\n', "\\n")
            ));
        }
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
        // The last block of headers (after any redirects or 100-continue).
        let block = head
            .split("\r\n\r\n")
            .filter(|b| b.starts_with("HTTP/"))
            .last()
            .unwrap_or("");
        let mut lines = block.lines();
        let status = lines
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| Transport("no status line".into()))?;
        let headers = lines
            .filter_map(|l| l.split_once(':'))
            .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
            .collect();
        Ok(Response {
            status,
            headers,
            body: String::from_utf8_lossy(&out.stdout).to_string(),
        })
    }
}

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
