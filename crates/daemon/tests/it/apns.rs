// SPDX-License-Identifier: GPL-3.0-or-later

//! The APNs channel (s460) inside the daemon: a push goes to each eligible device through a
//! fake APNs on loopback, with a JWT that verifies against the owner's key; a 410 clears the
//! token; a key file others can read is refused; a device registers only for itself.

use crate::support;
use axum::extract::State;
use axum::{Extension, Json};
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use mason_daemon::lan::Peer;
use mason_daemon::{attention, notify, Daemon};
use ring::rand::SystemRandom;
use ring::signature::{
    EcdsaKeyPair, KeyPair, UnparsedPublicKey, ECDSA_P256_SHA256_FIXED,
    ECDSA_P256_SHA256_FIXED_SIGNING,
};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use support::*;

/// What the fake APNs saw: the request line and headers, and the body.
type Seen = Arc<Mutex<Vec<(String, String)>>>;

fn fake_apns(status: u16, reply: &'static str) -> (String, Seen) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let seen: Seen = Arc::default();
    let g = seen.clone();
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
                        g.lock().unwrap().push((head.to_string(), body.to_string()));
                        break;
                    }
                }
                if n == 0 {
                    break;
                }
            }
            let _ = s.write_all(
                format!(
                    "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                    reply.len()
                )
                .as_bytes(),
            );
        }
    });
    (url, seen)
}

fn start(s: &Scratch) -> Arc<Daemon> {
    s.start_with(|c| {
        c.owner_names = vec!["brian".into()];
        c.reload_interval = Duration::from_secs(3600);
    })
}

/// A `.p8` in the scratch config dir (mode 0600) and the public half of its key.
fn write_key(s: &Scratch, mode: u32) -> (String, Vec<u8>) {
    use std::os::unix::fs::PermissionsExt;
    let rng = SystemRandom::new();
    let der = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
    let pair =
        EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, der.as_ref(), &rng).unwrap();
    let path = s.config_dir.join("AuthKey_TEST.p8");
    std::fs::write(
        &path,
        format!(
            "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n",
            STANDARD.encode(der.as_ref())
        ),
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
    (
        path.display().to_string(),
        pair.public_key().as_ref().to_vec(),
    )
}

fn config(s: &Scratch, key: &str, endpoint: &str, extra: &str) {
    std::fs::write(
        s.config_dir.join("notify.toml"),
        format!(
            "[routing.paused]\napns = \"0\"\n[apns]\nkey_file = \"{key}\"\nkey_id = \"KEYID12345\"\nteam_id = \"TEAMID1234\"\ntopic = \"com.example.mason\"\nendpoint = \"{endpoint}\"\n{extra}"
        ),
    )
    .unwrap();
}

fn pause(d: &Arc<Daemon>) {
    attention::pause(d, "", "all", "brian", 3600).unwrap();
    attention::sync_alerts(d);
}

fn register(d: &Daemon, name: &str, role: &str, token: &str, env: &str) -> u64 {
    let id = d.store.create_device(name, role, name, 1).unwrap();
    notify::set_own_push(d, id, Some((token, env))).unwrap();
    id
}

#[test]
fn a_push_reaches_owner_devices_with_a_jwt_that_verifies_and_skips_viewers() {
    let (url, seen) = fake_apns(200, "");
    let s = Scratch::new("true", json!({"steps": {}}));
    let d = start(&s);
    let (key, public) = write_key(&s, 0o600);
    config(&s, &key, &url, "");
    let phone = register(&d, "phone", "owner", "aabbcc01", "sandbox");
    let _watcher = register(&d, "tv", "viewer", "ddeeff02", "production");
    pause(&d);
    for _ in 0..3 {
        notify::tick(&d);
    }
    let got = seen.lock().unwrap().clone();
    assert_eq!(got.len(), 1, "one POST for the one owner device: {got:?}");
    let (head, body) = &got[0];
    assert!(head.starts_with("POST /3/device/aabbcc01 "), "{head}");
    assert!(head
        .to_ascii_lowercase()
        .contains("apns-topic: com.example.mason"));
    let auth = head
        .lines()
        .find_map(|l| l.strip_prefix("authorization: bearer "))
        .expect("the JWT travels in the authorization header")
        .trim();
    let p: Vec<&str> = auth.split('.').collect();
    let header: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(p[0]).unwrap()).unwrap();
    assert_eq!(header, json!({"alg": "ES256", "kid": "KEYID12345"}));
    let claims: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(p[1]).unwrap()).unwrap();
    assert_eq!(claims["iss"], "TEAMID1234");
    UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, &public)
        .verify(
            format!("{}.{}", p[0], p[1]).as_bytes(),
            &URL_SAFE_NO_PAD.decode(p[2]).unwrap(),
        )
        .expect("signature verifies against the owner's key");
    let v: Value = serde_json::from_str(body).unwrap();
    assert_eq!(v["mason"]["kind"], "paused");
    assert!(v["aps"]["alert"]["title"]
        .as_str()
        .unwrap()
        .starts_with("MASON"));
    // Delivered once, however often it is looked at, and recorded.
    let a = &d.store.alerts(true).unwrap()[0];
    assert!(d
        .store
        .deliveries_for(&a.key, a.since)
        .unwrap()
        .iter()
        .any(|r| r.channel == "apns" && r.ok));
    assert!(d.store.push_of(phone).unwrap().is_some());
}

#[test]
fn viewers_get_the_push_when_the_owner_asks_for_it() {
    let (url, seen) = fake_apns(200, "");
    let s = Scratch::new("true", json!({"steps": {}}));
    let d = start(&s);
    let (key, _) = write_key(&s, 0o600);
    config(&s, &key, &url, "viewers = true\n");
    register(&d, "phone", "owner", "aabbcc01", "sandbox");
    register(&d, "tv", "viewer", "ddeeff02", "production");
    pause(&d);
    notify::tick(&d);
    let paths: Vec<String> = seen
        .lock()
        .unwrap()
        .iter()
        .map(|(h, _)| h.lines().next().unwrap().to_string())
        .collect();
    assert_eq!(paths.len(), 2, "{paths:?}");
}

#[test]
fn a_410_clears_that_devices_token_and_only_that_one() {
    let (url, seen) = fake_apns(410, r#"{"reason":"Unregistered"}"#);
    let s = Scratch::new("true", json!({"steps": {}}));
    let d = start(&s);
    let (key, _) = write_key(&s, 0o600);
    config(&s, &key, &url, "");
    let gone = register(&d, "phone", "owner", "aabbcc01", "sandbox");
    pause(&d);
    notify::tick(&d);
    assert_eq!(seen.lock().unwrap().len(), 1);
    assert!(
        d.store.push_of(gone).unwrap().is_none(),
        "the token was cleared"
    );
    // With no token left there is nothing to send to, and nothing to retry.
    notify::tick(&d);
    assert_eq!(seen.lock().unwrap().len(), 1);
}

#[test]
fn a_key_file_others_can_read_is_refused_and_nothing_is_sent() {
    let (url, seen) = fake_apns(200, "");
    let s = Scratch::new("true", json!({"steps": {}}));
    let d = start(&s);
    let (key, _) = write_key(&s, 0o644);
    config(&s, &key, &url, "");
    register(&d, "phone", "owner", "aabbcc01", "sandbox");
    pause(&d);
    notify::tick(&d);
    assert!(seen.lock().unwrap().is_empty());
    let err = notify::test(&d, "apns").unwrap_err();
    assert!(err.contains("0600"), "{err}");
    assert!(!err.contains("BEGIN"), "no key text in an error");
}

#[test]
fn apns_counts_as_reaching_you_only_with_a_key_and_a_registered_device() {
    let s = Scratch::new("true", json!({"steps": {}}));
    let d = start(&s);
    let (key, _) = write_key(&s, 0o600);
    let reaches = |d: &Arc<Daemon>| {
        notify::status(d)["reaches_without_monitor"]
            .as_bool()
            .unwrap()
    };
    assert!(!reaches(&d));
    config(&s, &key, "http://127.0.0.1:1", "");
    assert!(!reaches(&d), "no device has a token");
    let id = register(&d, "phone", "owner", "aabbcc01", "sandbox");
    assert!(reaches(&d));
    d.store.revoke_device(id, 5).unwrap();
    assert!(!reaches(&d), "a revoked device's token is gone");
}

#[test]
fn the_test_command_sends_to_every_eligible_device() {
    let (url, seen) = fake_apns(200, "");
    let s = Scratch::new("true", json!({"steps": {}}));
    let d = start(&s);
    let (key, _) = write_key(&s, 0o600);
    config(&s, &key, &url, "");
    assert!(notify::test(&d, "apns").is_err(), "no device yet");
    register(&d, "phone", "owner", "aabbcc01", "sandbox");
    register(&d, "ipad", "owner", "aabbcc02", "sandbox");
    let r = notify::test(&d, "apns").unwrap();
    assert_eq!(r["devices"], 2);
    assert_eq!(seen.lock().unwrap().len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_device_registers_a_token_for_itself_only() {
    let s = Scratch::new("true", json!({"steps": {}}));
    let d = start(&s);
    let mine = d.store.create_device("mine", "viewer", "h1", 1).unwrap();
    let other = d.store.create_device("other", "owner", "h2", 1).unwrap();
    let peer = |device| {
        Some(Extension(Peer {
            device,
            name: "x".into(),
            role: mason_core::access::Role::Viewer,
        }))
    };
    let set = |p, token: &str, env: &str| {
        mason_daemon::lan::set_push(
            State(d.clone()),
            p,
            Json(json!({"token": token, "env": env})),
        )
    };
    // A viewer sets its own row; the other device is untouched.
    let _ = set(peer(mine), "AABB11", "sandbox").await.unwrap();
    assert_eq!(
        d.store.push_of(mine).unwrap(),
        Some(("aabb11".into(), "sandbox".into()))
    );
    assert!(d.store.push_of(other).unwrap().is_none());
    // A body naming another device changes nothing about who is written.
    let _ = mason_daemon::lan::set_push(
        State(d.clone()),
        peer(mine),
        Json(json!({"token": "cc22", "env": "production", "device": other, "id": other})),
    )
    .await
    .unwrap();
    assert!(d.store.push_of(other).unwrap().is_none());
    // Not a request from a paired device: refused.
    let (code, _) = set(None, "aabb", "sandbox").await.unwrap_err();
    assert_eq!(code.as_u16(), 403);
    // Bad input is refused, not stored.
    assert!(set(peer(mine), "not hex!", "sandbox").await.is_err());
    assert!(set(peer(mine), "aabb", "staging").await.is_err());
    // Forgetting.
    let _ = mason_daemon::lan::clear_push(State(d.clone()), peer(mine))
        .await
        .unwrap();
    assert!(d.store.push_of(mine).unwrap().is_none());
    assert!(mason_daemon::lan::clear_push(State(d.clone()), None)
        .await
        .is_err());
}
