// SPDX-License-Identifier: GPL-3.0-or-later

//! A scripted stand-in for the `codex` CLI, the counterpart of `fake-claude`.
//! It reads a prompt on stdin, plays one turn of the script named by
//! `FAKE_CODEX_SCRIPT` and prints `codex exec --json` events. Tool calls go to
//! the contract socket in `MASON_CONTRACT_SOCK`, where the MCP bridge would send
//! them. The script format is `fake-claude`'s (`{"turns": [[action, ...]]}` or
//! `{"steps": {"explore": [[...]]}}`, invocation N plays turn N), with these
//! actions: `{"replay": "<fixture>"}`, `{"say": "..."}`,
//! `{"call": {"tool": "...", "args": {...}}}`, `{"api_retry": {"attempt": 1}}`,
//! `{"sleep_ms": 100}`, `{"result": {"is_error": false, "text": "..."}}`,
//! `{"stderr": "..."}`, `{"exit": 1}` and `{"sh": "..."}` (run in the working
//! directory). `"$HEAD"` in call arguments becomes `git rev-parse HEAD`. A
//! turn with no `result` or `exit` gets a normal successful result. The model
//! and sandbox it was started with are logged to `<script>.argv` for tests.

use mason_platform::net::UnixStream;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};

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
        .position(|a| a == "resume")
        .and_then(|i| args.get(i + 1))
        .cloned();
    let mut prompt = String::new();
    let _ = std::io::stdin().read_to_string(&mut prompt);

    let script_path = std::env::var("FAKE_CODEX_SCRIPT").expect("FAKE_CODEX_SCRIPT");
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
    let by_step = script.get("steps").is_some();
    let count_path = if by_step {
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
    let turns = if by_step {
        &script["steps"][kind.as_str()]
    } else {
        &script["turns"]
    };
    let turn = turns.get(n).and_then(Value::as_array).unwrap_or(&empty);

    let sid = resume.unwrap_or_else(|| "fake-codex-thread".to_string());
    let mut inited = false;
    let mut finished = false;
    let mut items = 0u32;
    let init = |sid: &str| {
        out(&json!({"type": "thread.started", "thread_id": sid}));
        out(&json!({"type": "turn.started"}));
    };
    let done_ok = || {
        out(&json!({"type": "turn.completed",
            "usage": {"input_tokens": 120, "cached_input_tokens": 100, "output_tokens": 8}}));
    };
    for a in turn {
        if !inited && a.get("replay").is_none() && a.get("exit").is_none() {
            init(&sid);
            inited = true;
        }
        if let Some(path) = a.get("replay").and_then(Value::as_str) {
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
            items += 1;
            out(
                &json!({"type": "item.completed", "item": {"id": format!("item_{items}"),
                "type": "agent_message", "text": t}}),
            );
        } else if let Some(c) = a.get("call") {
            items += 1;
            let tool = c["tool"].as_str().unwrap_or_default();
            let cargs = subst(&c["args"]);
            let (ok, _text) = call(tool, &cargs);
            out(
                &json!({"type": "item.completed", "item": {"id": format!("item_{items}"),
                "type": "mcp_tool_call", "server": "mason", "tool": tool, "arguments": cargs,
                "status": if ok { "completed" } else { "failed" }}}),
            );
        } else if let Some(r) = a.get("api_retry") {
            out(&json!({"type": "error",
                "message": format!("Reconnecting... {}/5", r["attempt"])}));
        } else if let Some(cmd) = a.get("sh").and_then(Value::as_str) {
            let st = std::process::Command::new("sh").args(["-c", cmd]).status();
            items += 1;
            out(
                &json!({"type": "item.completed", "item": {"id": format!("item_{items}"),
                "type": "command_execution", "command": cmd,
                "exit_code": st.as_ref().map_or(-1, |s| s.code().unwrap_or(-1)),
                "status": "completed"}}),
            );
        } else if let Some(ms) = a.get("sleep_ms").and_then(Value::as_u64) {
            std::thread::sleep(std::time::Duration::from_millis(ms));
        } else if let Some(s) = a.get("stderr").and_then(Value::as_str) {
            eprintln!("{s}");
        } else if let Some(code) = a.get("exit").and_then(Value::as_i64) {
            std::process::exit(code as i32);
        } else if let Some(r) = a.get("result") {
            if r["is_error"].as_bool().unwrap_or(false) {
                let m = r["text"].as_str().unwrap_or("the turn failed");
                out(&json!({"type": "error", "message": m}));
                out(&json!({"type": "turn.failed", "error": {"message": m}}));
            } else {
                done_ok();
            }
            finished = true;
        }
    }
    if !finished {
        if !inited {
            init(&sid);
        }
        done_ok();
    }
}
