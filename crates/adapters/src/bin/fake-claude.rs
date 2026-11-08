// SPDX-License-Identifier: GPL-3.0-or-later

//! A scripted stand-in for the `claude` CLI. It reads a prompt on stdin,
//! plays one turn of the script named by `FAKE_CLAUDE_SCRIPT`, and prints
//! `stream-json` lines in the real CLI's format. Tool calls go to the contract
//! socket in `MASON_CONTRACT_SOCK`, exactly where the MCP bridge would send them.
//!
//! The script is `{"turns": [[action, ...], ...]}` or, to give each kind of
//! step its own, `{"steps": {"explore": [[...]], "develop": [[...]]}}` (the kind
//! comes from `MASON_STEP_KIND`). Invocation N, counted per step kind and
//! working directory in `<script>.<kind>.<dir hash>.count`, plays turn N, so a
//! resume plays the next turn. Actions:
//! `{"replay": "<fixture path>"}`, `{"init": "<session id>"}`, `{"say": "..."}`,
//! `{"call": {"tool": "...", "args": {...}}}`, `{"rate_limit": {...}}`,
//! `{"api_retry": {"attempt": 1, "status": 529}}`, `{"sleep_ms": 100}`,
//! `{"result": {"is_error": false, "text": "...", "api_error_status": null}}`,
//! `{"stderr": "..."}`, `{"exit": 1}` (die with no result) and
//! `{"sh": "..."}` (run a shell command in the working directory, so the
//! "agent" can really edit and commit; the prompt is in `$FAKE_PROMPT`). In a call's arguments the string
//! `"$HEAD"` is replaced by `git rev-parse HEAD`.
//! A turn with no `result` or `exit` gets a normal successful result.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;

fn out(v: &Value) {
    let mut o = std::io::stdout().lock();
    let _ = writeln!(o, "{v}");
    let _ = o.flush();
}

fn head() -> String {
    let o = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output();
    o.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

fn subst(v: &Value) -> Value {
    match v {
        Value::String(s) if s == "$HEAD" => Value::String(head()),
        Value::Array(a) => Value::Array(a.iter().map(subst).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, v)| (k.clone(), subst(v))).collect()),
        other => other.clone(),
    }
}

/// Like the MCP bridge: ride out a controller that is briefly away.
fn connect() -> Option<UnixStream> {
    let sock = std::env::var("MASON_CONTRACT_SOCK").ok()?;
    let end = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        match UnixStream::connect(&sock) {
            Ok(s) => return Some(s),
            Err(_) if std::time::Instant::now() >= end => return None,
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(100)),
        }
    }
}

fn call(tool: &str, args: &Value) -> (bool, String) {
    let Some(mut s) = connect() else {
        return (false, "contract unavailable".into());
    };
    let _ = writeln!(s, "{}", json!({"tool": tool, "args": args}));
    let mut line = String::new();
    let _ = BufReader::new(s).read_line(&mut line);
    let v: Value = serde_json::from_str(&line).unwrap_or(json!({"ok": false, "text": "bad reply"}));
    (
        v["ok"].as_bool().unwrap_or(false),
        v["text"].as_str().unwrap_or_default().to_string(),
    )
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let resume = args
        .iter()
        .position(|a| a == "--resume")
        .and_then(|i| args.get(i + 1))
        .cloned();
    let mut prompt = String::new();
    let _ = std::io::stdin().read_to_string(&mut prompt);

    let script_path = std::env::var("FAKE_CLAUDE_SCRIPT").expect("FAKE_CLAUDE_SCRIPT");
    // The arguments it was started with, one line per start, for tests.
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(format!("{script_path}.argv"))
    {
        let _ = writeln!(f, "{}", args[1..].join(" "));
    }
    let script: Value =
        serde_json::from_str(&std::fs::read_to_string(&script_path).expect("script"))
            .expect("json");
    let kind = std::env::var("MASON_STEP_KIND").unwrap_or_default();
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let h = cwd
        .bytes()
        .fold(0u64, |a, b| a.wrapping_mul(31).wrapping_add(u64::from(b)));
    let count_path = if script.get("steps").is_some() {
        format!("{script_path}.{kind}.{h:x}.count")
    } else {
        format!("{script_path}.count")
    };
    let n: usize = std::fs::read_to_string(&count_path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);
    let _ = std::fs::write(&count_path, (n + 1).to_string());
    let empty = vec![];
    let turns = if script.get("steps").is_some() {
        &script["steps"][kind.as_str()]
    } else {
        &script["turns"]
    };
    let turn = turns.get(n).and_then(Value::as_array).unwrap_or(&empty);

    let sid = resume.unwrap_or_else(|| "fake-claude-session".to_string());
    // Like the real CLI, report the model asked for (`--model`), else a stand-in.
    let reported = args
        .iter()
        .position(|a| a == "--model")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| "claude-fake".to_string());
    let mut inited = false;
    let mut finished = false;
    let mut calls = 0u32;
    let init = |sid: &str| {
        out(
            &json!({"type": "system", "subtype": "init", "cwd": "/scratch", "session_id": sid,
                    "tools": [], "mcp_servers": [{"name": "mason", "status": "connected"}],
                    "model": reported, "permissionMode": "dontAsk", "claude_code_version": "0.0.0-fake"}),
        );
    };
    for a in turn {
        if !inited
            && a.get("init").is_none()
            && a.get("replay").is_none()
            && a.get("exit").is_none()
        {
            init(&sid);
            inited = true;
        }
        if let Some(id) = a.get("init").and_then(Value::as_str) {
            init(id);
            inited = true;
        } else if let Some(path) = a.get("replay").and_then(Value::as_str) {
            for l in BufReader::new(std::fs::File::open(path).expect("fixture"))
                .lines()
                .map_while(Result::ok)
            {
                let mut m = std::io::stdout().lock();
                let _ = writeln!(m, "{l}");
            }
            inited = true;
            finished = true;
        } else if let Some(t) = a.get("say").and_then(Value::as_str) {
            out(
                &json!({"type": "assistant", "session_id": sid, "message": {"role": "assistant",
                "content": [{"type": "text", "text": t}]}}),
            );
        } else if let Some(c) = a.get("call") {
            calls += 1;
            let tool = c["tool"].as_str().unwrap_or_default();
            let cargs = subst(&c["args"]);
            let tid = format!("toolu_fake_{calls}");
            out(
                &json!({"type": "assistant", "session_id": sid, "message": {"role": "assistant",
                "content": [{"type": "tool_use", "id": tid, "name": format!("mcp__mason__{tool}"), "input": cargs}]}}),
            );
            let (ok, text) = call(tool, &cargs);
            out(
                &json!({"type": "user", "session_id": sid, "message": {"role": "user", "content": [
                {"tool_use_id": tid, "type": "tool_result", "is_error": !ok, "content": [{"type": "text", "text": text}]}]}}),
            );
        } else if let Some(r) = a.get("rate_limit") {
            out(
                &json!({"type": "rate_limit_event", "session_id": sid, "rate_limit_info": {
                "status": r["status"], "resetsAt": r["resetsAt"], "rateLimitType": r["window"],
                "unifiedWindows": {r["window"].as_str().unwrap_or("five_hour"): {"utilization": r["utilization"]}}}}),
            );
        } else if let Some(r) = a.get("api_retry") {
            out(
                &json!({"type": "system", "subtype": "api_retry", "session_id": sid, "attempt": r["attempt"],
                "max_retries": 10, "retry_delay_ms": 1, "error_status": r["status"], "error": "overloaded_error"}),
            );
        } else if let Some(cmd) = a.get("sh").and_then(Value::as_str) {
            // The prompt is handed to the command, so a scripted "agent" can read what
            // it was told (its reserved numbers, say) the way a real one would.
            let st = std::process::Command::new("sh")
                .args(["-c", cmd])
                .env("FAKE_PROMPT", &prompt)
                .status();
            if !st.map(|s| s.success()).unwrap_or(false) {
                eprintln!("fake-claude: sh action failed: {cmd}");
            }
        } else if let Some(ms) = a.get("sleep_ms").and_then(Value::as_u64) {
            std::thread::sleep(std::time::Duration::from_millis(ms));
        } else if let Some(s) = a.get("stderr").and_then(Value::as_str) {
            eprintln!("{s}");
        } else if let Some(code) = a.get("exit").and_then(Value::as_i64) {
            std::process::exit(code as i32);
        } else if let Some(r) = a.get("result") {
            let is_error = r["is_error"].as_bool().unwrap_or(false);
            out(
                &json!({"type": "result", "subtype": "success", "is_error": is_error, "duration_ms": 1,
                "num_turns": 1, "result": r["text"], "api_error_status": r["api_error_status"],
                "session_id": sid, "total_cost_usd": 0.001,
                "usage": {"input_tokens": 4, "cache_creation_input_tokens": 100, "cache_read_input_tokens": 2000, "output_tokens": 50}}),
            );
            finished = true;
        }
    }
    if !finished {
        if !inited {
            init(&sid);
        }
        out(
            &json!({"type": "result", "subtype": "success", "is_error": false, "duration_ms": 1, "num_turns": 1,
            "result": "done", "api_error_status": null, "session_id": sid, "total_cost_usd": 0.001,
            "usage": {"input_tokens": 4, "cache_creation_input_tokens": 100, "cache_read_input_tokens": 2000, "output_tokens": 50}}),
        );
    }
}
