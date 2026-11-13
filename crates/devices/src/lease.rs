// SPDX-License-Identifier: GPL-3.0-or-later

//! One device, one holder. The pool counts units; this makes a particular
//! device real with a kernel `flock` on `device-<id>.lock`, so a holder that
//! dies frees it, the way machine classes do.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub struct Leases {
    dir: PathBuf,
    held: Mutex<BTreeMap<String, String>>,
}

pub struct DeviceLease {
    pub id: String,
    leases: Arc<Leases>,
    _lock: File,
}

impl Drop for DeviceLease {
    fn drop(&mut self) {
        self.leases
            .held
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.id);
    }
}

fn safe(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

impl Leases {
    pub fn new(dir: impl Into<PathBuf>) -> std::io::Result<Arc<Leases>> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)?;
        Ok(Arc::new(Leases {
            dir,
            held: Mutex::default(),
        }))
    }

    /// Take a device if nobody has it. `label` says who (for `mason targets list`).
    pub fn try_take(self: &Arc<Self>, id: &str, label: &str) -> Option<DeviceLease> {
        let path = self.dir.join(format!("device-{}.lock", safe(id)));
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .ok()?;
        file.try_lock().ok()?;
        let lock = file;
        self.held
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.to_string(), label.to_string());
        let _ = std::fs::write(path.with_extension("holder"), format!("{label}\n"));
        Some(DeviceLease {
            id: id.to_string(),
            leases: self.clone(),
            _lock: lock,
        })
    }

    /// Devices held right now by this process, and by whom.
    pub fn held(&self) -> BTreeMap<String, String> {
        self.held.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn is_held(&self, id: &str) -> bool {
        self.held
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_device_has_one_holder_until_it_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let l = Leases::new(dir.path()).unwrap();
        let a = l.try_take("emu-5554", "#1").unwrap();
        assert!(l.try_take("emu-5554", "#2").is_none());
        assert!(l.try_take("emu-5556", "#2").is_some());
        assert_eq!(l.held().get("emu-5554").map(String::as_str), Some("#1"));
        drop(a);
        assert!(!l.is_held("emu-5554"));
        assert!(l.try_take("emu-5554", "#2").is_some());
    }

    #[test]
    fn another_process_view_of_the_same_directory_sees_the_lock() {
        let dir = tempfile::tempdir().unwrap();
        let one = Leases::new(dir.path()).unwrap();
        let two = Leases::new(dir.path()).unwrap();
        let _a = one.try_take("x/y", "#1").unwrap();
        assert!(two.try_take("x/y", "#2").is_none(), "the kernel lock holds");
    }
}
