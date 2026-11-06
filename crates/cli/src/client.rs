// SPDX-License-Identifier: GPL-3.0-or-later

//! A minimal HTTP/1.1 client for the daemon's loopback API: one request per
//! connection, JSON in and out. (Stage 11 moves the CLI onto the same
//! WebSocket protocol the monitors use.)

use serde_json::Value;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::time::Duration;

pub struct Daemon {
    pub port: u16,
}

pub enum CallError {
    /// No daemon is running (or its record is stale).
    NotRunning(String),
    Io(String),
    /// The daemon answered with an error.
    Api(u16, String),
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CallError::NotRunning(m) | CallError::Io(m) => write!(f, "{m}"),
            CallError::Api(code, m) => write!(f, "{m} (HTTP {code})"),
        }
    }
}

pub fn find(data_dir: &Path) -> Result<Daemon, CallError> {
    let info = data_dir.join("daemon.json");
    let text = std::fs::read_to_string(&info).map_err(|_| {
        CallError::NotRunning(format!(
            "masond is not running (no {} yet). Start it with `masond`.",
            info.display()
        ))
    })?;
    let v: Value = serde_json::from_str(&text)
        .map_err(|e| CallError::Io(format!("{}: {e}", info.display())))?;
    let pid = v["pid"].as_u64().unwrap_or(0) as u32;
    if !mason_step_host::registry::pid_alive(pid) {
        return Err(CallError::NotRunning(format!(
            "masond is not running (pid {pid} from {} is gone). Start it with `masond`.",
            info.display()
        )));
    }
    Ok(Daemon {
        port: v["port"].as_u64().unwrap_or(0) as u16,
    })
}

impl Daemon {
    pub fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value, CallError> {
        let mut s = TcpStream::connect(("127.0.0.1", self.port)).map_err(|e| {
            CallError::NotRunning(format!(
                "cannot reach masond on 127.0.0.1:{}: {e}",
                self.port
            ))
        })?;
        s.set_read_timeout(Some(Duration::from_secs(120))).ok();
        let payload = body.map(|b| b.to_string()).unwrap_or_default();
        let req = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Type: application/json\r\n{}Content-Length: {}\r\n\r\n{payload}",
            // Who is acting and why, for the record of interventions (Part 10).
            ["MASON_AS", "MASON_REASON"]
                .iter()
                .filter_map(|k| {
                    let v = std::env::var(k).ok()?.replace(['\r', '\n'], " ");
                    Some(format!("X-Mason-{}: {v}\r\n", if *k == "MASON_AS" { "As" } else { "Reason" }))
                })
                .collect::<String>(),
            payload.len()
        );
        s.write_all(req.as_bytes())
            .map_err(|e| CallError::Io(e.to_string()))?;
        let mut raw = Vec::new();
        s.read_to_end(&mut raw)
            .map_err(|e| CallError::Io(e.to_string()))?;
        let text = String::from_utf8_lossy(&raw).to_string();
        let (head, body) = text
            .split_once("\r\n\r\n")
            .ok_or_else(|| CallError::Io("malformed response".into()))?;
        let code: u16 = head
            .split_whitespace()
            .nth(1)
            .and_then(|c| c.parse().ok())
            .ok_or_else(|| CallError::Io("malformed status line".into()))?;
        let body = if head.to_lowercase().contains("transfer-encoding: chunked") {
            dechunk(body)
        } else {
            body.to_string()
        };
        let v: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
        if code >= 400 {
            return Err(CallError::Api(
                code,
                v["error"].as_str().unwrap_or("request failed").to_string(),
            ));
        }
        Ok(v)
    }
}

fn dechunk(body: &str) -> String {
    let mut out = String::new();
    let mut rest = body;
    while let Some((len, tail)) = rest.split_once("\r\n") {
        let n = usize::from_str_radix(len.trim(), 16).unwrap_or(0);
        if n == 0 || tail.len() < n {
            break;
        }
        out.push_str(&tail[..n]);
        rest = tail[n..].trim_start_matches("\r\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::dechunk;

    #[test]
    fn chunked_bodies_are_reassembled() {
        assert_eq!(
            dechunk("5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n"),
            "hello world"
        );
        assert_eq!(dechunk(""), "");
    }
}
