// SPDX-License-Identifier: GPL-3.0-or-later

// The phone's screens live in `MonitorUI` (`Phone*.swift`), so they share its pages and are
// tested with them; this target keeps the `MonitorPhone` product the iOS app imports.
@_exported import MonitorUI
