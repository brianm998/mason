// SPDX-License-Identifier: GPL-3.0-or-later

//! The wait for a person to plug a device in: what it waits for, in words,
//! starting "environment problem" so it is an alert and charged to no task.

use mason_adapter_api::device::DeviceKind;

/// The device kind a resource name asks for, if it is a device capability.
pub fn kind_of(resource: &str) -> Option<DeviceKind> {
    DeviceKind::from_name(resource.strip_prefix("device:")?)
}

pub fn no_device(kind: DeviceKind) -> String {
    let how = match kind {
        DeviceKind::IosDevice => "none is connected. Plug one in and unlock it",
        DeviceKind::AndroidDevice => {
            "none is connected. Plug one in, unlock it and allow USB debugging"
        }
        DeviceKind::IosSimulator => {
            "none exists. Create one in Xcode (Window, Devices and Simulators)"
        }
        DeviceKind::AndroidEmulator => "none exists. Create one in Android Studio's Device Manager",
    };
    format!(
        "environment problem, not the task's fault: waiting for {} ({}): {how}; the task resumes when it appears",
        kind.noun(),
        kind.capability()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sentence_names_what_is_awaited_and_blames_nobody() {
        let s = no_device(DeviceKind::IosDevice);
        assert!(s.starts_with("environment problem, not the task's fault"));
        assert!(s.contains("device:ios-device") && s.contains("an iPhone"));
        assert!(s.contains("Plug one in"));
    }

    #[test]
    fn only_device_resources_have_a_kind() {
        assert_eq!(
            kind_of("device:android-emulator"),
            Some(DeviceKind::AndroidEmulator)
        );
        assert_eq!(kind_of("compile"), None);
        assert_eq!(kind_of("device:toaster"), None);
    }
}
