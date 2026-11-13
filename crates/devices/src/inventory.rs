// SPDX-License-Identifier: GPL-3.0-or-later

//! Two listings of devices become capacity changes; a lease picks a device.

use mason_adapter_api::device::{Device, DeviceKind, DeviceState};
use std::collections::BTreeMap;

/// Ready devices per kind. Every kind is present, with zero if none: a pool
/// with no units is a wait, and a name nobody declared is not.
pub fn counts(devices: &[Device]) -> BTreeMap<DeviceKind, u32> {
    let mut m: BTreeMap<DeviceKind, u32> = DeviceKind::ALL.into_iter().map(|k| (k, 0)).collect();
    for d in devices.iter().filter(|d| d.state == DeviceState::Ready) {
        *m.entry(d.kind).or_insert(0) += 1;
    }
    m
}

/// The kinds whose unit count differs between two readings.
pub fn changes(
    old: &BTreeMap<DeviceKind, u32>,
    new: &BTreeMap<DeviceKind, u32>,
) -> Vec<(DeviceKind, u32)> {
    new.iter()
        .filter(|(k, n)| old.get(k) != Some(n))
        .map(|(k, n)| (*k, *n))
        .collect()
}

/// Which devices of a kind an environment may lease: an OS version prefix,
/// a glob on the name, and globs on name or id that rule a device out. The
/// empty filter admits every device.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Filter {
    pub os: Option<String>,
    pub name: Option<String>,
    pub exclude: Vec<String>,
}

impl Filter {
    pub fn admits(&self, d: &Device) -> bool {
        self.os.as_deref().is_none_or(|p| d.os.starts_with(p))
            && self.name.as_deref().is_none_or(|g| glob(g, &d.name))
            && !self
                .exclude
                .iter()
                .any(|g| glob(g, &d.name) || glob(g, &d.id))
    }
}

/// `*` matches any run of characters, `?` exactly one; everything else is literal.
pub fn glob(pattern: &str, text: &str) -> bool {
    fn go(g: &[char], s: &[char]) -> bool {
        match g.split_first() {
            None => s.is_empty(),
            Some(('*', rest)) => (0..=s.len()).any(|n| go(rest, &s[n..])),
            Some(('?', rest)) => !s.is_empty() && go(rest, &s[1..]),
            Some((c, rest)) => s.first() == Some(c) && go(rest, &s[1..]),
        }
    }
    let (g, s): (Vec<char>, Vec<char>) = (pattern.chars().collect(), text.chars().collect());
    go(&g, &s)
}

/// The lowest-id ready device of a kind that the filter admits and `taken`
/// does not claim.
pub fn pick<'a>(
    devices: &'a [Device],
    kind: DeviceKind,
    filter: &Filter,
    taken: &dyn Fn(&str) -> bool,
) -> Option<&'a Device> {
    devices
        .iter()
        .filter(|d| d.kind == kind && d.state == DeviceState::Ready)
        .filter(|d| filter.admits(d))
        .filter(|d| !taken(&d.id))
        .min_by(|a, b| a.id.cmp(&b.id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(id: &str, kind: DeviceKind, os: &str, state: DeviceState) -> Device {
        Device {
            id: id.into(),
            kind,
            os: os.into(),
            name: id.into(),
            state,
        }
    }

    #[test]
    fn counts_include_every_kind_and_skip_unavailable_devices() {
        let v = vec![
            dev("a", DeviceKind::AndroidEmulator, "14", DeviceState::Ready),
            dev(
                "b",
                DeviceKind::AndroidEmulator,
                "14",
                DeviceState::Unavailable,
            ),
        ];
        let c = counts(&v);
        assert_eq!(c[&DeviceKind::AndroidEmulator], 1);
        assert_eq!(c[&DeviceKind::IosDevice], 0);
        assert_eq!(c.len(), 4);
    }

    #[test]
    fn only_changed_kinds_are_reported() {
        let a = counts(&[dev("a", DeviceKind::IosDevice, "17", DeviceState::Ready)]);
        let b = counts(&[]);
        assert_eq!(changes(&a, &b), vec![(DeviceKind::IosDevice, 0)]);
        assert!(changes(&a, &a).is_empty());
    }

    #[test]
    fn pick_takes_the_lowest_free_match() {
        let v = vec![
            dev("b", DeviceKind::IosSimulator, "17.5", DeviceState::Ready),
            dev("a", DeviceKind::IosSimulator, "16.4", DeviceState::Ready),
            dev("c", DeviceKind::IosSimulator, "17.5", DeviceState::Ready),
        ];
        let free = |_: &str| false;
        let any = Filter::default();
        let os17 = Filter {
            os: Some("17".into()),
            ..Filter::default()
        };
        assert_eq!(
            pick(&v, DeviceKind::IosSimulator, &any, &free).unwrap().id,
            "a"
        );
        assert_eq!(
            pick(&v, DeviceKind::IosSimulator, &os17, &free).unwrap().id,
            "b"
        );
        let a_taken = |id: &str| id == "a";
        assert_eq!(
            pick(&v, DeviceKind::IosSimulator, &any, &a_taken)
                .unwrap()
                .id,
            "b"
        );
        assert!(pick(&v, DeviceKind::IosDevice, &any, &free).is_none());
    }

    fn named(id: &str, name: &str) -> Device {
        Device {
            name: name.into(),
            ..dev(id, DeviceKind::IosSimulator, "26.1", DeviceState::Ready)
        }
    }

    // Another pipeline's simulators sort first by id; a name glob keeps the
    // environment off them even when they are the only free ones.
    #[test]
    fn a_name_glob_never_picks_another_pipelines_simulator() {
        let v = vec![
            named("0A", "pipeline-slot-1"),
            named("0B", "pipeline-slot-2"),
            named("9Z", "tdt-1"),
        ];
        let f = Filter {
            name: Some("tdt-*".into()),
            ..Filter::default()
        };
        let free = |_: &str| false;
        assert_eq!(
            pick(&v, DeviceKind::IosSimulator, &f, &free).unwrap().name,
            "tdt-1"
        );
        let tdt_taken = |id: &str| id == "9Z";
        assert!(
            pick(&v, DeviceKind::IosSimulator, &f, &tdt_taken).is_none(),
            "with its own device busy it waits; it does not fall back to pipeline-slot-*"
        );
    }

    #[test]
    fn exclude_rules_out_by_name_or_id() {
        let v = vec![
            named("0A", "pipeline-slot-1"),
            named("0B", "iPhone 17"),
            named("0C", "iPhone 17 Pro"),
        ];
        let f = Filter {
            exclude: vec!["pipeline-slot-*".into(), "0B".into()],
            ..Filter::default()
        };
        let free = |_: &str| false;
        assert_eq!(
            pick(&v, DeviceKind::IosSimulator, &f, &free).unwrap().id,
            "0C"
        );
        assert!(!f.admits(&v[0]) && !f.admits(&v[1]));
    }

    #[test]
    fn globs_on_names() {
        for (g, s, want) in [
            ("tdt-*", "tdt-1", true),
            ("tdt-*", "tdt-", true),
            ("tdt-*", "pipeline-slot-1", false),
            ("*slot*", "pipeline-slot-1", true),
            ("iPhone ?? Pro", "iPhone 17 Pro", true),
            ("iPhone ?? Pro", "iPhone 7 Pro", false),
            ("exact", "exact", true),
            ("exact", "exactly", false),
        ] {
            assert_eq!(glob(g, s), want, "{g} vs {s}");
        }
    }
}
