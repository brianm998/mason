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

    // A monitor plays the first announcement; only then is the sound and speech delivered, and
    // only then does a reminder follow.
    assert_eq!(
        notify::claim(&d, "mon", false)["announcements"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
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
    // The speech runs off the tick's thread: wait for it.
    wait_for("both sentences", 10, || {
        std::fs::read_to_string(&out).is_ok_and(|t| t.lines().count() >= 2)
    });
    let said = std::fs::read_to_string(&out).unwrap();
    assert_eq!(
        said.lines().collect::<Vec<_>>(),
        vec!["Task 1 failed.", "Task 2 needs a decision."]
    );
    notify::tick(&d);
    std::thread::sleep(Duration::from_millis(300));
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

fn reaches(s: &Scratch, d: &Arc<Daemon>, toml: Option<&str>) -> (bool, bool) {
    if let Some(t) = toml {
        notify_toml(s, t);
    }
    (
        notify::status(d)["reaches_without_monitor"]
            .as_bool()
            .unwrap(),
        notify::claim(d, "m", true)["reaches_without_monitor"]
            .as_bool()
            .unwrap(),
    )
}

#[test]
fn reaches_without_monitor_for_each_configuration() {
    let s = world();
    let d = start(&s);
    assert_eq!(reaches(&s, &d, None), (false, false), "no notify.toml");
    for (toml, want) in [
        ("[speech]\ndaemon = true\n", false),
        ("[speech]\ndaemon = false\ncommand = [\"say\"]\n", false),
        ("[speech]\ndaemon = true\ncommand = [\"say\"]\n", true),
        ("[ntfy]\ntopic = \"t\"\n", true),
        ("[webhook]\nurl = \"http://127.0.0.1:1/h\"\n", true),
        (
            "[email]\nserver = \"smtp://x\"\nfrom = \"a@b\"\nto = [\"c@d\"]\n",
            true,
        ),
    ] {
        assert_eq!(reaches(&s, &d, Some(toml)), (want, want), "{toml}");
    }
}

#[test]
fn a_routed_channel_that_is_not_set_up_is_not_a_failed_delivery() {
    let s = world();
    let d = start(&s);
    notify_toml(&s, "[routing.paused]\nsound = \"0\"\npush = \"0\"\n");
    pause(&d);
    notify::tick(&d);
    let alerts = d.store.alerts(false).unwrap();
    assert!(!alerts.is_empty());
    for a in alerts {
        assert!(d
            .store
            .deliveries_for(&a.key, a.since)
            .unwrap()
            .iter()
            .all(|r| r.channel != "push"));
    }
}

#[test]
fn a_speech_command_that_hangs_is_killed_and_does_not_delay_the_tick() {
    let s = world();
    let d = s.start_with(|c| {
        c.owner_names = vec!["brian".into()];
        c.reload_interval = Duration::from_secs(3600);
        c.speech_timeout = Duration::from_millis(500);
    });
    let started = s.root.path().join("started.txt");
    notify_toml(
        &s,
        &format!(
            "[speech]\ndaemon = true\ncommand = [\"sh\", \"-c\", \"echo \\\"$0\\\" >> {}; sleep 600\"]\n",
            started.display()
        ),
    );
    let now = mason_daemon::daemon::now_ms();
    for (k, t) in [("a", "One."), ("b", "Two.")] {
        d.store
            .enqueue_announcement(k, "failed", t, true, true, now)
            .unwrap();
    }
    let t0 = std::time::Instant::now();
    notify::tick(&d);
    assert!(
        t0.elapsed() < Duration::from_millis(400),
        "the tick did not wait for the speech: {:?}",
        t0.elapsed()
    );
    // Both sentences are tried, each cut off at the limit.
    wait_for("the second sentence", 10, || {
        std::fs::read_to_string(&started).is_ok_and(|t| t.lines().count() >= 2)
    });
}

fn stuck_world() -> (Scratch, Arc<Daemon>) {
    let s = world();
    let d = s.start_with(|c| {
        c.owner_names = vec!["brian".into()];
        c.reload_interval = Duration::from_millis(100);
        c.fetch_interval = Duration::from_millis(100);
        c.attention_interval = Duration::from_millis(100);
        c.stuck_after = Duration::from_secs(2);
    });
    (s, d)
}

#[test]
fn alerts_and_notifications_keep_working_while_the_repository_lock_is_held() {
    let (url, got) = receiver();
    let (s, d) = stuck_world();
    notify_toml(
        &s,
        &format!("[routing.paused]\nwebhook = \"0\"\n[routing.git_stuck]\nwebhook = \"0\"\n[webhook]\nurl = \"{url}\"\n"),
    );
    let pid = *d.projects.keys().next().unwrap();
    let lock = d.repo_lock(pid);
    let guard = lock.lock().unwrap();
    // Fetches have a lock of their own; hold it too, so the definitions thread blocks in its fetch.
    let fetch = d.fetch_lock(pid);
    let fetch_guard = fetch.lock().unwrap();
    // The definitions thread now blocks in its fetch. A pause is a condition that appears
    // meanwhile; the attention thread must raise it and deliver it.
    pause(&d);
    wait_for("the pause alert", 10, || {
        d.store
            .alerts(true)
            .unwrap()
            .iter()
            .any(|a| a.key.starts_with("paused"))
    });
    wait_for("a notification", 10, || !got.lock().unwrap().is_empty());
    wait_for("the stuck alert", 15, || {
        d.store
            .alerts(true)
            .unwrap()
            .iter()
            .any(|a| a.key == "git-stuck")
    });
    let a = d
        .store
        .alerts(true)
        .unwrap()
        .into_iter()
        .find(|a| a.key == "git-stuck")
        .unwrap();
    assert!(a.text.contains("stuck"), "{}", a.text);
    assert!(a.text.contains("notify.rs"), "names the holder: {}", a.text);
    drop(guard);
    drop(fetch_guard);
    wait_for("the stuck alert to clear", 15, || {
        !d.store
            .alerts(true)
            .unwrap()
            .iter()
            .any(|a| a.key == "git-stuck")
    });
}

#[test]
fn a_disk_below_the_floor_is_announced_with_sound_and_speech_once() {
    let s = world();
    let d = s.start_with(|c| {
        c.owner_names = vec!["brian".into()];
        c.reload_interval = Duration::from_secs(3600);
        c.disk = Some(mason_daemon::disk::FakeDisk::new_gb(5));
        c.disk_every = Duration::from_millis(50);
    });
    wait_for("the disk alert", 30, || {
        attention::sync_alerts(&d);
        d.store
            .alerts(true)
            .unwrap()
            .iter()
            .any(|a| a.kind == "disk")
    });
    for _ in 0..3 {
        notify::tick(&d);
    }
    let got = notify::claim(&d, "monitor", false);
    let list = got["announcements"].as_array().unwrap();
    let disk: Vec<_> = list.iter().filter(|a| a["kind"] == "disk").collect();
    assert_eq!(disk.len(), 1, "{got}");
    assert_eq!(disk[0]["sound"], true);
    assert_eq!(disk[0]["speech"], true);
    assert_eq!(disk[0]["sound_name"], "broke");
}

#[test]
fn the_disk_alert_survives_a_store_that_refuses_writes_and_is_delivered_once() {
    let (url, got) = receiver();
    let s = world();
    let disk = mason_daemon::disk::FakeDisk::new_gb(5);
    let d = s.start_with(|c| {
        c.owner_names = vec!["brian".into()];
        c.reload_interval = Duration::from_secs(3600);
        c.disk = Some(disk.clone());
        c.disk_every = Duration::from_millis(50);
    });
    notify_toml(
        &s,
        &format!("[routing.disk]\nwebhook = \"0\"\n[webhook]\nurl = \"{url}\"\nsecret = \"s\"\n"),
    );
    notify::refuse_writes(&d, true);
    wait_for("the disk alert, from memory", 30, || {
        attention::sync_alerts(&d);
        notify::live_alerts(&d, true)
            .unwrap()
            .iter()
            .any(|a| a.kind == "disk")
    });
    assert!(notify::store_unwritable(&d));
    assert!(
        d.store.alerts(true).unwrap().is_empty(),
        "nothing reached the table"
    );
    for _ in 0..4 {
        notify::tick(&d);
    }
    assert_eq!(got.lock().unwrap().len(), 1, "sent once, not every pass");
    assert_eq!(notify::unrecorded_deliveries(&d), 1);
    let since = notify::live_alerts(&d, true).unwrap()[0].since;

    notify::refuse_writes(&d, false);
    attention::sync_alerts(&d);
    assert!(!notify::store_unwritable(&d));
    let rows = d.store.alerts(true).unwrap();
    assert_eq!(rows.len(), 1, "the table caught up");
    assert_eq!(rows[0].since, since, "it is the same alert");
    for _ in 0..4 {
        notify::tick(&d);
    }
    assert_eq!(got.lock().unwrap().len(), 1, "nothing announced twice");
    assert_eq!(notify::unrecorded_deliveries(&d), 0);
    assert_eq!(d.store.deliveries_for("disk", since).unwrap().len(), 1);
}

// ---- delivery is a monitor playing it -------------------------------------------------

const HOUR: u64 = 3_600_000;

fn audible_pause(s: &Scratch) -> Arc<Daemon> {
    let d = start(s);
    notify_toml(s, "[routing.paused]\nsound = \"0\"\nspeech = \"0\"\n");
    pause(&d);
    d
}

fn played(d: &Daemon) -> Vec<(String, String, bool)> {
    let a = d.store.alerts(true).unwrap().remove(0);
    d.store
        .deliveries_for(&a.key, a.since)
        .unwrap()
        .into_iter()
        .filter(|r| r.channel == "sound" || r.channel == "speech")
        .map(|r| (r.channel, r.delivery, r.ok))
        .collect()
}

fn count(v: &Value) -> usize {
    v["announcements"].as_array().unwrap().len()
}

#[test]
fn an_alert_raised_with_no_monitor_records_no_delivery() {
    let s = world();
    let d = audible_pause(&s);
    let since = d.store.alerts(true).unwrap()[0].since;
    notify::tick_at(&d, since + 1000);
    assert_eq!(announcements(&d), 1, "queued");
    assert!(played(&d).is_empty(), "queued is not delivered");
}

#[test]
fn a_claim_long_afterwards_gets_one_announcement_and_records_it() {
    let s = world();
    let d = audible_pause(&s);
    let since = d.store.alerts(true).unwrap()[0].since;
    notify::tick_at(&d, since + 1000);
    let late = since + 3 * HOUR;
    let first = notify::claim_at(&d, "mon", false, late);
    assert_eq!(count(&first), 1, "{first}");
    let got = played(&d);
    assert!(
        got.contains(&("sound".into(), "initial".into(), true))
            && got.contains(&("speech".into(), "initial".into(), true)),
        "{got:?}"
    );
    assert_eq!(count(&notify::claim_at(&d, "mon", false, late + 1000)), 0);
    notify::tick_at(&d, late + 2000);
    assert_eq!(count(&notify::claim_at(&d, "mon", false, late + 3000)), 0);
    let status = notify::status(&d);
    assert!(status["last_played"].as_u64().is_some());
}

#[test]
fn an_alert_that_cleared_first_gets_nothing() {
    let s = world();
    let d = audible_pause(&s);
    let since = d.store.alerts(true).unwrap()[0].since;
    notify::tick_at(&d, since + 1000);
    attention::resume(&d, "", "all").unwrap();
    attention::sync_alerts(&d);
    assert_eq!(
        count(&notify::claim_at(&d, "mon", false, since + 3 * HOUR)),
        0
    );
}

#[test]
fn a_muted_claim_gets_nothing_and_records_nothing() {
    let s = world();
    let d = audible_pause(&s);
    let since = d.store.alerts(true).unwrap()[0].since;
    notify::tick_at(&d, since + 1000);
    let m = notify::claim_at(&d, "quiet", true, since + 3 * HOUR);
    assert_eq!(count(&m), 0);
    assert!(played(&d).is_empty());
}
