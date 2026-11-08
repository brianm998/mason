// SPDX-License-Identifier: GPL-3.0-or-later

//! A minimal HTTP/1.1 client for the daemon's loopback API: one request per
//! connection, JSON in and out. (Stage 11 moves the CLI onto the same
//! WebSocket protocol the monitors use.)

use serde_json::Value;
use std::path::Path;
use std::time::Duration;

pub struct Daemon {
    pub port: u16,
    data_dir: std::path::PathBuf,
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
        data_dir: data_dir.to_path_buf(),
    })
}

impl Daemon {
    /// One call, over the same WebSocket protocol the monitors use (Part 16):
    /// connect, say hello asking for no views, send the command, read its
    /// result. Everything a monitor can do goes through here.
    pub fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value, CallError> {
        use mason_protocol::{command, Command, Encoding, Raw};
        let mut c = match mason_protocol::client::Client::connect(self.port, Encoding::Json) {
            Ok(c) => c,
            Err(first) => {
                // The daemon may have restarted between reading daemon.json and
                // connecting: read it again, once, and try the new address.
                std::thread::sleep(Duration::from_millis(200));
                let again = find(&self.data_dir)?;
                mason_protocol::client::Client::connect(again.port, Encoding::Json).map_err(
                    |second| {
                        CallError::NotRunning(if again.port == self.port {
                            first
                        } else {
                            second
                        })
                    },
                )?
            }
        };
        c.hello(0, true).map_err(CallError::Io)?;
        let cmd = Command {
            id: String::new(),
            body: Some(command::Body::Raw(Raw {
                method: method.into(),
                path: path.into(),
                json: body.map(|b| b.to_string()).unwrap_or_default(),
            })),
            // Who is acting and why, for the record of interventions (Part 10).
            actor: std::env::var("MASON_AS").unwrap_or_default(),
            reason: std::env::var("MASON_REASON").unwrap_or_default(),
        };
        let (r, _) = c
            .command(cmd, Duration::from_secs(120))
            .map_err(CallError::Io)?;
        c.close();
        if !r.ok {
            return Err(CallError::Api(
                r.status as u16,
                if r.error.is_empty() {
                    "request failed".into()
                } else {
                    r.error
                },
            ));
        }
        Ok(serde_json::from_str(&r.json).unwrap_or(Value::Null))
    }
}
