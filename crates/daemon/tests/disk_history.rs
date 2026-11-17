// SPDX-License-Identifier: GPL-3.0-or-later

//! Disk space history (task 514) against a real `masond`: the free space of each volume is
//! sampled from the file `MASON_DISK_FILE` names (one volume per line, `name gb_free gb_total`),
//! served on `/disk/history` with each volume's name, and follows the file as it is rewritten.
//! A project whose path is on a second volume adds that volume. The real disk is never read.

mod support;
use serde_json::Value;
use std::io::{Read, Write};
use std::time::{Duration, Instant};
use support::*;

const GB: u64 = 1024 * 1024 * 1024;

fn get(s: &Scratch, path: &str) -> Value {
    let info: Value =
        serde_json::from_str(&std::fs::read_to_string(s.data_dir.join("daemon.json")).unwrap())
            .unwrap();
    let port = info["port"].as_u64().unwrap();
    let mut c = std::net::TcpStream::connect(("127.0.0.1", port as u16)).unwrap();
    write!(c, "GET {path} HTTP/1.0\r\nHost: x\r\n\r\n").unwrap();
    let mut body = String::new();
    c.read_to_string(&mut body).unwrap();
    serde_json::from_str(body.split("\r\n\r\n").nth(1).unwrap()).unwrap()
}

fn until(what: &str, mut f: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(60);
    while !f() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The newest reading of the volume called `name`, in whole GB free.
fn free_gb(h: &Value, name: &str) -> Option<u64> {
    let id = h["volumes"]
        .as_array()?
        .iter()
        .find(|v| v["name"] == name)?["id"]
        .clone();
    let last = h["points"].as_array()?.last()?;
    let r = last["readings"]
        .as_array()?
        .iter()
        .find(|r| r["volume"] == id)?;
    Some(r["freeBytes"].as_str()?.parse::<u64>().ok()? / GB)
}

#[test]
fn volumes_are_sampled_from_the_file_and_a_project_adds_one() {
    let s = Scratch::new("true", serde_json::json!({"steps": {}}));
    let file = s.root.path().join("disks");
    // The data dir is on no named volume, so on the first line's: `alpha`.
    std::fs::write(&file, "alpha 40 100\n").unwrap();
    let env = [
        ("MASON_DISK_FILE", file.to_str().unwrap()),
        ("MASON_DISK_HISTORY_EVERY_MS", "50"),
    ];
    {
        let mut m = s.masond(&[], &env);
        s.wait_daemon(&mut m);
        until("alpha at 40", || {
            free_gb(&get(&s, "/disk/history?span=5m"), "alpha") == Some(40)
        });
        let h = get(&s, "/disk/history?span=5m");
        assert_eq!(h["volumes"].as_array().unwrap().len(), 1, "{h}");
        assert_eq!(h["volumes"][0]["roles"][0], "data");
        std::fs::write(&file, "alpha 35 100\n").unwrap();
        until("alpha at 35", || {
            free_gb(&get(&s, "/disk/history?span=5m"), "alpha") == Some(35)
        });
        std::fs::write(&file, "alpha 30 100\n").unwrap();
        until("alpha at 30", || {
            free_gb(&get(&s, "/disk/history?span=5m"), "alpha") == Some(30)
        });
        // Readings kept, oldest first, each with the total.
        let h = get(&s, "/disk/history?span=5m");
        let pts = h["points"].as_array().unwrap();
        assert!(pts.len() >= 3);
        let at: Vec<u64> = pts
            .iter()
            .map(|p| p["atMs"].as_str().unwrap().parse().unwrap())
            .collect();
        assert!(at.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(pts[0]["readings"][0]["totalBytes"], (100 * GB).to_string());
        let bad = get(&s, "/disk/history?span=nonsense");
        assert!(bad.to_string().contains("not a span"), "{bad}");
    }

    // The project's path (`.../repo`) is on the second volume, named for that directory.
    s.write_project("scratch", "true", "");
    std::fs::write(&file, "alpha 30 100\nrepo 7 50\n").unwrap();
    let mut m = s.masond(&[], &env);
    s.wait_daemon(&mut m);
    until("both volumes", || {
        let h = get(&s, "/disk/history?span=5m");
        free_gb(&h, "alpha") == Some(30) && free_gb(&h, "repo") == Some(7)
    });
    std::fs::write(&file, "alpha 30 100\nrepo 6 50\n").unwrap();
    until("repo at 6", || {
        free_gb(&get(&s, "/disk/history?span=5m"), "repo") == Some(6)
    });
    let h = get(&s, "/disk/history?span=5m");
    let repo = h["volumes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["name"] == "repo")
        .unwrap();
    assert!(repo["roles"].to_string().contains("project"), "{repo}");
}
