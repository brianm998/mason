// SPDX-License-Identifier: GPL-3.0-or-later

//! The MCP bridge, spoken to as an agent CLI would: JSON-RPC over stdio.

use mason_adapter_api::conformance::valid_plan;
use mason_adapter_api::contract::{ContractHandle, NoChecks};
use mason_adapter_api::StepKind;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

struct Bridge {
    child: Child,
    stdin: ChildStdin,
    out: BufReader<ChildStdout>,
    next: u64,
}

impl Bridge {
    fn start(step: &str, sock: &std::path::Path, retry_secs: u32) -> Bridge {
        Self::start_with(step, sock, retry_secs, &[])
    }

    fn start_with(
        step: &str,
        sock: &std::path::Path,
        retry_secs: u32,
        env: &[(&str, &str)],
    ) -> Bridge {
        let mut child = Command::new(env!("CARGO_BIN_EXE_mason-mcp"))
            .envs(env.iter().copied())
            .env("MASON_STEP_KIND", step)
            .env("MASON_CONTRACT_SOCK", sock)
            .env("MASON_BRIDGE_RETRY_SECS", retry_secs.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let out = BufReader::new(child.stdout.take().unwrap());
        let mut b = Bridge {
            child,
            stdin,
            out,
            next: 1,
        };
        let r = b.rpc("initialize", json!({"protocolVersion": "2025-06-18"}));
        assert_eq!(r["result"]["serverInfo"]["name"], "mason");
        writeln!(
            b.stdin,
            "{}",
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"})
        )
        .unwrap();
        b
    }

    fn rpc(&mut self, method: &str, params: Value) -> Value {
        let id = self.next;
        self.next += 1;
        writeln!(
            self.stdin,
            "{}",
            json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
        )
        .unwrap();
        let mut line = String::new();
        self.out.read_line(&mut line).unwrap();
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["id"], id);
        v
    }

    fn tool(&mut self, name: &str, args: Value) -> (bool, String) {
        let r = self.rpc("tools/call", json!({"name": name, "arguments": args}));
        (
            r["result"]["isError"].as_bool().unwrap(),
            r["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .to_string(),
        )
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn sock(d: &tempfile::TempDir) -> std::path::PathBuf {
    d.path().join("c.sock")
}

#[test]
fn each_step_lists_exactly_its_own_tools_with_schemas() {
    let d = tempfile::tempdir().unwrap();
    for (step, kind) in [
        ("explore", StepKind::Explore),
        ("develop", StepKind::Develop),
        ("review", StepKind::Review),
    ] {
        let mut b = Bridge::start(step, &sock(&d), 1);
        let r = b.rpc("tools/list", json!({}));
        let mut names: Vec<String> = r["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect();
        let mut want: Vec<String> = kind.tools().iter().map(|s| s.to_string()).collect();
        names.sort();
        want.sort();
        assert_eq!(names, want, "{step}");
        for t in r["result"]["tools"].as_array().unwrap() {
            assert_eq!(t["inputSchema"]["type"], "object");
            assert!(!t["description"].as_str().unwrap().is_empty());
        }
    }
}

#[test]
fn calls_are_forwarded_and_rejections_come_back_as_tool_errors() {
    let d = tempfile::tempdir().unwrap();
    let c = ContractHandle::new(StepKind::Explore, Box::new(NoChecks));
    c.serve(&sock(&d)).unwrap();
    let mut b = Bridge::start("explore", &sock(&d), 1);
    let (err, text) = b.tool("submit_plan", json!({"summary": ""}));
    assert!(
        err && text.contains("summary: must not be empty") && text.contains("approach: required"),
        "{text}"
    );
    assert!(!c.flag().is_set());
    let (err, text) = b.tool("submit_plan", valid_plan());
    assert!(!err, "{text}");
    assert_eq!(text, "accepted");
    assert!(c.flag().is_set());
}

#[test]
fn the_bridge_rides_out_a_controller_restart() {
    let d = tempfile::tempdir().unwrap();
    let path = sock(&d);
    let mut b = Bridge::start("explore", &path, 10);
    // The controller is away when the agent calls; it comes back 600 ms later.
    let p2 = path.clone();
    let t = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(600));
        let c = ContractHandle::new(StepKind::Explore, Box::new(NoChecks));
        c.serve(&p2).unwrap();
        std::thread::sleep(Duration::from_secs(3));
        drop(c);
    });
    let start = Instant::now();
    let (err, text) = b.tool("submit_plan", valid_plan());
    assert!(!err && text == "accepted", "{text}");
    assert!(
        start.elapsed() >= Duration::from_millis(500),
        "it waited for the controller"
    );
    t.join().unwrap();
}

#[test]
fn a_controller_that_never_returns_is_reported_not_hung() {
    let d = tempfile::tempdir().unwrap();
    let mut b = Bridge::start("explore", &sock(&d), 1);
    let start = Instant::now();
    let (err, text) = b.tool("submit_plan", valid_plan());
    assert!(
        err && text.contains("pipeline controller unavailable"),
        "{text}"
    );
    assert!(start.elapsed() < Duration::from_secs(5));
}

#[test]
fn a_definition_can_narrow_the_tools_the_bridge_offers() {
    let d = tempfile::tempdir().unwrap();
    let mut b = Bridge::start_with(
        "develop",
        &sock(&d),
        1,
        &[("MASON_TOOLS", "submit_change,report_progress")],
    );
    let r = b.rpc("tools/list", json!({}));
    let mut names: Vec<&str> = r["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    names.sort();
    assert_eq!(names, vec!["report_progress", "submit_change"]);
}

#[cfg(unix)] // Unix-only; Windows coverage comes with the later port tasks.
#[test]
fn a_check_is_sent_again_when_the_controller_goes_away_before_answering() {
    use std::os::unix::net::UnixListener;
    let d = tempfile::tempdir().unwrap();
    let path = sock(&d);
    let listener = UnixListener::bind(&path).unwrap();
    let server = std::thread::spawn(move || {
        // The daemon restarts mid-check: the call is read and never answered.
        let (first, _) = listener.accept().unwrap();
        let mut line = String::new();
        BufReader::new(&first).read_line(&mut line).unwrap();
        assert!(line.contains("run_check"), "{line}");
        drop(first);
        // Back up: the same call arrives again, and this time it is answered.
        let (mut again, _) = listener.accept().unwrap();
        let mut line = String::new();
        BufReader::new(&again).read_line(&mut line).unwrap();
        assert!(line.contains("run_check"), "{line}");
        writeln!(
            again,
            "{}",
            json!({"ok": true, "text": "check tests: PASSED"})
        )
        .unwrap();
    });
    let mut b = Bridge::start("develop", &path, 5);
    let (is_err, text) = b.tool("run_check", json!({"name": "tests"}));
    assert!(!is_err, "{text}");
    assert!(text.contains("PASSED"), "{text}");
    server.join().unwrap();
}
