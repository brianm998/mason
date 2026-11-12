// SPDX-License-Identifier: GPL-3.0-or-later

//! A fetch that keeps failing raises `fetch:<project>` after the limit, naming git's error, and a
//! good fetch clears it.

mod support;
use serde_json::json;
use std::time::Duration;
use support::*;

#[test]
fn failing_fetches_alert_after_the_limit_and_a_good_fetch_clears_it() {
    let s = Scratch::new("true", json!({"steps": {}}));
    let d = s.start_with(|c| {
        c.reload_interval = Duration::from_secs(3600);
        c.fetch_interval = Duration::from_secs(3600);
        c.fetch_alert_after = Duration::from_secs(2);
    });
    let pid = *d.projects().keys().next().unwrap();
    let has = |d: &std::sync::Arc<mason_daemon::Daemon>| {
        mason_daemon::attention::sync_alerts(d);
        d.store
            .alerts(true)
            .unwrap()
            .into_iter()
            .find(|a| a.key == "fetch:scratch")
    };
    assert!(d.fetch_remote(pid).is_ok());
    assert!(has(&d).is_none());

    let away = s.origin.with_extension("away");
    std::fs::rename(&s.origin, &away).unwrap();
    assert!(d.fetch_remote(pid).is_err());
    assert!(has(&d).is_none(), "below the limit there is no alert");

    std::thread::sleep(Duration::from_millis(2200));
    assert!(d.fetch_remote(pid).is_err());
    let a = has(&d).expect("past the limit the alert is raised");
    assert_eq!(a.kind, "tracker");
    assert!(a.text.contains("failing for"), "{}", a.text);
    assert!(a.text.contains("o.git"), "names git's error: {}", a.text);
    assert!(a.text.contains("base as it was"), "{}", a.text);

    std::fs::rename(&away, &s.origin).unwrap();
    assert!(d.fetch_remote(pid).is_ok());
    assert!(has(&d).is_none(), "a good fetch clears it");
}
