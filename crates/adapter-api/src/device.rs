// SPDX-License-Identifier: GPL-3.0-or-later

//! What a device provider is (Part 7): something that can list the simulators,
//! emulators and phones a machine has, and get one ready, check it, and let it
//! go. The daemon decides who may use which device (leases); a provider only
//! knows devices. Every error says whether waiting may help.

/// What a device is. The capability a task asks for is `device:<name>`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DeviceKind {
    IosSimulator,
    AndroidEmulator,
    IosDevice,
    AndroidDevice,
}

impl DeviceKind {
    pub const ALL: [DeviceKind; 4] = [
        DeviceKind::IosSimulator,
        DeviceKind::AndroidEmulator,
        DeviceKind::IosDevice,
        DeviceKind::AndroidDevice,
    ];

    pub fn name(self) -> &'static str {
        match self {
            DeviceKind::IosSimulator => "ios-simulator",
            DeviceKind::AndroidEmulator => "android-emulator",
            DeviceKind::IosDevice => "ios-device",
            DeviceKind::AndroidDevice => "android-device",
        }
    }

    pub fn from_name(s: &str) -> Option<DeviceKind> {
        Self::ALL.into_iter().find(|k| k.name() == s)
    }

    /// The resource a task leases.
    pub fn capability(self) -> String {
        format!("device:{}", self.name())
    }

    /// For sentences: "an iPhone", "an Android emulator".
    pub fn noun(self) -> &'static str {
        match self {
            DeviceKind::IosSimulator => "an iOS simulator",
            DeviceKind::AndroidEmulator => "an Android emulator",
            DeviceKind::IosDevice => "an iPhone",
            DeviceKind::AndroidDevice => "an Android phone",
        }
    }

    pub fn physical(self) -> bool {
        matches!(self, DeviceKind::IosDevice | DeviceKind::AndroidDevice)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceState {
    /// Can run a suite now (a simulator that is shut down counts: preparing it boots it).
    Ready,
    /// Seen but not usable: offline, locked, or untrusted. Not counted as capacity.
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    /// Stable for the life of the device (a UDID or a serial).
    pub id: String,
    pub kind: DeviceKind,
    /// Operating system version, like "17.5" or "14".
    pub os: String,
    pub name: String,
    pub state: DeviceState,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderError {
    /// May pass: a tool timed out, the device is still booting.
    Transient(String),
    /// Will not pass by waiting: no such device, a tool refuses.
    Permanent(String),
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProviderError::Transient(s) => write!(f, "{s} (may pass)"),
            ProviderError::Permanent(s) => write!(f, "{s} (will not pass by waiting)"),
        }
    }
}

pub trait DeviceProvider: Send + Sync {
    fn name(&self) -> &'static str;
    /// Every device now, sorted by id. A tool that is not installed is not an
    /// error: it has no devices.
    fn list(&self) -> Result<Vec<Device>, ProviderError>;
    /// Get a device ready (boot a simulator). Starts only what is not running.
    fn prepare(&self, id: &str) -> Result<(), ProviderError>;
    /// Is it usable now? Repeatable, changes nothing.
    fn health(&self, id: &str) -> Result<(), ProviderError>;
    /// Undo `prepare`: stop only what `prepare` started. Safe to call twice,
    /// and for a device that has gone.
    fn teardown(&self, id: &str) -> Result<(), ProviderError>;
}

/// Order a listing the way every provider must return it.
pub fn sorted(mut v: Vec<Device>) -> Vec<Device> {
    v.sort_by(|a, b| a.id.cmp(&b.id));
    v
}
