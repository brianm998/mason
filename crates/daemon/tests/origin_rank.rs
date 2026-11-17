// SPDX-License-Identifier: GPL-3.0-or-later

//! s552: who is really waiting. A `mason run` with no terminal, from an agent, a hook or a
//! task's environment, is pipeline work and does not take the whole machine unless the owner
//! allows its origin; a person at a terminal keeps the top rank and the machine. Nothing
//! here asserts on timing.

use mason_daemon::cpu::FakeCpu;
use mason_daemon::sched::{parse_resource_cfg, Broker, LeaseState, ResourceCfg};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

fn broker(dir: &Path, cfg: &ResourceCfg) -> Arc<Broker> {
    Broker::new(
        dir.join("locks"),
        dir.join("limits.kv"),
        cfg,
        10,
        FakeCpu::new(0),
        &mason_daemon::providers::default_accounts(),
    )
    .unwrap()
}

fn scratch(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("mason-origin-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn uses(items: &[(&str, u32)]) -> BTreeMap<String, u32> {
    items.iter().map(|(k, n)| (k.to_string(), *n)).collect()
}

#[test]
fn a_non_person_exclusive_request_becomes_all_compile_units() {
    let b = broker(&scratch("convert"), &ResourceCfg::default());
    b.declare("compile", 3);
    for origin in ["agent", "hook", "no-terminal", "task"] {
        let o = b.lease_open_from(
            "xcodebuild",
            &uses(&[("exclusive", 1)]),
            None,
            None,
            Some(origin),
        );
        assert_eq!(o.rank, "pipeline", "{origin}");
        let c = o.converted.clone().expect("converted");
        assert!(c.contains("all 3 compile units"), "{c}");
        assert!(b.lease_close(o.id));
    }
}

#[test]
fn a_person_keeps_the_whole_machine_and_the_top_rank() {
    let b = broker(&scratch("person"), &ResourceCfg::default());
    b.declare("compile", 1);
    for origin in [None, Some("person")] {
        let o = b.lease_open_from("archive", &uses(&[("exclusive", 1)]), None, None, origin);
        assert_eq!(o.rank, "person");
        assert_eq!(o.converted, None);
        assert!(matches!(
            b.lease_poll(o.id),
            LeaseState::Granted {
                exclusive: true,
                ..
            }
        ));
        assert!(b.lease_close(o.id));
    }
}

#[test]
fn the_owner_can_allow_exclusive_for_an_origin() {
    let cfg = parse_resource_cfg("[order]\nexclusive_origins = [\"agent\"]\n").unwrap();
    assert_eq!(cfg.exclusive_origins, vec!["agent".to_string()]);
    let b = broker(&scratch("allow"), &cfg);
    b.declare("compile", 3);
    let o = b.lease_open_from("x", &uses(&[("exclusive", 1)]), None, None, Some("agent"));
    assert_eq!(o.converted, None);
    assert_eq!(o.rank, "pipeline");
    assert!(matches!(
        b.lease_poll(o.id),
        LeaseState::Granted {
            exclusive: true,
            ..
        }
    ));
}

#[test]
fn a_person_goes_ahead_of_an_agent_that_asked_first() {
    let b = broker(&scratch("order"), &ResourceCfg::default());
    b.declare("compile", 1);
    let held = b.lease_open("holder", &uses(&[("compile", 1)]));
    assert!(matches!(b.lease_poll(held), LeaseState::Granted { .. }));
    let agent = b.lease_open_from(
        "agent build",
        &uses(&[("compile", 1)]),
        None,
        None,
        Some("agent"),
    );
    let person = b.lease_open_from("my build", &uses(&[("compile", 1)]), None, None, None);
    assert!(b.lease_close(held));
    assert!(matches!(
        b.lease_poll(person.id),
        LeaseState::Granted { .. }
    ));
    assert!(matches!(b.lease_poll(agent.id), LeaseState::Waiting { .. }));
}

#[test]
fn exclusive_settings_parse() {
    let c = parse_resource_cfg("[order]\nexclusive_max_wait_s = 120\n").unwrap();
    assert_eq!(c.exclusive_max_wait_s, 120);
    assert_eq!(ResourceCfg::default().exclusive_max_wait_s, 600);
    assert!(parse_resource_cfg("[order]\nexclusive_origins = 3\n").is_err());
}
