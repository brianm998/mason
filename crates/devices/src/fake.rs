// SPDX-License-Identifier: GPL-3.0-or-later

//! A provider that reads its devices from a JSON file, read afresh on every
//! call so a test can plug a device in or out by rewriting it, and writes each
//! lifecycle call to a log: `setup <id>`, `health <id>`, `teardown <id>`.
//!
//! ```json
//! {"devices":[{"id":"emu-1","kind":"android-emulator","os":"14","name":"Pixel","state":"ready"}]}
//! ```

use mason_adapter_api::device::{
    sorted, Device, DeviceKind, DeviceProvider, DeviceState, ProviderError,
};
use std::io::Write;
use std::path::{Path, PathBuf};

pub struct FakeProvider {
    file: PathBuf,
    log: PathBuf,
}

impl FakeProvider {
    pub fn new(file: impl Into<PathBuf>, log: Option<PathBuf>) -> FakeProvider {
        let file = file.into();
        let log = log.unwrap_or_else(|| {
            let mut s = file.clone().into_os_string();
            s.push(".calls");
            PathBuf::from(s)
        });
        FakeProvider { file, log }
    }

    pub fn log_path(&self) -> &Path {
        &self.log
    }

    fn note(&self, what: &str, id: &str) {
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log)
        {
            let _ = writeln!(f, "{what} {id}");
        }
    }

    fn read(&self) -> Result<Vec<Device>, ProviderError> {
        let text = match std::fs::read_to_string(&self.file) {
            Ok(t) => t,
            // No file is no devices: nothing plugged in.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => {
                return Err(ProviderError::Transient(format!(
                    "cannot read the fixture: {e}"
                )))
            }
        };
        // A file caught half-written reads as a transient error, not as no devices.
        let v: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| ProviderError::Transient(format!("the fixture is not JSON yet: {e}")))?;
        let mut out = vec![];
        for d in v["devices"].as_array().cloned().unwrap_or_default() {
            let s = |k: &str| d[k].as_str().unwrap_or("").to_string();
            let kind = DeviceKind::from_name(&s("kind")).ok_or_else(|| {
                ProviderError::Permanent(format!(
                    "the fixture has a device of kind `{}`",
                    s("kind")
                ))
            })?;
            out.push(Device {
                id: s("id"),
                kind,
                os: s("os"),
                name: s("name"),
                state: if s("state") == "offline" {
                    DeviceState::Unavailable
                } else {
                    DeviceState::Ready
                },
            });
        }
        Ok(sorted(out))
    }

    fn find(&self, id: &str) -> Result<Device, ProviderError> {
        self.read()?
            .into_iter()
            .find(|d| d.id == id)
            .ok_or_else(|| ProviderError::Permanent(format!("no device `{id}`")))
    }
}

impl DeviceProvider for FakeProvider {
    fn name(&self) -> &'static str {
        "fake"
    }

    fn list(&self) -> Result<Vec<Device>, ProviderError> {
        self.read()
    }

    fn prepare(&self, id: &str) -> Result<(), ProviderError> {
        self.note("setup", id);
        let d = self.find(id)?;
        match d.state {
            DeviceState::Ready => Ok(()),
            DeviceState::Unavailable => Err(ProviderError::Transient(format!("`{id}` is offline"))),
        }
    }

    fn health(&self, id: &str) -> Result<(), ProviderError> {
        self.note("health", id);
        self.prepare_check(id)
    }

    fn teardown(&self, id: &str) -> Result<(), ProviderError> {
        self.note("teardown", id);
        Ok(())
    }
}

impl FakeProvider {
    fn prepare_check(&self, id: &str) -> Result<(), ProviderError> {
        match self.find(id)?.state {
            DeviceState::Ready => Ok(()),
            DeviceState::Unavailable => Err(ProviderError::Transient(format!("`{id}` is offline"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(p: &Path, ids: &[&str]) {
        let devs: Vec<String> = ids
            .iter()
            .map(|i| format!(r#"{{"id":"{i}","kind":"android-emulator","os":"14","name":"{i}"}}"#))
            .collect();
        std::fs::write(p, format!(r#"{{"devices":[{}]}}"#, devs.join(","))).unwrap();
    }

    #[test]
    fn the_fake_passes_the_conformance_kit() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("devices.json");
        write(&f, &["emu-2", "emu-1"]);
        let p = FakeProvider::new(&f, None);
        mason_adapter_api::device_kit::conformance(&p, &|id| {
            let rest: Vec<&str> = ["emu-1", "emu-2"]
                .into_iter()
                .filter(|x| *x != id)
                .collect();
            write(&f, &rest);
        })
        .unwrap();
    }

    #[test]
    fn calls_are_logged_in_order_and_a_missing_file_is_no_devices() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("none.json");
        let p = FakeProvider::new(&f, None);
        assert!(p.list().unwrap().is_empty());
        write(&f, &["emu-1"]);
        p.prepare("emu-1").unwrap();
        p.health("emu-1").unwrap();
        p.teardown("emu-1").unwrap();
        let log = std::fs::read_to_string(p.log_path()).unwrap();
        assert_eq!(log, "setup emu-1\nhealth emu-1\nteardown emu-1\n");
    }

    #[test]
    fn a_half_written_file_is_transient_not_empty() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("d.json");
        std::fs::write(&f, "{\"devices\":[").unwrap();
        assert!(matches!(
            FakeProvider::new(&f, None).list(),
            Err(ProviderError::Transient(_))
        ));
    }
}
