// SPDX-License-Identifier: GPL-3.0-or-later

//! Stage 16 against the real `masond`: an update while an agent session runs,
//! a build that crashes, one that fails its self-test, and the rule against
//! going backwards. The "new code" is the same binaries copied into a runtime of
//! their own, so what is tested is the machinery, not a code difference.

mod support;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use support::*;

const BUILD: &str = r#"mkdir -p target/release
for b in masond step-host mason-mcp fake-claude; do cp "$BINDIR/$b" target/release/$b; done
if [ -f crash ]; then
  printf '#!/bin/sh\n[ "$1" = selftest ] && exec "%s/masond" "$@"\nexit 1\n' "$BINDIR" > target/release/masond
  chmod +x target/release/masond
fi
if [ -f selftest-fails ]; then
  printf '#!/bin/sh\n[ "$1" = selftest ] && { echo "migration 99 failed" >&2; exit 1; }\nexec "%s/masond" "$@"\n' "$BINDIR" > target/release/masond
  chmod +x target/release/masond
fi
"#;

fn bindir() -> PathBuf {
    let m = bin("mason-daemon", "masond");
    bin("mason-step-host", "step-host");
    bin("mason-adapters", "mason-mcp");
    bin("mason-adapters", "fake-claude");
    m.parent().unwrap().to_path_buf()
}

/// A source repository with commits c1, c2, ... (c1 is the oldest), and `update.toml`.
struct Src {
    shas: Vec<String>,
}

fn source(s: &Scratch, files: &[&[(&str, &str)]]) -> Src {
    let repo = s.root.path().join("source");
    std::fs::create_dir_all(&repo).unwrap();
    sh(&repo, &["init", "-q", "-b", "main"]);
    let mut shas = vec![];
    for (i, fs) in files.iter().enumerate() {
        std::fs::write(repo.join("version"), format!("c{}", i + 1)).unwrap();
        for (p, t) in *fs {
            std::fs::write(repo.join(p), t).unwrap();
        }
        sh(&repo, &["add", "-A"]);
        sh(
            &repo,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-qm",
                &format!("c{}", i + 1),
            ],
        );
        shas.push(sh(&repo, &["rev-parse", "HEAD"]));
    }
    std::fs::write(
        s.config_dir.join("update.toml"),
        format!(
            "repo = \"{}\"\nchannel = \"main\"\nbuild = {:?}\nbins = [\"masond\", \"step-host\", \"mason-mcp\", \"fake-claude\"]\n",
            repo.display(),
            format!("BINDIR='{}'\n{BUILD}", bindir().display())
        ),
    )
    .unwrap();
    Src { shas }
}

fn status(s: &Scratch) -> Value {
    serde_json::from_str(&s.mason_ok(&["update", "status", "--json"])).unwrap()
}

fn daemon_pid(s: &Scratch) -> u64 {
    let t = std::fs::read_to_string(s.data_dir.join("daemon.json")).unwrap();
    serde_json::from_str::<Value>(&t).unwrap()["pid"]
        .as_u64()
        .unwrap()
}

fn slow_script(ms: u64) -> Value {
    json!({"steps": {"explore": [explore_turn()], "develop": [[
        {"sh": TASK_SCRIPT_COMMIT},
        {"sleep_ms": ms},
        {"call": {"tool": "submit_change", "args": {"summary": "done after a wait", "head": "$HEAD"}}}
    ]]}})
}

fn running_hosts(s: &Scratch) -> Vec<(String, u32)> {
    mason_step_host::registry::Registry::new(s.data_dir.join("h"))
        .scan()
        .unwrap_or_default()
        .into_iter()
        .filter(|h| h.state == mason_step_host::registry::HostState::Running)
        .map(|h| (h.info.id.clone(), h.info.host_pid))
        .collect()
}

fn wait_status(s: &Scratch, what: &str, secs: u64, f: impl Fn(&Value) -> bool) -> Value {
    let end = Instant::now() + Duration::from_secs(secs);
    loop {
        let o = s.mason(&["update", "status", "--json"]);
        if let Ok(v) = serde_json::from_slice::<Value>(&o.stdout) {
            if f(&v) {
                return v;
            }
        }
        assert!(Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn start(s: &Scratch) -> Masond {
    let mut m = s.masond(&["--managed", "--healthy-after-ms", "1500"], &[]);
    s.wait_daemon(&mut m);
    m
}

#[test]
fn an_update_while_an_agent_session_runs_does_not_interrupt_it() {
    let s = Scratch::new("test -f hello.txt", slow_script(25_000));
    let src = source(&s, &[&[]]);
    let mut m = start(&s);
    s.mason_ok(&[
        "task",
        "add",
        "Slow work",
        "--body",
        "Create hello.txt.",
        "--explored",
    ]);
    // The agent is mid-session: its step host is running.
    wait_for("the agent's host to run", 60, || {
        !running_hosts(&s).is_empty()
    });
    let (host_before, pid_before) = (running_hosts(&s)[0].clone(), daemon_pid(&s));
    std::thread::sleep(Duration::from_secs(1));

    let out = s.mason_ok(&["update"]);
    assert!(
        out.contains("taking over") && out.contains(&src.shas[0]),
        "{out}"
    );
    let v = wait_status(&s, "the new runtime to answer", 60, |v| {
        v["running"] == src.shas[0].as_str()
    });
    assert_eq!(v["current"], src.shas[0].as_str());
    // Same process (exec keeps the pid) and the very same step host: the session was not touched.
    assert_eq!(daemon_pid(&s), pid_before, "exec keeps the pid");
    assert_eq!(
        running_hosts(&s).first().cloned(),
        Some(host_before.clone()),
        "the agent's host is the same process"
    );
    // The task carries on and lands, once, with one attempt at developing.
    let end = Instant::now() + Duration::from_secs(120);
    loop {
        let t = s.task_json(0);
        if t["state"] == "landed" {
            break;
        }
        assert!(t["state"] != "failed", "{t:#}");
        assert!(
            Instant::now() < end,
            "the task never landed after the update: {t:#}"
        );
        std::thread::sleep(Duration::from_millis(300));
    }
    let log = s.origin_log();
    assert_eq!(
        log.iter().filter(|m| *m == "add hello").count(),
        1,
        "{log:?}"
    );
    let t = s.task_json(0);
    assert_eq!(
        t["attempts"]["developing"].as_u64().unwrap_or(1),
        1,
        "the session was not restarted: {t:#}"
    );
    // The database was backed up before the takeover, and the build left its log.
    let backups: Vec<_> = std::fs::read_dir(s.data_dir.join("runtimes/backups"))
        .unwrap()
        .flatten()
        .collect();
    assert!(
        backups
            .iter()
            .any(|b| b.file_name().to_string_lossy().starts_with("store-")),
        "{backups:?}"
    );
    drop(m.child.kill());
    kill_hosts(&s.data_dir);
}

#[test]
fn a_build_that_crashes_is_rolled_back_by_itself_with_an_alert() {
    let s = Scratch::new("true", json!({"steps": {}}));
    let src = source(&s, &[&[], &[("crash", "x")]]);
    let mut m = start(&s);
    s.mason_ok(&["update", "--sha", &src.shas[0]]);
    wait_status(&s, "c1 to run", 90, |v| {
        v["running"] == src.shas[0].as_str()
    });
    // Healthy long enough to be recorded as such.
    std::thread::sleep(Duration::from_secs(3));
    // c2 builds and passes its self-test, then dies the moment it starts.
    // (The command line gives up waiting for a daemon that does not come back, and says so.)
    let o = s.mason(&["update", "--sha", &src.shas[1]]);
    assert!(
        String::from_utf8_lossy(&o.stderr).contains("has not come back"),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let end = Instant::now() + Duration::from_secs(60);
    while m.child.try_wait().unwrap().is_none() {
        assert!(
            Instant::now() < end,
            "the crashing build never took over (and died)"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    // The service manager restarts the daemon, again and again, as launchd or systemd would.
    let mut tries = 0;
    let mut alive: Option<std::process::Child> = None;
    while alive.is_none() {
        tries += 1;
        assert!(tries <= 8, "no daemon came up after {tries} restarts");
        let cfg = s.config();
        let mut c = Command::new(env!("CARGO_BIN_EXE_masond"))
            .args(["--managed", "--healthy-after-ms", "1500"])
            // A free port, as the first daemon had: the default may be taken by a real masond.
            .args(["--port", "0"])
            .args([
                "--data-dir",
                &s.data_dir.display().to_string(),
                "--config-dir",
                &s.config_dir.display().to_string(),
            ])
            .args([
                "--host-bin",
                &cfg.host_bin.display().to_string(),
                "--mcp-bin",
                &cfg.mcp_bin.display().to_string(),
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_millis(800));
        if c.try_wait().unwrap().is_none() {
            alive = Some(c);
        }
    }
    let mut daemon = alive.unwrap();
    let v = wait_status(&s, "the rolled-back daemon", 60, |v| {
        v["running"].is_string()
    });
    assert_eq!(
        v["running"],
        src.shas[0].as_str(),
        "back on the previous runtime: {v:#}"
    );
    assert!(
        v["bad"]
            .as_array()
            .unwrap()
            .iter()
            .any(|b| *b == src.shas[1].as_str()),
        "{v:#}"
    );
    let alert = v["alert"].as_str().expect("an alert is raised");
    assert!(
        alert.contains("crashed three times") && alert.contains("rolled back"),
        "{alert}"
    );
    assert!(
        (3..=4).contains(&tries),
        "three crashes, not more: the loader started {tries} times"
    );
    // The alert is what a monitor shows, and it stands until acknowledged.
    let alerts = s.mason_ok(&["alerts"]);
    assert!(alerts.contains("rolled back"), "{alerts}");
    s.mason_ok(&["update", "ack"]);
    assert!(status(&s)["alert"].is_null());
    // The bad build is not tried again by accident.
    let msg = s.mason(&["update", "--sha", &src.shas[1]]);
    let _ = msg;
    let end = Instant::now() + Duration::from_secs(30);
    let v = wait_status(&s, "the refusal", 30, |v| v["phase"] == "failed");
    assert!(
        v["error"].as_str().unwrap().contains("rolled back from"),
        "{v:#}"
    );
    assert!(Instant::now() < end);
    let _ = daemon.kill();
    let _ = daemon.wait();
    kill_hosts(&s.data_dir);
}

#[test]
fn a_build_that_fails_its_self_test_never_takes_over() {
    let s = Scratch::new("true", json!({"steps": {}}));
    let src = source(&s, &[&[("selftest-fails", "x")]]);
    let _m = start(&s);
    let pid = daemon_pid(&s);
    let o = s.mason(&["update"]);
    assert!(!o.status.success(), "the update must fail");
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(
        said.contains("self-test failed") && said.contains("migration 99 failed"),
        "{said}"
    );
    assert!(said.contains("the running daemon is unchanged"), "{said}");
    let v = status(&s);
    assert!(
        v["running"].is_null() && v["current"].is_null(),
        "nothing was switched: {v:#}"
    );
    assert_eq!(daemon_pid(&s), pid);
    let _ = src;
}

#[test]
fn the_updater_refuses_to_go_backwards_unless_told_to_roll_back() {
    let s = Scratch::new("true", json!({"steps": {}}));
    let src = source(&s, &[&[], &[]]);
    let mut m = start(&s);
    s.mason_ok(&["update", "--sha", &src.shas[1]]);
    wait_status(&s, "c2 to run", 90, |v| {
        v["running"] == src.shas[1].as_str()
    });
    // Backwards, by sha and by a branch pointing at the older commit: refused, and nothing changes.
    let o = s.mason(&["update", "--sha", &src.shas[0]]);
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(!o.status.success(), "{said}");
    assert!(
        said.contains("ancestor") && said.contains("1 commit(s)") && said.contains("--rollback"),
        "{said}"
    );
    assert_eq!(status(&s)["running"], src.shas[1].as_str());
    // Asked for on purpose, it goes.
    s.mason_ok(&["update", "--sha", &src.shas[0], "--rollback"]);
    wait_status(&s, "the rollback", 90, |v| {
        v["running"] == src.shas[0].as_str()
    });
    // The same version again is not an update.
    let o = s.mason(&["update", "--sha", &src.shas[0]]);
    assert!(!o.status.success());
    assert!(format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
    .contains("already running"));
    let _ = m.child.kill();
}

#[test]
fn the_self_test_runs_on_a_copy_and_a_bad_database_fails_it() {
    let s = Scratch::new("true", json!({"steps": {}}));
    let d = s.start();
    drop(d);
    let copy = s.root.path().join("copy.db");
    std::fs::copy(s.data_dir.join("store.db"), &copy).unwrap();
    let masond = Path::new(env!("CARGO_BIN_EXE_masond"));
    bindir();
    let ok = Command::new(masond)
        .args(["selftest", "--db"])
        .arg(&copy)
        .output()
        .unwrap();
    let said = String::from_utf8_lossy(&ok.stdout).to_string();
    assert!(
        ok.status.success(),
        "{said}{}",
        String::from_utf8_lossy(&ok.stderr)
    );
    assert!(
        said.contains("migrations:")
            && said.contains("golden replay: 5 recorded runs")
            && said.contains("sandbox: a scripted task went from filed to landed"),
        "{said}"
    );
    // The original was not touched: the copy is what migrated.
    for ext in ["db-wal", "db-shm"] {
        let _ = std::fs::remove_file(copy.with_extension(ext));
    }
    std::fs::write(&copy, b"this is not a database at all").unwrap();
    let bad = Command::new(masond)
        .args(["selftest", "--db"])
        .arg(&copy)
        .output()
        .unwrap();
    assert!(!bad.status.success());
    assert!(String::from_utf8_lossy(&bad.stderr).contains("self-test FAILED"));
    kill_hosts(&s.data_dir);
}
