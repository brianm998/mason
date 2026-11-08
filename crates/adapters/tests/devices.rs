// SPDX-License-Identifier: GPL-3.0-or-later

//! The parsers read fixtures that were written by hand from the tools'
//! documented formats. They were NOT recorded from real simctl, adb or
//! devicectl runs; a real run is left to the owner (docs/stages/s25.md).

use mason_adapter_api::device::{DeviceKind, DeviceState};
use mason_adapters::devices::{adb, devicectl, simctl};

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/tests/fixtures/devices/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

#[test]
fn simctl_lists_ios_simulators_only_and_marks_unavailable_ones() {
    let text = fixture("simctl-list.json");
    let v = simctl::parse(&text).unwrap();
    assert_eq!(v.len(), 3, "the watch is not ours");
    assert!(v
        .iter()
        .all(|d| d.kind == DeviceKind::IosSimulator && d.os == "17.5"));
    assert_eq!(
        v.iter().filter(|d| d.state == DeviceState::Ready).count(),
        2
    );
    assert_eq!(
        simctl::booted(&text),
        vec!["A1B2C3D4-0000-0000-0000-000000000001"]
    );
    assert!(simctl::parse("not json").is_err());
}

#[test]
fn adb_tells_emulators_from_phones_and_usable_from_not() {
    let v = adb::parse(&fixture("adb-devices.txt"));
    assert_eq!(v.len(), 4);
    let by = |id: &str| v.iter().find(|d| d.id == id).unwrap();
    assert_eq!(by("emulator-5554").kind, DeviceKind::AndroidEmulator);
    assert_eq!(by("emulator-5554").state, DeviceState::Ready);
    assert_eq!(by("R58M4ABCDEF").kind, DeviceKind::AndroidDevice);
    assert_eq!(by("R58M4ABCDEF").name, "SM_G973F");
    assert_eq!(by("0123456789ABCDEF").state, DeviceState::Unavailable);
    assert_eq!(by("emulator-5556").state, DeviceState::Unavailable);
    assert!(adb::parse("").is_empty());
}

#[test]
fn devicectl_lists_connected_iphones_and_skips_other_platforms() {
    let v = devicectl::parse(&fixture("devicectl-list.json")).unwrap();
    assert_eq!(v.len(), 2);
    assert_eq!(v[0].id, "00008110-001234567890801E");
    assert_eq!(v[0].state, DeviceState::Ready);
    assert_eq!(v[0].os, "17.5.1");
    assert_eq!(v[1].state, DeviceState::Unavailable);
}
