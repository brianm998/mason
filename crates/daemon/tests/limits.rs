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
fn a_further_compile_waits_while_cpu_is_high_and_starts_when_it_falls() {
    let dir = scratch("cpu");
    let cpu = FakeCpu::new(71);
    let b = broker(&dir, &ResourceCfg::default(), &cpu);
    b.declare("compile", 4);
    // None of ours running: the load is someone else's, and the first starts.
    let first = b.lease_open("cc", &compile());
    assert!(matches!(b.lease_poll(first), LeaseState::Granted { .. }));
    let id = b.lease_open("cc2", &compile());
    match b.lease_poll(id) {
        LeaseState::Waiting { explain } => {
            assert_eq!(explain, "compile waiting: CPU 71% > 50%")
        }
        other => panic!("should wait: {other:?}"),
    }
    let snap = b.snapshot();
    assert_eq!(snap["cpu"]["busy_pct"], 71);
    assert_eq!(snap["cpu"]["compile_max_pct"], 50);
    let c = b.compilation();
    assert_eq!(
        (c.limit, c.used, c.cpu_busy_pct, c.cpu_max_pct),
        (4, 1, 71, 50)
    );
    assert_eq!(c.running.len(), 1);
    assert!(c.running[0].interactive);
    assert_eq!(c.pending.len(), 1);
    assert_eq!(c.pending[0].cause, "cpu");
    assert_eq!(c.pending[0].position, 1);
    assert!(
        c.pending[0].reason.contains("CPU 71% busy"),
        "{}",
        c.pending[0].reason
    );
    cpu.set(20);
    assert!(matches!(b.lease_poll(id), LeaseState::Granted { .. }));
    // Started: a busy machine does not take it back.
    cpu.set(100);
    assert!(matches!(b.lease_poll(id), LeaseState::Granted { .. }));
    // Another compile waits behind the gate, though a slot is free.
    let id3 = b.lease_open("cc3", &compile());
    assert!(matches!(b.lease_poll(id3), LeaseState::Waiting { .. }));
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
fn the_owners_compile_count_replaces_compile_jobs_live_and_after_a_restart() {
    let dir = scratch("count");
    let cfg = mason_daemon::sched::parse_resource_cfg("[cpu]\ncompile_settle_s = 0\n").unwrap();
    let cpu = FakeCpu::replacing_compile_count(10);
    let b = broker(&dir, &cfg, &cpu);
    assert_eq!(
        b.limit("compile"),
        Some(3),
        "compile_jobs until the owner says otherwise"
    );
    let ids: Vec<u64> = (0..5)
        .map(|i| b.lease_open(&format!("cc{i}"), &compile()))
        .collect();
    for id in &ids[..3] {
        assert!(matches!(b.lease_poll(*id), LeaseState::Granted { .. }));
    }
    assert!(matches!(b.lease_poll(ids[3]), LeaseState::Waiting { .. }));
    let c = b.compilation();
    assert_eq!(c.pending[0].cause, "full");
    assert_eq!(c.pending[0].reason, "every slot is held (3 of 3)");
    assert_eq!(
        (c.pending[1].cause.as_str(), c.pending[1].position),
        ("full", 2)
    );
    // Raised live: the waiters start at once.
    b.set_limit("compile", 5).unwrap();
    for id in &ids[3..] {
        assert!(matches!(b.lease_poll(*id), LeaseState::Granted { .. }));
    }
    assert!(b
        .set_limit("compile", 0)
        .unwrap_err()
        .contains("at least 1"));
    drop(b);
    let b2 = broker(&dir, &cfg, &cpu);
    assert_eq!(
        b2.limit("compile"),
        Some(5),
        "the saved count survives a restart"
    );
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

#[test]
fn priority_first_orders_urgent_tasks_ahead_of_tasks_nearer_landing() {
    use mason_daemon::sched::Standing;
    let dir = scratch("order");
    let text = "[cpu]\ncompile_settle_s = 0\ncompile_jobs = 1\n[order]\npriority_first = true\n";
    let cfg = mason_daemon::sched::parse_resource_cfg(text).unwrap();
    assert!(cfg.priority_first);
    let b = broker(&dir, &cfg, &FakeCpu::replacing_compile_count(0));
    let held = b.lease_open("cc", &compile());
    assert!(matches!(b.lease_poll(held), LeaseState::Granted { .. }));
    let as_task = |steps, priority| {
        Some(Standing {
            steps_remaining: steps,
            span: 1,
            priority,
        })
    };
    b.lease_open_as("near, P2", &compile(), as_task(2, 2));
    b.lease_open_as("far, P0", &compile(), as_task(4, 0));
    let c = b.compilation();
    let order: Vec<&str> = c.pending.iter().map(|w| w.label.as_str()).collect();
    assert_eq!(order, ["far, P0", "near, P2"]);
    assert!(c.rules.contains("the most urgent task"), "{}", c.rules);
    assert!(
        mason_daemon::sched::parse_resource_cfg("[order]\npriority_first = 1\n").is_err(),
        "a bool, not a number"
    );
}

#[test]
fn by_default_progress_orders_waiters_and_notes_show_newest_first() {
    use mason_daemon::sched::Standing;
    let dir = scratch("progress");
    let cfg =
        mason_daemon::sched::parse_resource_cfg("[cpu]\ncompile_settle_s = 0\ncompile_jobs = 1\n")
            .unwrap();
    let b = broker(&dir, &cfg, &FakeCpu::replacing_compile_count(0));
    let held = b.lease_open("cc", &compile());
    assert!(matches!(b.lease_poll(held), LeaseState::Granted { .. }));
    let as_task = |steps, priority| {
        Some(Standing {
            steps_remaining: steps,
            span: 1,
            priority,
        })
    };
    b.lease_open_as("near, P2", &compile(), as_task(2, 2));
    b.lease_open_as("far, P0", &compile(), as_task(4, 0));
    let c = b.compilation();
    let order: Vec<&str> = c.pending.iter().map(|w| w.label.as_str()).collect();
    assert_eq!(order, ["near, P2", "far, P0"]);
    assert!(c.pending.iter().all(|w| !w.landing));
    let holders = b.compile_holders();
    assert_eq!(holders.len(), 1);
    assert!(holders[0].interactive && holders[0].task.is_none());
    let now = 10 * 60_000;
    for i in 0..7 {
        b.note(now, format!("note {i}"));
    }
    let c = b.compilation();
    assert_eq!(c.notes.len(), 5, "the last five");
    assert!(c.notes[0].ends_with("note 6"), "{}", c.notes[0]);
    assert!(c.rules.contains("is stopped"), "{}", c.rules);
}
