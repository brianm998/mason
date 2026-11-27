// SPDX-License-Identifier: GPL-3.0-or-later

//! #1261: a monitor's `SetLimit` for `gha` over the socket changes the home view's runner pool and
//! is kept in `limits.kv` (so a restart reads it back); a command that is not the owner's is refused
//! with the daemon's reason and changes nothing.

use crate::support::*;
use mason_daemon::views::Ctx;
use mason_protocol::client::Client;
use mason_protocol::*;
use std::time::Duration;

fn set_limit(resource: &str, units: u32, actor: &str) -> Command {
    Command {
        id: String::new(),
        body: Some(command::Body::SetLimit(SetLimit {
            resource: resource.into(),
            units,
        })),
        actor: actor.into(),
        reason: String::new(),
    }
}

fn pool(d: &std::sync::Arc<mason_daemon::Daemon>, name: &str) -> RunnerPool {
    let w = d.world().unwrap();
    Ctx::new(d, &w)
        .home("")
        .runner_pools
        .into_iter()
        .find(|p| p.name == name)
        .unwrap_or_else(|| panic!("no {name} pool in the home view"))
}

#[test]
fn set_limit_for_gha_over_the_socket_changes_the_view_and_is_kept_for_a_restart() {
    let s = Scratch::new("true", serde_json::json!({"steps": {}}));
    s.write_project("scratch", "true", "");
    let d = s.start();
    let d2 = d.clone();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let _ = rt.block_on(mason_daemon::http::serve(d2));
    });
    let info = d.cfg.info_path();
    let mut port = 0;
    wait_for("the socket to be served", 20, || {
        std::fs::read_to_string(&info)
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .and_then(|v| v["port"].as_u64())
            .map(|p| port = p as u16)
            .is_some()
    });
    let mut c = Client::connect(port, Encoding::Json).unwrap();
    c.hello(0, true).unwrap();
    let owner = d.cfg.owner_names[0].clone();

    let before = pool(&d, "gha");
    assert!(before.ceiling > 0 && before.limit > 0, "{before:?}");

    // Not the owner: refused with the reason, nothing changes.
    let (r, _) = c
        .command(
            set_limit("gha", 3, "somebody-else"),
            Duration::from_secs(10),
        )
        .unwrap();
    assert!(!r.ok, "{r:?}");
    assert!(r.error.contains("only the owner"), "{r:?}");
    assert_eq!(pool(&d, "gha").limit, before.limit);

    // The owner, the way a monitor sends it.
    let (r, _) = c
        .command(set_limit("gha", 3, &owner), Duration::from_secs(10))
        .unwrap();
    assert!(r.ok, "{r:?}");
    assert_eq!(pool(&d, "gha").limit, 3);
    assert_eq!(
        pool(&d, "gha-macos").limit,
        d.broker.limit("gha-macos").unwrap()
    );
    let kept = std::fs::read_to_string(d.cfg.data_dir.join("limits.kv")).unwrap();
    assert!(kept.lines().any(|l| l == "gha\t3"), "{kept}");

    // A new broker on the same data directory, as after a restart, reads it back.
    let b = mason_daemon::sched::Broker::new(
        d.cfg.data_dir.join("locks-restart"),
        d.cfg.data_dir.join("limits.kv"),
        &mason_daemon::sched::ResourceCfg::default(),
        10,
        mason_daemon::cpu::FakeCpu::new(0),
        &mason_daemon::providers::default_accounts(),
    )
    .unwrap();
    assert_eq!(b.limit("gha"), Some(3));
}
