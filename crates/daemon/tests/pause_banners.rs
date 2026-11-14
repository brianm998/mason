// SPDX-License-Identifier: GPL-3.0-or-later
#![cfg(unix)]
// Unix-only: signals, permission bits or unix sockets. Windows coverage comes with the later port tasks.

//! A paused banner says which pause it is (project and mode), so a monitor can lift exactly that
//! one. Other banners leave both empty, and a hold for a refused push has no pause banner.

mod support;
use mason_daemon::views::Ctx;
use support::*;

#[test]
fn paused_banners_carry_the_project_and_mode_of_their_pause() {
    let s = Scratch::new("true", happy_script());
    let d = s.start();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let far = now + 3_600_000;
    d.store
        .add_pause("star", "claims", "brian", now, far)
        .unwrap();
    d.store.add_pause("", "all", "brian", now, far).unwrap();
    d.store
        .add_pause(
            "star",
            "landing",
            mason_daemon::ssh_keys::PAUSE_OWNER,
            now,
            far,
        )
        .unwrap();
    let hv = Ctx::new(&d, &d.world().unwrap()).header("");
    let paused: Vec<_> = hv.banners.iter().filter(|b| b.kind == "paused").collect();
    let mut got: Vec<(String, String)> = paused
        .iter()
        .map(|b| (b.project.clone(), b.mode.clone()))
        .collect();
    got.sort();
    assert_eq!(
        got,
        vec![
            ("".to_string(), "all".to_string()),
            ("star".to_string(), "claims".to_string())
        ],
        "the push-auth pause has no banner"
    );
    let claims = paused.iter().find(|b| b.mode == "claims").unwrap();
    assert_eq!(claims.text, "Paused (claims) for star by brian");
    for b in hv.banners.iter().filter(|b| b.kind != "paused") {
        assert!(b.project.is_empty() && b.mode.is_empty(), "{}", b.kind);
    }
}
