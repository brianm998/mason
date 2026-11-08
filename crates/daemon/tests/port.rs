// SPDX-License-Identifier: GPL-3.0-or-later

//! The daemon's port: the fixed default, `--port 0`, a taken port, and
//! `daemon.json` matching what was bound. Real `masond` processes always get
//! `--port 0` (the support code adds it), so parallel tests never meet on the
//! default; the default itself is checked by `resolve_port`, not by binding it.

mod support;
use mason_daemon::config::{parse_daemon_port, resolve_port, DEFAULT_PORT};
use serde_json::{json, Value};
use support::*;

#[test]
fn port_precedence() {
    assert_eq!(resolve_port(None, None, false), DEFAULT_PORT);
    assert_eq!(resolve_port(None, Some(9000), false), 9000);
    assert_eq!(resolve_port(Some(0), Some(9000), false), 0);
    assert_eq!(resolve_port(Some(8000), Some(9000), false), 8000);
    // A demo takes a free port unless told otherwise.
    assert_eq!(resolve_port(None, None, true), 0);
    assert_eq!(resolve_port(Some(8000), None, true), 8000);
    assert_eq!(resolve_port(None, Some(9000), true), 9000);
}

#[test]
fn daemon_toml_is_read_strictly() {
    assert_eq!(parse_daemon_port("").unwrap(), None);
    assert_eq!(
        parse_daemon_port("[daemon]\nport = 9000\n").unwrap(),
        Some(9000)
    );
    assert_eq!(parse_daemon_port("[daemon]\nport = 0\n").unwrap(), Some(0));
    for bad in [
        "[daemon]\nport = 70000\n",
        "[daemon]\nport = \"x\"\n",
        "[daemon]\nportt = 1\n",
        "[other]\nport = 1\n",
        "[daemon\n",
    ] {
        let e = parse_daemon_port(bad).unwrap_err();
        assert!(e.contains("daemon.toml"), "{e}");
    }
}

fn info(s: &Scratch) -> Value {
    serde_json::from_str(&std::fs::read_to_string(s.data_dir.join("daemon.json")).unwrap()).unwrap()
}

#[test]
fn port_zero_is_free_and_daemon_json_matches_the_bound_port() {
    let s = Scratch::new("true", happy_script());
    let mut m = s.masond(&[], &[]);
    s.wait_daemon(&mut m);
    let v = info(&s);
    let port = v["port"].as_u64().unwrap() as u16;
    assert_ne!(port, 0);
    assert_ne!(port, DEFAULT_PORT);
    assert_eq!(v["pid"].as_u64().unwrap() as u32, m.child.id());
    assert!(v["started_ms"].as_u64().unwrap() > 0);
    // Whatever answers on that port is this daemon: it is bound there.
    assert!(std::net::TcpStream::connect(("127.0.0.1", port)).is_ok());
}

#[test]
fn a_taken_port_fails_with_the_message_and_no_other_port() {
    let s = Scratch::new("true", happy_script());
    let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = held.local_addr().unwrap().port();
    let mut m = s.masond(&["--port", &port.to_string()], &[]);
    // stderr is dropped by the helper; run it again by hand to read it.
    let _ = m.child.kill();
    let _ = m.child.wait();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_masond"))
        .args(["--data-dir", &s.data_dir.display().to_string()])
        .args(["--config-dir", &s.config_dir.display().to_string()])
        .args(["--port", &port.to_string()])
        .args(["--host-bin", "/nonexistent", "--mcp-bin", "/nonexistent"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains(&format!("port {port} on 127.0.0.1 is taken by")),
        "{err}"
    );
    assert!(
        err.contains("--port") && err.contains("daemon.toml"),
        "{err}"
    );
    assert!(err.contains("does not pick another"), "{err}");
    drop(held);
}

#[test]
fn port_from_daemon_toml_applies_and_a_bad_file_refuses_to_start() {
    let s = Scratch::new("true", happy_script());
    // A free port found now and handed back: small race, fine for a test.
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    std::fs::write(
        s.config_dir.join("daemon.toml"),
        format!("[daemon]\nport = {port}\n"),
    )
    .unwrap();
    let mut m = s.masond_no_port(&[]);
    wait_for("daemon.json", 60, || {
        s.data_dir.join("daemon.json").exists()
    });
    assert_eq!(info(&s)["port"], json!(port));
    let _ = m.child.kill();
    let _ = m.child.wait();

    std::fs::write(s.config_dir.join("daemon.toml"), "[daemon]\nport = \"x\"\n").unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_masond"))
        .args(["--data-dir", &s.data_dir.display().to_string()])
        .args(["--config-dir", &s.config_dir.display().to_string()])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("daemon.toml"));
}
