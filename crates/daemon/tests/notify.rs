// SPDX-License-Identifier: GPL-3.0-or-later

//! Stage 23 end to end, inside the daemon: a condition produces one alert, one delivery per
//! channel and one reminder, then silence once it clears or is acknowledged; announcements
//! queue in order; and of two monitors exactly one is told to speak.

mod support;
use mason_daemon::{attention, notify, Daemon};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use support::*;

type Received = Arc<Mutex<Vec<(String, String)>>>;

/// A webhook receiver on loopback: records each body and its signature header.
fn receiver() -> (String, Received) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/hook", l.local_addr().unwrap());
    let got: Received = Arc::default();
    let g = got.clone();
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { continue };
            let mut buf = vec![];
            let mut chunk = [0u8; 4096];
            loop {
                let n = s.read(&mut chunk).unwrap_or(0);
                buf.extend_from_slice(&chunk[..n]);
                let text = String::from_utf8_lossy(&buf).to_string();
                if let Some((head, body)) = text.split_once("\r\n\r\n") {
                    let want: usize = head
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse().unwrap_or(0))
                        })
                        .unwrap_or(0);
                    if body.len() >= want {
                        let sig = head
                            .lines()
                            .find_map(|l| l.strip_prefix("X-Mason-Signature: "))
                            .unwrap_or("")
                            .to_string();
                        g.lock().unwrap().push((sig, body.to_string()));
                        break;
                    }
                }
                if n == 0 {
                    break;
                }
            }
            let _ =
                s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        }
    });
    (url, got)
}

fn start(s: &Scratch) -> Arc<Daemon> {
    s.start_with(|c| {
        c.owner_names = vec!["brian".into()];
        // The watcher is not what is under test: the dispatcher is ticked by hand.
        c.reload_interval = Duration::from_secs(3600);
    })
}

fn world() -> Scratch {
    Scratch::new("true", json!({"steps": {}}))
}

fn notify_toml(s: &Scratch, text: &str) {
    std::fs::write(s.config_dir.join("notify.toml"), text).unwrap();
}

fn pause(d: &Arc<Daemon>) {
    attention::pause(d, "", "all", "brian", 3600).unwrap();
    attention::sync_alerts(d);
}

fn announcements(d: &Daemon) -> usize {
    d.store.announcements(100).unwrap().len()
}

#[test]
fn one_alert_one_delivery_one_reminder_and_silence_after_it_clears() {
    let (url, got) = receiver();
    let s = world();
    let d = start(&s);
    notify_toml(
        &s,
        &format!("[routing.paused]\nsound = \"0\"\nspeech = \"0\"\nwebhook = \"0\"\n[reminder]\nafter = \"1s\"\n[webhook]\nurl = \"{url}\"\nsecret = \"s3cret\"\n"),
    );
    pause(&d);
    for _ in 0..3 {
        notify::tick(&d);
    }
    assert_eq!(d.store.alerts(true).unwrap().len(), 1);
    assert_eq!(
        announcements(&d),
        1,
        "one announcement however often it looks"
    );
    assert_eq!(got.lock().unwrap().len(), 1, "one webhook");
    // The signature verifies against the body that arrived.
    let (sig, body) = got.lock().unwrap()[0].clone();
    assert!(
        mason_adapter_api::webhook::verify("s3cret", body.as_bytes(), &sig),
        "{sig} {body}"
    );
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["reminder"], false);

    std::thread::sleep(Duration::from_millis(1300));
    for _ in 0..3 {
        notify::tick(&d);
    }
    assert_eq!(announcements(&d), 2, "the one reminder");
    assert_eq!(got.lock().unwrap().len(), 2);
    assert_eq!(
        serde_json::from_str::<Value>(&got.lock().unwrap()[1].1).unwrap()["reminder"],
        true
    );
    assert!(d.store.alerts(true).unwrap()[0].reminded_at.is_some());

    std::thread::sleep(Duration::from_millis(1300));
    for _ in 0..3 {
        notify::tick(&d);
    }
    assert_eq!(
        (announcements(&d), got.lock().unwrap().len()),
        (2, 2),
        "no loops"
    );

    // Clearing it: nothing more, ever, and the alert is gone from the live list.
    attention::resume(&d, "", "all").unwrap();
    attention::sync_alerts(&d);
    notify::tick(&d);
    assert!(d.store.alerts(true).unwrap().is_empty());
    assert_eq!((announcements(&d), got.lock().unwrap().len()), (2, 2));
}

#[test]
fn acknowledging_stops_the_noise_and_a_return_is_a_new_alert() {
    let (url, got) = receiver();
    let s = world();
    let d = start(&s);
    notify_toml(
        &s,
        &format!("[routing.paused]\nspeech = \"0\"\nwebhook = \"1s\"\n[reminder]\nafter = \"1s\"\n[webhook]\nurl = \"{url}\"\n"),
    );
    pause(&d);
    notify::tick(&d);
    assert_eq!(announcements(&d), 1);
    let key = d.store.alerts(true).unwrap()[0].key.clone();
    assert!(notify::ack(&d, &key).unwrap());
    std::thread::sleep(Duration::from_millis(1300));
    notify::tick(&d);
    assert_eq!(
        (announcements(&d), got.lock().unwrap().len()),
        (1, 0),
        "acknowledged: no webhook, no reminder"
    );
    assert!(
        d.store.alerts(true).unwrap()[0].acked_at.is_some(),
        "still visible"
    );
    // It clears and comes back: new alert, new notice.
    attention::resume(&d, "", "all").unwrap();
    attention::sync_alerts(&d);
    pause(&d);
    notify::tick(&d);
    assert_eq!(announcements(&d), 2);
}

#[test]
fn only_one_of_two_monitors_is_told_to_speak_and_announcements_come_in_order() {
    let s = world();
    let d = start(&s);
    for (k, line) in [("a", "First."), ("b", "Second.")] {
        d.store
            .enqueue_announcement(
                k,
                "needs_you",
                line,
                true,
                true,
                mason_daemon::daemon::now_ms(),
            )
            .unwrap();
    }
    let a = notify::claim(&d, "one", false);
    let b = notify::claim(&d, "two", false);
    let c = notify::claim(&d, "one", false);
    assert_eq!(a["speaker"], true);
    assert_eq!(b["speaker"], false);
    assert_eq!(c["speaker"], true);
    let lines: Vec<&str> = a["announcements"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["sentence"].as_str().unwrap())
        .collect();
    assert_eq!(lines, vec!["First.", "Second."]);
    assert!(b["announcements"].as_array().unwrap().is_empty());
    assert!(
        c["announcements"].as_array().unwrap().is_empty(),
        "handed out once"
    );
}

#[test]
fn a_muted_monitor_never_speaks() {
    let s = world();
    let d = start(&s);
    d.store
        .enqueue_announcement(
            "a",
            "failed",
            "Broke.",
            true,
            true,
            mason_daemon::daemon::now_ms(),
        )
        .unwrap();
    let m = notify::claim(&d, "quiet", true);
    assert_eq!(m["speaker"], false);
    assert!(m["announcements"].as_array().unwrap().is_empty());
    let other = notify::claim(&d, "loud", false);
    assert_eq!(other["announcements"].as_array().unwrap().len(), 1);
}

#[test]
fn the_daemon_speaks_only_when_no_monitor_has_claimed_recently() {
    let s = world();
    let d = start(&s);
    let out = s.root.path().join("spoken.txt");
    notify_toml(
        &s,
        &format!(
            "[speech]\ndaemon = true\ncommand = [\"sh\", \"-c\", \"echo \\\"$0\\\" >> {}\"]\n",
            out.display()
        ),
    );
    d.store
        .enqueue_announcement(
            "a",
            "failed",
            "Task 1 failed.",
            true,
            true,
            mason_daemon::daemon::now_ms(),
        )
        .unwrap();
    notify::claim(&d, "monitor", true); // a monitor is open (even muted)
    notify::tick(&d);
    assert!(!out.exists(), "a monitor is around: the daemon stays quiet");
}

#[test]
fn with_no_monitor_open_and_speech_on_the_daemon_speaks_in_order_through_its_command() {
    let s = world();
    let d = start(&s);
    let out = s.root.path().join("spoken.txt");
    notify_toml(
        &s,
        &format!(
            "[speech]\ndaemon = true\ncommand = [\"sh\", \"-c\", \"echo \\\"$0\\\" >> {}\"]\n",
            out.display()
        ),
    );
    let now = mason_daemon::daemon::now_ms();
    d.store
        .enqueue_announcement("a", "failed", "Task 1 failed.", true, true, now)
        .unwrap();
    d.store
        .enqueue_announcement(
            "b",
            "needs_you",
            "Task 2 needs a decision.",
            true,
            true,
            now,
        )
        .unwrap();
    notify::tick(&d);
    let said = std::fs::read_to_string(&out).unwrap();
    assert_eq!(
        said.lines().collect::<Vec<_>>(),
        vec!["Task 1 failed.", "Task 2 needs a decision."]
    );
    notify::tick(&d);
    assert_eq!(
        std::fs::read_to_string(&out).unwrap().lines().count(),
        2,
        "said once"
    );
}

#[test]
fn with_speech_off_the_daemon_stays_silent() {
    let s = world();
    let d = start(&s);
    let out = s.root.path().join("spoken.txt");
    notify_toml(
        &s,
        &format!(
            "[speech]\ncommand = [\"sh\", \"-c\", \"echo \\\"$0\\\" >> {}\"]\n",
            out.display()
        ),
    );
    d.store
        .enqueue_announcement(
            "a",
            "failed",
            "Task 1 failed.",
            true,
            true,
            mason_daemon::daemon::now_ms(),
        )
        .unwrap();
    notify::tick(&d);
    assert!(!out.exists());
}
