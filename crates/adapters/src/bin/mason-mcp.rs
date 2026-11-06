// SPDX-License-Identifier: GPL-3.0-or-later

//! MCP stdio server exposing the step contract's tools to an agent CLI.
//!
//! Environment: `MASON_STEP_KIND` (explore|develop|review) picks the tools,
//! `MASON_CONTRACT_SOCK` is where calls are forwarded. If the controller is
//! briefly away (a daemon restart), calls retry for `MASON_BRIDGE_RETRY_SECS`
//! (default 30) before failing, so the agent is not disturbed.

use mason_adapter_api::contract::{tool_spec, StepKind};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

fn send(v: &Value) {
    let mut o = std::io::stdout().lock();
    let _ = writeln!(o, "{v}");
    let _ = o.flush();
}

fn forward(
    sock: &str,
    tool: &str,
    args: &Value,
    retry: Duration,
) -> Result<(bool, String), String> {
    let end = Instant::now() + retry;
    loop {
        match UnixStream::connect(sock) {
            Ok(mut s) => {
                writeln!(s, "{}", json!({"tool": tool, "args": args}))
                    .map_err(|e| e.to_string())?;
                let mut line = String::new();
                BufReader::new(s)
                    .read_line(&mut line)
                    .map_err(|e| e.to_string())?;
                let v: Value =
                    serde_json::from_str(&line).map_err(|e| format!("bad reply: {e}"))?;
                return Ok((
                    v["ok"].as_bool().unwrap_or(false),
                    v["text"].as_str().unwrap_or_default().to_string(),
                ));
            }
            Err(e) if Instant::now() >= end => {
                return Err(format!("pipeline controller unavailable: {e}"))
            }
            Err(_) => std::thread::sleep(Duration::from_millis(200)),
        }
    }
}

fn main() {
    let step = std::env::var("MASON_STEP_KIND")
        .ok()
        .and_then(|s| StepKind::from_name(&s));
    let sock = std::env::var("MASON_CONTRACT_SOCK").unwrap_or_default();
    let retry = Duration::from_secs(
        std::env::var("MASON_BRIDGE_RETRY_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(30),
    );
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let Ok(m) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let id = m.get("id").cloned();
        let reply = |result: Value| {
            if let Some(id) = &id {
                send(&json!({"jsonrpc": "2.0", "id": id, "result": result}));
            }
        };
        match m["method"].as_str() {
            Some("initialize") => reply(json!({
                "protocolVersion": m["params"]["protocolVersion"],
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "mason", "version": env!("CARGO_PKG_VERSION")}
            })),
            Some("tools/list") => {
                let tools: Vec<Value> = step
                    .map(|s| s.tools().to_vec())
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|t| {
                        tool_spec(t).map(|(d, schema)| json!({"name": t, "description": d, "inputSchema": schema}))
                    })
                    .collect();
                reply(json!({"tools": tools}));
            }
            Some("tools/call") => {
                let name = m["params"]["name"].as_str().unwrap_or_default();
                let args = m["params"].get("arguments").cloned().unwrap_or(json!({}));
                let (ok, text) = match forward(&sock, name, &args, retry) {
                    Ok(r) => r,
                    Err(e) => (false, e),
                };
                reply(json!({"content": [{"type": "text", "text": text}], "isError": !ok}));
            }
            Some("ping") => reply(json!({})),
            Some(_) => {
                if let Some(id) = &id {
                    send(
                        &json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "no such method"}}),
                    );
                }
            }
            None => {}
        }
    }
}
