// SPDX-License-Identifier: GPL-3.0-or-later

//! `xcrun devicectl list devices --json-output -`.

use mason_adapter_api::device::{Device, DeviceKind, DeviceState};

pub fn parse(text: &str) -> Result<Vec<Device>, String> {
    let v: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("devicectl output is not JSON: {e}"))?;
    let mut out = vec![];
    for d in v["result"]["devices"]
        .as_array()
        .cloned()
        .unwrap_or_default()
    {
        if d["hardwareProperties"]["platform"].as_str() != Some("iOS") {
            continue;
        }
        let Some(id) = d["hardwareProperties"]["udid"]
            .as_str()
            .or_else(|| d["identifier"].as_str())
        else {
            continue;
        };
        let connected = matches!(
            d["connectionProperties"]["tunnelState"].as_str(),
            Some("connected")
        );
        out.push(Device {
            id: id.into(),
            kind: DeviceKind::IosDevice,
            os: d["deviceProperties"]["osVersionNumber"]
                .as_str()
                .unwrap_or("")
                .into(),
            name: d["deviceProperties"]["name"].as_str().unwrap_or(id).into(),
            state: if connected {
                DeviceState::Ready
            } else {
                DeviceState::Unavailable
            },
        });
    }
    Ok(out)
}
