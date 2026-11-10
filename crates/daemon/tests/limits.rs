// SPDX-License-Identifier: GPL-3.0-or-later

//! Stage 46: the only strict limits are on the machine. The `llm:claude` limit
//! survives a restart, and a compile waits while CPU use is high. The CPU number
//! is injected: nothing here reads the machine's real load.

use mason_daemon::cpu::FakeCpu;
use mason_daemon::sched::{Broker, LeaseState, ResourceCfg};
use mason_daemon::stall::FakeStall;
use std::collections::BTreeMap;
use std::path::Path;

fn broker(dir: &Path, cfg: &ResourceCfg, cpu: &std::sync::Arc<FakeCpu>) -> std::sync::Arc<Broker> {
    Broker::new(
        dir.join("locks"),
        dir.join("limits.kv"),
        cfg,
        10,
        cpu.clone(),
        &mason_daemon::providers::default_accounts(),
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

#[test]
fn with_the_machines_own_reading_a_saved_compile_count_is_ignored() {
    let dir = scratch("nocount");
    std::fs::write(dir.join("limits.kv"), "compile\t2\nllm:claude\t4\n").unwrap();
    let cfg =
        mason_daemon::sched::parse_resource_cfg("[cpu]\ncompile_settle_s = 0\ncompile_jobs = 0\n")
            .unwrap();
    let b = broker(&dir, &cfg, &FakeCpu::replacing_compile_count(10));
    b.declare("compile", 2);
    let threads = std::thread::available_parallelism().unwrap().get() as u32;
    assert_eq!(b.limit("compile"), Some(threads));
    assert_eq!(b.limit("llm:claude"), Some(4), "other saved limits stay");
    for i in 0..threads {
        let id = b.lease_open(&format!("cc{i}"), &compile());
        assert!(matches!(b.lease_poll(id), LeaseState::Granted { .. }));
    }
    let err = b.set_limit("compile", 2).unwrap_err();
    assert!(err.contains("compile_cpu_max"), "{err}");
}

#[test]
fn compiles_are_capped_by_default_however_idle_the_cpu_reads() {
    let dir = scratch("cap");
    let cfg = mason_daemon::sched::parse_resource_cfg("[cpu]\ncompile_settle_s = 0\n").unwrap();
    let b = broker(&dir, &cfg, &FakeCpu::replacing_compile_count(0));
    assert_eq!(b.limit("compile"), Some(3));
    let ids: Vec<u64> = (0..4)
        .map(|i| b.lease_open(&format!("cc{i}"), &compile()))
        .collect();
    for id in &ids[..3] {
        assert!(matches!(b.lease_poll(*id), LeaseState::Granted { .. }));
    }
    assert!(matches!(b.lease_poll(ids[3]), LeaseState::Waiting { .. }));
    b.lease_close(ids[0]);
    assert!(matches!(b.lease_poll(ids[3]), LeaseState::Granted { .. }));
}

#[test]
fn the_compile_cap_comes_from_the_config() {
    let dir = scratch("capcfg");
    let cfg = mason_daemon::sched::parse_resource_cfg("[cpu]\ncompile_jobs = 5\n").unwrap();
    let b = broker(&dir, &cfg, &FakeCpu::replacing_compile_count(0));
    assert_eq!(b.limit("compile"), Some(5));
}

#[test]
fn a_stalled_disk_holds_further_compiles_but_never_the_first() {
    let dir = scratch("stall");
    let cfg = mason_daemon::sched::parse_resource_cfg("[cpu]\ncompile_settle_s = 0\n").unwrap();
    let b = broker(&dir, &cfg, &FakeCpu::replacing_compile_count(0));
    let stall = FakeStall::new(21_000);
    b.set_stall(stall.clone(), 2_000);
    let first = b.lease_open("cc1", &compile());
    assert!(
        matches!(b.lease_poll(first), LeaseState::Granted { .. }),
        "with no compile running a stall is not ours to wait out"
    );
    let second = b.lease_open("cc2", &compile());
    match b.lease_poll(second) {
        LeaseState::Waiting { explain } => assert_eq!(
            explain,
            "compile waiting: the build disk took 21.0 s to answer (limit 2.0 s)"
        ),
        other => panic!("should wait: {other:?}"),
    }
    stall.set(5);
    assert!(matches!(b.lease_poll(second), LeaseState::Granted { .. }));
    // Started: a stall does not take it back.
    stall.set(60_000);
    assert!(matches!(b.lease_poll(first), LeaseState::Granted { .. }));
}

#[test]
fn compiles_start_one_at_a_time_on_an_idle_machine() {
    let dir = scratch("settle");
    let b = broker(
        &dir,
        &ResourceCfg::default(),
        &FakeCpu::replacing_compile_count(0),
    );
    let first = b.lease_open("cc1", &compile());
    let second = b.lease_open("cc2", &compile());
    assert!(matches!(b.lease_poll(first), LeaseState::Granted { .. }));
    match b.lease_poll(second) {
        LeaseState::Waiting { explain } => assert!(
            explain.starts_with("compile waiting: letting the last compile show in CPU use"),
            "{explain}"
        ),
        other => panic!("should wait: {other:?}"),
    }
}

#[test]
fn an_owners_compile_count_in_the_config_is_kept_and_not_paced() {
    let dir = scratch("owned");
    let cfg = mason_daemon::sched::parse_resource_cfg("[machine]\ncompile = 3\n").unwrap();
    let b = broker(&dir, &cfg, &FakeCpu::replacing_compile_count(0));
    assert_eq!(b.limit("compile"), Some(3));
    let first = b.lease_open("cc1", &compile());
    let second = b.lease_open("cc2", &compile());
    assert!(matches!(b.lease_poll(first), LeaseState::Granted { .. }));
    assert!(matches!(b.lease_poll(second), LeaseState::Granted { .. }));
    b.set_limit("compile", 2).unwrap();
    assert_eq!(b.limit("compile"), Some(2));
}
