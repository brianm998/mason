// SPDX-License-Identifier: GPL-3.0-or-later

//! `adb devices -l`.

use mason_adapter_api::device::{Device, DeviceKind, DeviceState};

pub fn parse(text: &str) -> Vec<Device> {
    text.lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let serial = it.next()?;
            let state = it.next()?;
            // Skips "List of devices attached" and the daemon start-up lines.
            if serial == "List" || serial.starts_with('*') {
                return None;
            }
            let model = it
                .find_map(|w| w.strip_prefix("model:"))
                .unwrap_or(serial)
                .to_string();
            Some(Device {
                id: serial.into(),
                kind: if serial.starts_with("emulator-") {
                    DeviceKind::AndroidEmulator
                } else {
                    DeviceKind::AndroidDevice
                },
                os: String::new(),
                name: model,
                // offline, unauthorized and no permissions are all "not usable".
                state: if state == "device" {
                    DeviceState::Ready
                } else {
                    DeviceState::Unavailable
                },
            })
        })
        .collect()
}
