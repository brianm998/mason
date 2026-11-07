// SPDX-License-Identifier: GPL-3.0-or-later

//! Stage 46: the only strict limits are on the machine. The `llm:claude` limit
//! survives a restart, and a compile waits while CPU use is high. The CPU number
//! is injected: nothing here reads the machine's real load.

use mason_daemon::cpu::FakeCpu;
use mason_daemon::sched::{Broker, LeaseState, ResourceCfg};
use std::collections::BTreeMap;
use std::path::Path;

fn broker(dir: &Path, cfg: &ResourceCfg, cpu: &std::sync::Arc<FakeCpu>) -> std::sync::Arc<Broker> {
    Broker::new(
        dir.join("locks"),
        dir.join("limits.kv"),
        cfg,
        10,
        cpu.clone(),
    )
    .unwrap()
}

fn compile() -> BTreeMap<String, u32> {
    [("compile".to_string(), 1)].into()
}

fn scratch(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("mason-limits-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn the_llm_limit_defaults_to_ten_and_a_set_limit_survives_a_restart() {
    let dir = scratch("llm");
    let cpu = FakeCpu::new(0);
    let b = broker(&dir, &ResourceCfg::default(), &cpu);
    assert_eq!(b.limit("llm:claude"), Some(10));
    b.set_limit("llm:claude", 4).unwrap();
    drop(b);
    // A new Broker on the same limits file, as after a restart.
    let b2 = broker(&dir, &ResourceCfg::default(), &cpu);
    assert_eq!(b2.limit("llm:claude"), Some(4));
}

#[test]
fn a_compile_waits_while_cpu_is_high_and_starts_when_it_falls() {
    let dir = scratch("cpu");
    let cpu = FakeCpu::new(71);
    let b = broker(&dir, &ResourceCfg::default(), &cpu);
    b.declare("compile", 4);
    let id = b.lease_open("cc", &compile());
    match b.lease_poll(id) {
        LeaseState::Waiting { explain } => {
            assert_eq!(explain, "compile waiting: CPU 71% > 50%")
        }
        other => panic!("should wait: {other:?}"),
    }
    let snap = b.snapshot();
    assert_eq!(snap["cpu"]["busy_pct"], 71);
    assert_eq!(snap["cpu"]["compile_max_pct"], 50);
    cpu.set(20);
    assert!(matches!(b.lease_poll(id), LeaseState::Granted { .. }));
    // Started: a busy machine does not take it back.
    cpu.set(100);
    assert!(matches!(b.lease_poll(id), LeaseState::Granted { .. }));
    // Another compile waits behind the gate, though a slot is free.
    let id2 = b.lease_open("cc2", &compile());
    assert!(matches!(b.lease_poll(id2), LeaseState::Waiting { .. }));
}

#[test]
fn the_compile_cpu_limit_comes_from_the_config() {
    let dir = scratch("cfg");
    let cpu = FakeCpu::new(60);
    let cfg = mason_daemon::sched::parse_resource_cfg("[cpu]\ncompile_cpu_max = 80\n").unwrap();
    let b = broker(&dir, &cfg, &cpu);
    b.declare("compile", 1);
    let id = b.lease_open("cc", &compile());
    assert!(matches!(b.lease_poll(id), LeaseState::Granted { .. }));
}
