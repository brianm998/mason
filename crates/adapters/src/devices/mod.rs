// SPDX-License-Identifier: GPL-3.0-or-later

//! The real device provider: asks `xcrun simctl`, `adb` and `xcrun devicectl`
//! what exists, and boots, checks and shuts down what a task leases. The
//! parsers are pure text-to-devices functions (tested on fixtures in
//! `tests/fixtures/devices`); only [`System`] runs commands.
//!
//! A tool that is not installed has no devices. It is not an error: a Linux
//! machine simply has no simulators.

pub mod adb;
pub mod devicectl;
pub mod simctl;

use mason_adapter_api::device::{sorted, Device, DeviceKind, DeviceProvider, ProviderError};
use std::collections::HashSet;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Run a tool, giving up after `limit`. `None` if it is not installed.
fn run(program: &str, args: &[&str], limit: Duration) -> Result<Option<String>, ProviderError> {
    let mut child = match Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(ProviderError::Permanent(format!(
                "cannot run {program}: {e}"
            )))
        }
    };
    let end = Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() >= end => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(ProviderError::Transient(format!(
                    "{program} {} took longer than {}s",
                    args.join(" "),
                    limit.as_secs()
                )));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(ProviderError::Transient(format!("{program}: {e}"))),
        }
    }
    let out = child
        .wait_with_output()
        .map_err(|e| ProviderError::Transient(format!("{program}: {e}")))?;
    if !out.status.success() {
        return Err(ProviderError::Transient(format!(
            "{program} {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(Some(String::from_utf8_lossy(&out.stdout).into_owned()))
}

/// simctl, adb and devicectl, on this machine.
#[derive(Default)]
pub struct System {
    /// Simulators and emulators this provider booted: the only ones it shuts down.
    started: Mutex<HashSet<String>>,
}

impl System {
    pub fn new() -> System {
        System::default()
    }

    fn find(&self, id: &str) -> Result<Device, ProviderError> {
        self.list()?
            .into_iter()
            .find(|d| d.id == id)
            .ok_or_else(|| ProviderError::Permanent(format!("no device `{id}`")))
    }
}

impl DeviceProvider for System {
    fn name(&self) -> &'static str {
        "system"
    }

    fn list(&self) -> Result<Vec<Device>, ProviderError> {
        let t = Duration::from_secs(8);
        let mut v = vec![];
        if let Some(out) = run(
            "xcrun",
            &["simctl", "list", "devices", "available", "--json"],
            t,
        )? {
            v.extend(simctl::parse(&out).map_err(ProviderError::Permanent)?);
        }
        if let Some(out) = run(
            "xcrun",
            &["devicectl", "list", "devices", "--json-output", "-"],
            t,
        )? {
            v.extend(devicectl::parse(&out).map_err(ProviderError::Permanent)?);
        }
        if let Some(out) = run("adb", &["devices", "-l"], t)? {
            v.extend(adb::parse(&out));
        }
        Ok(sorted(v))
    }

    fn prepare(&self, id: &str) -> Result<(), ProviderError> {
        let d = self.find(id)?;
        let t = Duration::from_secs(120);
        let started = match d.kind {
            DeviceKind::IosSimulator => {
                let booted = simctl_booted(id)?;
                if !booted {
                    run("xcrun", &["simctl", "boot", id], t)?;
                    run(
                        "xcrun",
                        &["simctl", "bootstatus", id, "-b"],
                        Duration::from_secs(300),
                    )?;
                }
                !booted
            }
            // An emulator is started by whoever made it; we only wait for it to be there.
            _ => false,
        };
        if started {
            self.started
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(id.into());
        }
        Ok(())
    }

    fn health(&self, id: &str) -> Result<(), ProviderError> {
        let d = self.find(id)?;
        match d.kind {
            DeviceKind::IosSimulator if !simctl_booted(id)? => Err(ProviderError::Transient(
                format!("simulator {id} is not booted"),
            )),
            DeviceKind::AndroidEmulator | DeviceKind::AndroidDevice => {
                let out = run(
                    "adb",
                    &["-s", id, "shell", "getprop", "sys.boot_completed"],
                    Duration::from_secs(10),
                )?;
                match out {
                    Some(s) if s.trim() == "1" => Ok(()),
                    _ => Err(ProviderError::Transient(format!(
                        "{id} has not finished booting"
                    ))),
                }
            }
            _ => Ok(()),
        }
    }

    fn teardown(&self, id: &str) -> Result<(), ProviderError> {
        let ours = self
            .started
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id);
        if ours {
            let _ = run(
                "xcrun",
                &["simctl", "shutdown", id],
                Duration::from_secs(60),
            )?;
        }
        Ok(())
    }
}

fn simctl_booted(id: &str) -> Result<bool, ProviderError> {
    let out = run(
        "xcrun",
        &["simctl", "list", "devices", "--json"],
        Duration::from_secs(10),
    )?;
    Ok(out.is_some_and(|o| simctl::booted(&o).iter().any(|b| b == id)))
}
