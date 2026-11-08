// SPDX-License-Identifier: GPL-3.0-or-later

use mason_step_host::client::{collect_to_file, ClientError, Event, HostClient};
use mason_step_host::launcher::{spawn_host, SpawnOptions};
use mason_step_host::protocol::{self, Frame};
use mason_step_host::registry::{
    lost_report, pid_alive, proc_start, read_kv, reap_dir, reap_verified, write_kv, ExitRecord,
    HostState, Registry,
};
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const HOST: &str = env!("CARGO_BIN_EXE_step-host");
const CTL: &str = env!("CARGO_BIN_EXE_step-host-ctl");

fn opts(root: &Path, id: &str, script: &str) -> SpawnOptions {
    let mut o = SpawnOptions::new(HOST, root, id, &["sh", "-c", script]);
    o.heartbeat = Duration::from_millis(50);
    o.linger = Duration::from_secs(30);
    o
}

fn root() -> (tempfile::TempDir, PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let r = d.path().join("h");
    (d, r)
}

/// Read everything from `offset` to the exit, returning bytes, record and
/// the number of heartbeats seen.
fn drain(c: &mut HostClient, offset: u64) -> (Vec<u8>, ExitRecord, usize) {
    c.attach(offset).unwrap();
    let (mut bytes, mut beats) = (vec![], 0);
    loop {
        match c.next_event().unwrap() {
            Event::Data {
                offset: o,
                bytes: b,
            } => {
                assert_eq!(o, offset + bytes.len() as u64, "contiguous");
                bytes.extend(b);
            }
            Event::Heartbeat { .. } => beats += 1,
            Event::Exit(rec) => return (bytes, rec, beats),
        }
    }
}

fn connect(sock: &Path) -> HostClient {
    HostClient::connect(sock, protocol::PREVIOUS, protocol::CURRENT, "test").unwrap()
}

fn wait_until(what: &str, mut f: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(10);
    while !f() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn output_and_exit_status_come_back_with_stderr_merged() {
    let (_d, r) = root();
    let info = spawn_host(&opts(&r, "a", "echo out; echo err >&2; exit 3")).unwrap();
    let (bytes, rec, _) = drain(&mut connect(&info.sock), 0);
    assert_eq!(String::from_utf8(bytes).unwrap(), "out\nerr\n");
    assert_eq!((rec.code, rec.signal, rec.output_len), (3, 0, 8));
}

#[test]
fn the_host_runs_in_its_own_session() {
    use nix::unistd::{getsid, Pid};
    let (_d, r) = root();
    let info = spawn_host(&opts(&r, "a", "sleep 5")).unwrap();
    let sid = getsid(Some(Pid::from_raw(info.host_pid as i32))).unwrap();
    assert_eq!(
        sid.as_raw() as u32,
        info.host_pid,
        "host is a session leader"
    );
    assert_ne!(sid, getsid(None).unwrap(), "not in our session");
    let child_sid = getsid(Some(Pid::from_raw(info.child_pid as i32))).unwrap();
    assert_eq!(child_sid, sid, "the child shares the host's session");
    let mut c = connect(&info.sock);
    c.stop(100).unwrap();
}

#[test]
fn a_new_controller_resumes_by_offset_without_loss_or_repeats() {
    let (_d, r) = root();
    let script = "i=1; while [ $i -le 60 ]; do echo line $i; i=$((i+1)); sleep 0.02; done";
    let info = spawn_host(&opts(&r, "a", script)).unwrap();
    let mut first = connect(&info.sock);
    first.attach(0).unwrap();
    let mut got = vec![];
    while got.len() < 100 {
        if let Event::Data { bytes, .. } = first.next_event().unwrap() {
            got.extend(bytes);
        }
    }
    drop(first); // the controller "dies"
    std::thread::sleep(Duration::from_millis(300)); // output continues unattended
    let (rest, rec, _) = drain(&mut connect(&info.sock), got.len() as u64);
    got.extend(rest);
    let want: String = (1..=60).map(|i| format!("line {i}\n")).collect();
    assert_eq!(String::from_utf8(got).unwrap(), want);
    assert_eq!(rec.code, 0);
}

#[test]
fn protocol_versions_current_and_previous_are_both_accepted() {
    let (_d, r) = root();
    let info = spawn_host(&opts(&r, "a", "sleep 0.6; echo done")).unwrap();

    let mut v1 = HostClient::connect(&info.sock, 1, 1, "old").unwrap();
    assert_eq!(v1.version, 1);
    let (_, _, beats) = drain(&mut v1, 0);
    assert_eq!(beats, 0, "version 1 has no heartbeats");

    let mut v2 = connect(&info.sock);
    assert_eq!(v2.version, 2);
    let (bytes, _, beats) = drain(&mut v2, 0);
    assert_eq!(bytes, b"done\n");
    let _ = beats; // child already finished: heartbeats are checked in the next test

    // A newer controller that also speaks ours negotiates down to current.
    assert_eq!(
        HostClient::connect(&info.sock, 1, 9, "future")
            .unwrap()
            .version,
        2
    );
    // One that speaks only versions we don't is refused with a reason.
    match HostClient::connect(&info.sock, 3, 4, "alien") {
        Err(ClientError::Rejected(why)) => {
            assert!(why.contains("no common protocol version"), "{why}")
        }
        other => panic!("expected rejection, got {:?}", other.map(|c| c.version)),
    }
}

#[test]
fn heartbeats_flow_to_v2_controllers_and_the_file_advances_unattended() {
    let (_d, r) = root();
    let info = spawn_host(&opts(&r, "a", "sleep 0.8")).unwrap();
    let (_, _, beats) = drain(&mut connect(&info.sock), 0);
    assert!(
        beats >= 3,
        "expected several heartbeats in 0.8s at 50ms, got {beats}"
    );

    let (_d2, r2) = root();
    let info = spawn_host(&opts(&r2, "b", "sleep 2")).unwrap();
    let reg = Registry::new(&r2);
    let a = reg.scan().unwrap()[0].last_heartbeat_ms.unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let b = reg.scan().unwrap()[0].last_heartbeat_ms.unwrap();
    assert!(
        b > a,
        "heartbeat file advances with no controller attached: {a} -> {b}"
    );
    connect(&info.sock).stop(100).unwrap();
}

#[test]
fn the_registry_tells_running_exited_and_lost_apart() {
    let (_d, r) = root();
    let reg = Registry::new(&r);
    let running = spawn_host(&opts(&r, "running", "sleep 30")).unwrap();
    let exited = spawn_host(&opts(&r, "exited", "echo bye; exit 4")).unwrap();
    let lost = spawn_host(&opts(&r, "lost", "echo partial; sleep 30")).unwrap();
    wait_until("exit record", || {
        matches!(state(&reg, "exited"), HostState::Exited(_))
    });
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(state(&reg, "running"), HostState::Running);
    match state(&reg, "exited") {
        HostState::Exited(rec) => assert_eq!((rec.code, rec.output_len), (4, 4)),
        s => panic!("{s:?}"),
    }

    // Kill the host itself: no exit record gets written.
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(lost.host_pid as i32),
        nix::sys::signal::Signal::SIGKILL,
    )
    .unwrap();
    wait_until("host death", || !pid_alive(lost.host_pid));
    assert_eq!(state(&reg, "lost"), HostState::Lost);
    let st = reg
        .scan()
        .unwrap()
        .into_iter()
        .find(|s| s.info.id == "lost")
        .unwrap();
    assert_eq!(
        st.journal_len, 8,
        "journal kept what was written before the death"
    );

    // The report says what is known: the host is gone, its child is not, the parent
    // saw signal 9, and the journal holds 8 bytes ending on a newline.
    let rep = lost_report(&reg.host_dir("lost")).unwrap();
    assert_eq!(
        (rep.host_pid, rep.child_pid),
        (lost.host_pid, lost.child_pid)
    );
    assert!(!rep.host_alive && rep.child_alive, "{rep:?}");
    assert_eq!(rep.signal, Some(9));
    assert_eq!((rep.journal_len, rep.ends_mid_line), (8, false));
    assert!(rep.last_heartbeat_ms.is_some() && rep.at_ms > 0);
    let text = rep.to_string();
    assert!(
        text.starts_with(&format!("host pid {} gone at ", lost.host_pid)),
        "{text}"
    );
    assert!(
        text.contains("killed by signal 9, no exit record"),
        "{text}"
    );

    // The controller can still salvage that output from disk.
    let dir = reg.host_dir("lost");
    match collect_to_file(&dir, &dir.join("rx"), &dir.join("ex")) {
        Err(ClientError::HostLost { collected }) => assert_eq!(collected, 8),
        other => panic!("{other:?}"),
    }
    assert_eq!(std::fs::read(dir.join("rx")).unwrap(), b"partial\n");

    // Cleanup of the child the killed host left behind (verified before it is killed).
    let r = reap_dir(&dir);
    assert!(r.child_group_killed && !r.host_killed, "{r:?}");
    connect(&running.sock).stop(100).unwrap();
    let _ = exited;
}

fn state(reg: &Registry, id: &str) -> HostState {
    reg.scan()
        .unwrap()
        .into_iter()
        .find(|s| s.info.id == id)
        .unwrap()
        .state
}

fn ctl(args: &[&str]) -> Child {
    Command::new(CTL)
        .env("STEP_HOST_BIN", HOST)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

fn ctl_wait(args: &[&str]) -> bool {
    ctl(args).wait().unwrap().success()
}

/// Kill the controller `after_ms` into the run, then let a fresh one finish.
fn kill_then_resume(id: &str, script: &str, after_ms: u64) -> (String, String) {
    let (d, r) = root();
    let rs = r.to_str().unwrap();
    let mut c = ctl(&["run", rs, id, "--hb-ms", "50", "--", "sh", "-c", script]);
    // The daemon would have recorded its intent before launching; a host that
    // is not yet up has nothing to reattach to, so the kill points start once
    // it is.
    wait_until("host up", || r.join(id).join("host.kv").exists());
    std::thread::sleep(Duration::from_millis(after_ms));
    let _ = c.kill();
    let _ = c.wait();
    assert!(
        ctl_wait(&["attach", rs, id]),
        "the replacement controller finishes (killed at {after_ms}ms)"
    );
    let dir = r.join(id);
    let out = std::fs::read_to_string(dir.join("received")).unwrap();
    let exit = std::fs::read_to_string(dir.join("exit_status")).unwrap();
    drop(d);
    (out, exit)
}

#[test]
fn killing_the_controller_during_output_loses_nothing() {
    let script = "i=1; while [ $i -le 150 ]; do echo line $i; i=$((i+1)); sleep 0.01; done; exit 7";
    let want: String = (1..=150).map(|i| format!("line {i}\n")).collect();
    for after in [30, 120, 400, 900] {
        let (out, exit) = kill_then_resume("o", script, after);
        assert_eq!(out, want, "killed at {after} ms");
        assert!(exit.contains("code=7"), "{exit}");
    }
}

#[test]
fn killing_the_controller_around_child_exit_loses_nothing() {
    for after in [250, 300, 350, 400, 450, 600] {
        let (out, exit) = kill_then_resume("e", "sleep 0.35; echo end; exit 5", after);
        assert_eq!(out, "end\n", "killed at {after} ms");
        assert!(exit.contains("code=5"), "killed at {after} ms: {exit}");
    }
}

#[test]
fn a_controller_dying_mid_socket_write_does_not_hurt_the_host() {
    let (_d, r) = root();
    let info = spawn_host(&opts(&r, "a", "echo alive; sleep 0.5; echo still")).unwrap();
    let dial = || UnixStream::connect(&info.sock).unwrap();

    // Half a length prefix, then gone.
    dial().write_all(&[0, 0]).unwrap();
    // A length that promises more than ever arrives.
    dial().write_all(&[0, 0, 0, 50, 1, 0]).unwrap();
    // An absurd length.
    dial().write_all(&[0xff, 0xff, 0xff, 0xff]).unwrap();
    // An unknown frame type.
    dial()
        .write_all(&protocol::encode(&Frame::Release)[..4])
        .unwrap();
    let mut s = dial();
    s.write_all(&[0, 0, 0, 1, 99]).unwrap();
    // A good Hello followed by half an Attach.
    let mut s = dial();
    protocol::write_frame(
        &mut s,
        &Frame::Hello {
            min: 1,
            max: 2,
            name: "x".into(),
        },
    )
    .unwrap();
    let attach = protocol::encode(&Frame::Attach { offset: 0 });
    s.write_all(&attach[..6]).unwrap();
    drop(s);
    // Garbage where Hello should be.
    let mut s = dial();
    protocol::write_frame(&mut s, &Frame::Release).unwrap();
    drop(s);

    let (bytes, rec, _) = drain(&mut connect(&info.sock), 0);
    assert_eq!(bytes, b"alive\nstill\n");
    assert_eq!(rec.code, 0);
}

#[test]
fn attaching_beyond_the_journal_is_refused() {
    let (_d, r) = root();
    let info = spawn_host(&opts(&r, "a", "echo x; sleep 1")).unwrap();
    let mut c = connect(&info.sock);
    c.attach(9999).unwrap();
    assert!(matches!(c.next_event(), Err(ClientError::Rejected(_))));
    connect(&info.sock).stop(100).unwrap();
}

#[test]
fn stop_terminates_the_whole_process_group() {
    let (_d, r) = root();
    let info = spawn_host(&opts(&r, "a", "sleep 60 & echo $!; wait")).unwrap();
    let mut c = connect(&info.sock);
    c.attach(0).unwrap();
    let mut out = vec![];
    while !out.contains(&b'\n') {
        if let Event::Data { bytes, .. } = c.next_event().unwrap() {
            out.extend(bytes);
        }
    }
    let grandchild: u32 = String::from_utf8(out).unwrap().trim().parse().unwrap();
    assert!(pid_alive(grandchild));
    c.stop(2000).unwrap();
    loop {
        if let Event::Exit(rec) = c.next_event().unwrap() {
            assert_eq!((rec.code, rec.signal), (-1, 15), "SIGTERM'd");
            break;
        }
    }
    wait_until("grandchild gone", || !pid_alive(grandchild));
}

#[test]
fn a_child_that_ignores_terminate_is_killed_after_the_grace_period() {
    let (_d, r) = root();
    let info = spawn_host(&opts(
        &r,
        "a",
        "trap '' TERM; echo ready; while true; do sleep 0.05; done",
    ))
    .unwrap();
    let mut c = connect(&info.sock);
    c.attach(0).unwrap();
    while !matches!(c.next_event().unwrap(), Event::Data { .. }) {}
    let t = Instant::now();
    c.stop(300).unwrap();
    loop {
        if let Event::Exit(rec) = c.next_event().unwrap() {
            assert_eq!(rec.signal, 9);
            break;
        }
    }
    assert!(
        t.elapsed() >= Duration::from_millis(250),
        "waited out the grace period"
    );
}

#[test]
fn release_ends_the_host_but_the_files_stay_readable() {
    let (_d, r) = root();
    let info = spawn_host(&opts(&r, "a", "echo kept; exit 2")).unwrap();
    let dir = r.join("a");
    let rec = collect_to_file(&dir, &dir.join("rx"), &dir.join("ex")).unwrap();
    assert_eq!((rec.code, rec.output_len), (2, 5));
    wait_until("host to leave", || !pid_alive(info.host_pid));
    assert!(!info.sock.exists(), "socket removed");
    // With the host gone, a second collection works from the files alone.
    let again = collect_to_file(&dir, &dir.join("rx2"), &dir.join("ex2")).unwrap();
    assert_eq!((again.code, again.output_len), (2, 5));
    assert_eq!(std::fs::read(dir.join("rx2")).unwrap(), b"kept\n");
}

#[test]
fn an_uncollected_result_lingers_then_the_host_leaves() {
    let (_d, r) = root();
    let mut o = opts(&r, "a", "echo x");
    o.linger = Duration::from_millis(300);
    let info = spawn_host(&o).unwrap();
    wait_until("host to leave after linger", || !pid_alive(info.host_pid));
    let dir = r.join("a");
    let rec = collect_to_file(&dir, &dir.join("rx"), &dir.join("ex")).unwrap();
    assert_eq!(rec.output_len, 2);
}

#[test]
fn a_command_that_cannot_start_is_a_result_not_a_crash() {
    let (_d, r) = root();
    let o = SpawnOptions::new(HOST, &r, "a", &["/no/such/binary"]);
    let info = spawn_host(&o).unwrap();
    let (bytes, rec, _) = drain(&mut connect(&info.sock), 0);
    assert_eq!(rec.code, 127);
    assert!(String::from_utf8(bytes)
        .unwrap()
        .contains("failed to start"));
}

#[test]
fn cwd_and_environment_reach_the_child() {
    let (_d, r) = root();
    let work = tempfile::tempdir().unwrap();
    let mut o = opts(&r, "a", "echo $MASON_TEST_VAR; pwd -P");
    o.cwd = Some(work.path().to_path_buf());
    o.env = vec![("MASON_TEST_VAR".into(), "hello world".into())];
    let info = spawn_host(&o).unwrap();
    let (bytes, _, _) = drain(&mut connect(&info.sock), 0);
    let text = String::from_utf8(bytes).unwrap();
    let want = std::fs::canonicalize(work.path()).unwrap();
    assert_eq!(text, format!("hello world\n{}\n", want.display()));
}

#[test]
fn large_output_streams_in_order() {
    let (_d, r) = root();
    let info = spawn_host(&opts(&r, "a", "head -c 3000000 /dev/zero | tr '\\0' 'x'")).unwrap();
    let (bytes, rec, _) = drain(&mut connect(&info.sock), 0);
    assert_eq!(bytes.len(), 3_000_000);
    assert!(bytes.iter().all(|b| *b == b'x'));
    assert_eq!(rec.output_len, 3_000_000);
}

#[test]
fn stdin_comes_from_a_file_and_clear_env_hides_ours() {
    let (d, r) = root();
    let input = d.path().join("in.txt");
    std::fs::write(&input, "from a file\n").unwrap();
    // A variable in *our* environment, inherited by the host, must not reach a cleared child.
    std::env::set_var("MASON_LEAK_CHECK", "leaked");
    let mut o = SpawnOptions::new(
        HOST,
        &r,
        "a",
        &[
            "sh",
            "-c",
            "cat; echo leak=[$MASON_LEAK_CHECK] kept=[$KEEP]",
        ],
    );
    o.stdin = Some(input);
    o.clear_env = true;
    o.env = vec![
        ("PATH".into(), std::env::var("PATH").unwrap()),
        ("KEEP".into(), "yes".into()),
    ];
    let info = spawn_host(&o).unwrap();
    let (bytes, rec, _) = drain(&mut connect(&info.sock), 0);
    assert_eq!(
        String::from_utf8(bytes).unwrap(),
        "from a file\nleak=[] kept=[yes]\n"
    );
    assert_eq!(rec.code, 0);
}

fn set_kv(dir: &Path, f: impl FnOnce(&mut std::collections::HashMap<String, String>)) {
    let mut kv = read_kv(&dir.join("host.kv")).unwrap();
    f(&mut kv);
    let pairs: Vec<(&str, String)> = kv.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
    write_kv(&dir.join("host.kv"), &pairs).unwrap();
}

#[test]
fn a_lost_report_notices_a_journal_cut_off_mid_line() {
    let (_d, r) = root();
    let h = spawn_host(&opts(&r, "cut", "printf abc; sleep 30")).unwrap();
    wait_until("output", || {
        std::fs::metadata(r.join("cut/journal")).is_ok_and(|m| m.len() == 3)
    });
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(h.host_pid as i32),
        nix::sys::signal::Signal::SIGKILL,
    )
    .unwrap();
    wait_until("host death", || !pid_alive(h.host_pid));
    let rep = lost_report(&r.join("cut")).unwrap();
    assert!(rep.ends_mid_line && rep.journal_len == 3, "{rep:?}");
    assert!(rep.to_string().contains("cut off mid-line"));
    reap_dir(&r.join("cut"));
}

#[test]
fn reap_leaves_a_recycled_pid_alone() {
    let (_d, r) = root();
    let h = spawn_host(&opts(&r, "live", "sleep 30")).unwrap();
    let dir = r.join("live");
    let orig = read_kv(&dir.join("host.kv")).unwrap();
    assert!(!orig["host_start"].is_empty() && !orig["child_start"].is_empty());
    // The pids are alive, and the host's command line names step-host and the directory,
    // but the start strings say these are not the processes that were recorded.
    set_kv(&dir, |kv| {
        kv.insert("host_start".into(), "Thu Jan  1 00:00:00 1970".into());
        kv.insert("child_start".into(), "Thu Jan  1 00:00:00 1970".into());
    });
    let out = reap_dir(&dir);
    assert!(!out.host_killed && !out.child_group_killed, "{out:?}");
    assert!(pid_alive(h.host_pid) && pid_alive(h.child_pid));
    // Entries from an older host carry no start strings at all.
    set_kv(&dir, |kv| {
        kv.remove("host_start");
        kv.remove("child_start");
    });
    let out = reap_dir(&dir);
    assert!(!out.host_killed && !out.child_group_killed, "{out:?}");
    assert!(pid_alive(h.host_pid) && pid_alive(h.child_pid));
    // The true entry is killed.
    set_kv(&dir, |kv| {
        kv.extend(orig);
    });
    let out = reap_dir(&dir);
    assert!(out.host_killed && out.child_group_killed, "{out:?}");
}

#[test]
fn reap_leaves_another_process_and_protected_pids_alone() {
    let (_d, r) = root();
    let h = spawn_host(&opts(&r, "base", "sleep 30")).unwrap();
    let dir = r.join("base");
    // Another process with correct start strings: not a step host, not a group leader.
    let mut other = Command::new("sleep").arg("30").spawn().unwrap();
    let pid = other.id();
    let start = proc_start(pid).unwrap();
    set_kv(&dir, |kv| {
        kv.insert("host_pid".into(), pid.to_string());
        kv.insert("child_pid".into(), pid.to_string());
        kv.insert("host_start".into(), start.clone());
        kv.insert("child_start".into(), start.clone());
    });
    let out = reap_dir(&dir);
    assert!(!out.host_killed && !out.child_group_killed, "{out:?}");
    assert!(pid_alive(pid));
    // Pids 0 and 1, and our own pid and group, are never signalled (we would not be here).
    let own = std::process::id();
    let own_start = proc_start(own).unwrap();
    let own_pg = nix::unistd::getpgrp().as_raw() as u32;
    for (host, child) in [(0, 0), (1, 1), (own, own), (own_pg, own_pg)] {
        set_kv(&dir, |kv| {
            kv.insert("host_pid".into(), host.to_string());
            kv.insert("child_pid".into(), child.to_string());
            kv.insert("host_start".into(), own_start.clone());
            kv.insert("child_start".into(), own_start.clone());
        });
        let out = reap_dir(&dir);
        assert!(
            !out.host_killed && !out.child_group_killed,
            "{host}: {out:?}"
        );
    }
    let _ = other.kill();
    let _ = other.wait();
    // Put the real pids back and finish the host properly.
    set_kv(&dir, |kv| {
        kv.insert("host_pid".into(), h.host_pid.to_string());
        kv.insert("child_pid".into(), h.child_pid.to_string());
    });
    connect(&h.sock).stop(100).unwrap();
    let _ = reap_verified(&r);
}
