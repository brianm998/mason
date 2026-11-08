// SPDX-License-Identifier: GPL-3.0-or-later

//! Reds that are the machine's fault on mobile, never the task's: no device,
//! a device that is offline or locked, an expired certificate or profile.
//! A project gets these only if it declares a device environment, and a
//! signature of its own with the same name wins.

use crate::model::{EnvKind, Signature, Testing};

pub fn device_signatures() -> Vec<Signature> {
    let s = |name: &str, pattern: &str, advice: &str| Signature {
        name: name.into(),
        pattern: pattern.into(),
        advice: advice.into(),
    };
    vec![
        s(
            "no-device",
            r"(?i)(no (connected )?(devices?|simulators?|emulators?)( or emulators?)? (found|available|connected)|device not found|no devices/emulators found|unable to find a destination matching)",
            "plug a device in or start a simulator or emulator",
        ),
        s(
            "device-offline-or-locked",
            r"(?i)(device (is )?(offline|locked)|device (is )?not (connected|trusted)|device (is )?unauthorized|the device is locked|error: closed|could not connect to the device|waiting for device)",
            "unlock the device, trust this computer and allow debugging",
        ),
        s(
            "expired-certificate",
            r"(?i)(certificate[^\n]{0,60}(has )?expired|expired[^\n]{0,40}(certificate|provisioning profile)|provisioning profile[^\n]{0,60}(has )?expired|no valid (signing )?(certificate|identity)|requires a provisioning profile|keystore was tampered)",
            "renew the signing certificate or provisioning profile",
        ),
    ]
}

/// Add the built-in mobile signatures to a project that declares a device
/// environment. A project without one is returned unchanged.
pub fn add_device_signatures(t: &mut Testing) {
    if !t
        .environments
        .iter()
        .any(|e| matches!(e.kind, EnvKind::Device(_)))
    {
        return;
    }
    for s in device_signatures() {
        if !t.signatures.iter().any(|x| x.name == s.name) {
            t.signatures.push(s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{DeviceType, Environment};
    use crate::verdict::{classify, Inputs, RedClass};

    fn env_class(output: &str) -> Option<String> {
        let sigs = device_signatures();
        match classify(&Inputs {
            failing: &["t".to_string()],
            output,
            signatures: &sigs,
            rerun_failing: None,
            baseline_failing: None,
            quarantined: &[],
        }) {
            RedClass::Environmental { signature, .. } => Some(signature),
            _ => None,
        }
    }

    #[test]
    fn real_world_messages_are_environmental() {
        for (text, name) in [
            ("error: no devices/emulators found", "no-device"),
            ("xcodebuild: error: Unable to find a destination matching the provided destination specifier", "no-device"),
            ("error: device offline", "device-offline-or-locked"),
            ("The device is locked. Unlock and retry", "device-offline-or-locked"),
            ("error: The certificate used to sign has expired", "expired-certificate"),
            ("Provisioning profile \"X\" has expired.", "expired-certificate"),
            ("error: No valid signing certificate found", "expired-certificate"),
        ] {
            assert_eq!(env_class(text).as_deref(), Some(name), "{text}");
        }
    }

    #[test]
    fn an_ordinary_failure_is_not_environmental() {
        assert_eq!(env_class("not ok 1 - expected 3, got 4"), None);
        assert_eq!(
            env_class("the certificate parser rejects empty input"),
            None
        );
    }

    #[test]
    fn only_device_projects_get_them() {
        let mut plain = Testing::default();
        plain.environments.push(Environment::local("db"));
        add_device_signatures(&mut plain);
        assert!(plain.signatures.is_empty());

        let mut phone = Testing::default();
        let mut e = Environment::local("phone");
        e.kind = EnvKind::Device(DeviceType::IosDevice);
        phone.environments.push(e);
        phone.signatures.push(Signature {
            name: "no-device".into(),
            pattern: "mine".into(),
            advice: String::new(),
        });
        add_device_signatures(&mut phone);
        assert_eq!(phone.signatures.len(), 3);
        assert_eq!(
            phone.signatures[0].pattern, "mine",
            "the project's own wins"
        );
    }
}
