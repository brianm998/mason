// SPDX-License-Identifier: GPL-3.0-or-later

//! Devices as test targets (Part 7, Part 8): reads what the provider lists,
//! sets the size of each device pool (`device:ios-simulator` and so on) from
//! it, and leases one particular device after the scheduler grants a pool
//! unit. A pool with no units is the scheduler's own wait; this module only
//! says it in words (`wait_text`) so it is an alert and charged to no task.
//!
//! The system provider is asked only while some loaded project declares a
//! device environment, so a machine without Xcode or adb is never probed.

use crate::config::TargetsCfg;
use crate::daemon::{now_ms, Daemon};
use mason_adapter_api::device::{Device, DeviceKind, DeviceProvider, DeviceState};
use mason_devices::inventory;
use mason_devices::lease::{DeviceLease, Leases};
use mason_devices::wait;
use mason_scheduler::Needs;
use mason_testing::EnvKind;
use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A reading that takes longer than this is abandoned and tried again next tick.
const READ_LIMIT: Duration = Duration::from_secs(10);

#[derive(Default)]
struct Reading {
    devices: Vec<Device>,
    /// The last failure to read, kept until a read succeeds.
    error: Option<String>,
    at_ms: u64,
    counts: BTreeMap<DeviceKind, u32>,
}

pub struct Targets {
    provider: Arc<dyn DeviceProvider>,
    pub cfg: TargetsCfg,
    leases: Arc<Leases>,
    reading: Mutex<Reading>,
}

impl Targets {
    pub fn new(cfg: TargetsCfg, data_dir: &std::path::Path) -> Result<Targets, String> {
        let provider: Arc<dyn DeviceProvider> = if cfg.fake {
            let file = cfg.fake_file.clone().ok_or("targets: no fake_file")?;
            Arc::new(mason_devices::fake::FakeProvider::new(
                file,
                cfg.fake_log.clone(),
            ))
        } else {
            Arc::new(mason_adapters::devices::System::new())
        };
        let leases = Leases::new(data_dir.join("locks"))
            .map_err(|e| format!("cannot create the device lock directory: {e}"))?;
        Ok(Targets {
            provider,
            cfg,
            leases,
            reading: Mutex::default(),
        })
    }

    pub fn provider(&self) -> &Arc<dyn DeviceProvider> {
        &self.provider
    }

    fn reading(&self) -> std::sync::MutexGuard<'_, Reading> {
        self.reading.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Declare every device pool (with no units if none is seen yet), so a
    /// request for one is a wait on a person, not an unknown resource.
    pub fn declare_all(&self, d: &Daemon) {
        for k in DeviceKind::ALL {
            if d.broker.limit(&k.capability()).is_none() {
                d.broker.set_pool_units(&k.capability(), 0);
            }
        }
    }

    /// Read the provider (on its own thread, with a limit) and apply what it says.
    pub fn refresh(&self, d: &Daemon) {
        let p = self.provider.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let _ = std::thread::Builder::new()
            .name("device-read".into())
            .spawn(move || {
                let _ = tx.send(p.list());
            });
        let result = match rx.recv_timeout(READ_LIMIT) {
            Ok(r) => r.map_err(|e| e.to_string()),
            Err(_) => Err(format!(
                "the device provider did not answer in {}s",
                READ_LIMIT.as_secs()
            )),
        };
        self.declare_all(d);
        let mut r = self.reading();
        match result {
            Ok(devices) => {
                let new = inventory::counts(&devices);
                let changed = inventory::changes(&r.counts, &new);
                r.devices = devices;
                r.error = None;
                r.at_ms = now_ms();
                r.counts = new;
                drop(r);
                for (kind, n) in changed {
                    d.broker.set_pool_units(&kind.capability(), n);
                }
                // The listing, not just the counts, may have changed what a waiter can use.
                d.poke();
            }
            // A failed read keeps the last good inventory: a flaky tool is not an unplug.
            Err(e) => {
                tracing::warn!(error = %e, "reading devices failed");
                r.error = Some(e);
            }
        }
    }

    /// Lease one device of a kind that nobody holds and the environment's
    /// filter admits. `None` if all such devices are taken or none matches.
    pub fn lease(
        &self,
        kind: DeviceKind,
        filter: &inventory::Filter,
        label: &str,
    ) -> Option<DeviceLease> {
        let devices = self.reading().devices.clone();
        let mut skip: HashSet<String> = HashSet::new();
        loop {
            let id = inventory::pick(&devices, kind, filter, &|id| {
                skip.contains(id) || self.leases.is_held(id)
            })?
            .id
            .clone();
            match self.leases.try_take(&id, label) {
                Some(l) => return Some(l),
                None => {
                    skip.insert(id);
                }
            }
        }
    }

    /// Inventory, leases and pool sizes, for `mason targets list` and GET /targets.
    pub fn view(&self, d: &Daemon) -> serde_json::Value {
        let held = self.leases.held();
        let r = self.reading();
        let devices: Vec<_> = r
            .devices
            .iter()
            .map(|x| {
                serde_json::json!({
                    "id": x.id,
                    "kind": x.kind.name(),
                    "os": x.os,
                    "name": x.name,
                    "state": if x.state == DeviceState::Ready { "ready" } else { "unavailable" },
                    "leased_by": held.get(&x.id),
                })
            })
            .collect();
        let pools: Vec<_> = DeviceKind::ALL
            .iter()
            .map(|k| {
                let name = k.capability();
                serde_json::json!({
                    "name": name,
                    "units": d.broker.limit(&name),
                    "used": d.broker.used(&name),
                })
            })
            .collect();
        serde_json::json!({
            "provider": self.provider.name(),
            "read_ms": r.at_ms,
            "error": r.error,
            "devices": devices,
            "pools": pools,
        })
    }
}

/// Does any loaded project declare a device environment?
pub fn wanted(d: &Daemon) -> bool {
    d.projects().keys().any(|pid| {
        d.current_def(*pid)
            .testing
            .environments
            .iter()
            .any(|e| matches!(e.kind, EnvKind::Device(_)))
    })
}

/// The device kind of a suite's environment, if it has one.
pub fn kind_of_env(e: &mason_testing::Environment) -> Option<DeviceKind> {
    match e.kind {
        EnvKind::Device(t) => DeviceKind::from_name(t.name()),
        _ => None,
    }
}

/// Which devices of its kind an environment may lease (`os`, `name`, `exclude`).
pub fn filter_of_env(e: &mason_testing::Environment) -> inventory::Filter {
    inventory::Filter {
        os: e.os.clone(),
        name: e.device_name.clone(),
        exclude: e.exclude.clone(),
    }
}

/// What a wait is for. A device pool with no units is the environment; otherwise the broker has
/// just said which resource or gate holds the request back, and the first thing asked for is the
/// fallback.
pub fn wait_class(d: &Daemon, needs: &Needs, task: mason_core::TaskId) -> String {
    for r in needs.amounts.keys() {
        if wait::kind_of(r).is_some() && d.broker.limit(r) == Some(0) {
            return r.clone();
        }
    }
    d.broker.wait_class(task).unwrap_or_else(|| {
        if needs.exclusive {
            "exclusive".into()
        } else {
            needs.amounts.keys().next().cloned().unwrap_or_default()
        }
    })
}

/// Words for a wait. A device pool with no units is waiting for a person.
pub fn wait_text(d: &Daemon, needs: &Needs, scheduler_text: &str) -> String {
    for r in needs.amounts.keys() {
        if let Some(kind) = wait::kind_of(r) {
            if d.broker.limit(r) == Some(0) {
                return wait::no_device(kind);
            }
        }
    }
    scheduler_text.to_string()
}

/// Start reading devices, if any project wants them.
pub fn spawn(d: &Arc<Daemon>) {
    if wanted(d) {
        d.targets.refresh(d);
    }
    let every = Duration::from_millis(d.targets.cfg.poll_ms);
    crate::bg::spawn_loop(d, "targets", move |weak| {
        std::thread::sleep(every);
        let Some(d) = weak.upgrade() else {
            return false;
        };
        if wanted(&d) {
            d.targets.refresh(&d);
        }
        true
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEF: &str = r#"schema = 1
start = "test"

[step.test]
type = "test"
category = "testing"
suites = ["ui"]

[suite.ui]
command = "true"
parser = "tap"
min_tests = 1
level = "unit"
env = "sim"

[environment.sim]
type = "ios-simulator"
name = "tdt-*"
"#;

    /// Targets on the fake provider, holding what the fixture lists.
    fn targets(dir: &std::path::Path, devices: &str) -> Targets {
        let fixture = dir.join("devices.json");
        std::fs::write(&fixture, format!(r#"{{"devices":[{devices}]}}"#)).unwrap();
        let cfg = TargetsCfg {
            fake: true,
            fake_file: Some(fixture),
            ..TargetsCfg::default()
        };
        let t = Targets::new(cfg, dir).unwrap();
        let listed = t.provider.list().unwrap();
        t.reading().devices = listed;
        t
    }

    fn sim(id: &str, name: &str) -> String {
        format!(r#"{{"id":"{id}","kind":"ios-simulator","os":"26.1","name":"{name}"}}"#)
    }

    // The case that matters on a shared Mac: another pipeline's
    // `pipeline-slot-*` simulators are listed (and sort first), and an
    // environment with `name = "tdt-*"` leases only its own.
    #[test]
    fn a_name_glob_from_the_definition_never_leases_pipeline_slot_1() {
        let dir = tempfile::tempdir().unwrap();
        let t = targets(
            dir.path(),
            &[
                sim("0000-SLOT1", "pipeline-slot-1"),
                sim("0001-SLOT2", "pipeline-slot-2"),
                sim("9999-TDT1", "tdt-1"),
            ]
            .join(","),
        );
        let def = mason_definition::parse(DEF).unwrap();
        let env = def.testing.environment("sim").unwrap();
        let kind = kind_of_env(env).unwrap();
        let filter = filter_of_env(env);

        let first = t.lease(kind, &filter, "#1 ui").unwrap();
        assert_eq!(first.id, "9999-TDT1");
        // Its own simulator is busy: no lease, rather than a pipeline-slot one.
        assert!(t.lease(kind, &filter, "#2 ui").is_none());
        assert!(!t.leases.is_held("0000-SLOT1") && !t.leases.is_held("0001-SLOT2"));
        drop(first);
        assert_eq!(t.lease(kind, &filter, "#2 ui").unwrap().id, "9999-TDT1");
    }

    #[test]
    fn with_only_another_pipelines_simulators_nothing_is_leased() {
        let dir = tempfile::tempdir().unwrap();
        let t = targets(dir.path(), &sim("0000-SLOT1", "pipeline-slot-1"));
        let def = mason_definition::parse(
            &DEF.replace("name = \"tdt-*\"", "exclude = [\"pipeline-slot-*\"]"),
        )
        .unwrap();
        let env = def.testing.environment("sim").unwrap();
        assert!(t
            .lease(kind_of_env(env).unwrap(), &filter_of_env(env), "#1 ui")
            .is_none());
        // Without a filter the same simulator is leased: the filter is what keeps it off.
        assert!(t
            .lease(
                DeviceKind::IosSimulator,
                &inventory::Filter::default(),
                "#1 ui"
            )
            .is_some());
    }
}
