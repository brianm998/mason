// SPDX-License-Identifier: GPL-3.0-or-later

//! `xcrun simctl list devices --json`.

use mason_adapter_api::device::{Device, DeviceKind, DeviceState};

fn walk(text: &str) -> Result<Vec<(Device, bool)>, String> {
    let v: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("simctl output is not JSON: {e}"))?;
    let mut out = vec![];
    let Some(map) = v["devices"].as_object() else {
        return Err("simctl output has no `devices`".into());
    };
    for (runtime, list) in map {
        // "com.apple.CoreSimulator.SimRuntime.iOS-17-5": other platforms are not ours.
        let Some(tail) = runtime.rsplit('.').next() else {
            continue;
        };
        let Some(ver) = tail.strip_prefix("iOS-") else {
            continue;
        };
        for d in list.as_array().cloned().unwrap_or_default() {
            let (Some(id), Some(name)) = (d["udid"].as_str(), d["name"].as_str()) else {
                continue;
            };
            out.push((
                Device {
                    id: id.into(),
                    kind: DeviceKind::IosSimulator,
                    os: ver.replace('-', "."),
                    name: name.into(),
                    state: if d["isAvailable"].as_bool().unwrap_or(false) {
                        DeviceState::Ready
                    } else {
                        DeviceState::Unavailable
                    },
                },
                d["state"].as_str() == Some("Booted"),
            ));
        }
    }
    Ok(out)
}

pub fn parse(text: &str) -> Result<Vec<Device>, String> {
    Ok(walk(text)?.into_iter().map(|(d, _)| d).collect())
}

/// The ids of simulators that are booted now.
pub fn booted(text: &str) -> Vec<String> {
    walk(text)
        .unwrap_or_default()
        .into_iter()
        .filter(|(_, b)| *b)
        .map(|(d, _)| d.id)
        .collect()
}
