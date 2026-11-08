// SPDX-License-Identifier: GPL-3.0-or-later

//! The conformance kit every device provider passes: a stable sorted listing,
//! a lease target that is a listed device, a repeatable health check, an
//! idempotent teardown, a vanished device that leaves the next listing, and
//! errors that are classified.

use crate::device::{DeviceProvider, DeviceState, ProviderError};

/// `vanish(id)` removes the device from whatever the provider reads (unplug it).
pub fn conformance(p: &dyn DeviceProvider, vanish: &dyn Fn(&str)) -> Result<(), String> {
    let name = p.name();
    let first = p.list().map_err(|e| format!("{name}: list failed: {e}"))?;
    let second = p.list().map_err(|e| format!("{name}: list failed: {e}"))?;
    if first != second {
        return Err(format!("{name}: two listings in a row differ"));
    }
    if first.windows(2).any(|w| w[0].id >= w[1].id) {
        return Err(format!("{name}: the listing is not sorted by unique id"));
    }
    let ready: Vec<_> = first
        .iter()
        .filter(|d| d.state == DeviceState::Ready)
        .collect();
    let Some(target) = ready.first() else {
        return Err(format!("{name}: the kit needs at least one ready device"));
    };
    let id = target.id.as_str();
    p.prepare(id)
        .map_err(|e| format!("{name}: prepare of a listed device failed: {e}"))?;
    for _ in 0..2 {
        p.health(id)
            .map_err(|e| format!("{name}: health is not repeatable: {e}"))?;
    }
    for _ in 0..2 {
        p.teardown(id)
            .map_err(|e| format!("{name}: teardown is not idempotent: {e}"))?;
    }
    match p.prepare("no-such-device") {
        Err(ProviderError::Permanent(_)) => {}
        other => {
            return Err(format!(
                "{name}: preparing an unknown device must be a permanent error, got {other:?}"
            ))
        }
    }
    vanish(id);
    let after = p.list().map_err(|e| format!("{name}: list failed: {e}"))?;
    if after
        .iter()
        .any(|d| d.id == id && d.state == DeviceState::Ready)
    {
        return Err(format!(
            "{name}: a vanished device is still listed as ready"
        ));
    }
    if after.len() + 1 < first.len() {
        return Err(format!("{name}: more than the vanished device left"));
    }
    if let Err(e) = p.teardown(id) {
        return Err(format!("{name}: teardown of a vanished device failed: {e}"));
    }
    match p.health(id) {
        Err(_) => {}
        Ok(()) => return Err(format!("{name}: a vanished device reported healthy")),
    }
    Ok(())
}
