// SPDX-License-Identifier: GPL-3.0-or-later
//
// Which mascot picture a screen shows, chosen only from the daemon's own state. The ids are
// names in clients/mascot.toml; the views load them with `Mascot.illustration`, which gives
// nothing for an id it does not know.

import Foundation

public struct MascotChoice: Equatable, Sendable {
    public var id: String
    /// True for the large picture (alone on an empty screen, with an action).
    public var large: Bool
}

/// The text shown with the picture when a sidebar search matches nothing.
public let searchNoMatchText = "Nothing matched."

extension MonitorModel {
    /// The Home status picture. Slow and quiet tasks do not change it (slow is not an alarm).
    public var homeMascot: MascotChoice {
        if !connected { return MascotChoice(id: "not_connected", large: false) }
        if attention > 0 { return MascotChoice(id: "needs_attention", large: false) }
        if !(home?.active ?? []).isEmpty { return MascotChoice(id: "developing", large: false) }
        let none = (home?.stages ?? []).allSatisfy { $0.rows.isEmpty } && (home?.attention ?? []).isEmpty
        if home != nil && none { return MascotChoice(id: "no_tasks", large: true) }
        return MascotChoice(id: "all_clear", large: false)
    }

    /// True when a search is on and no section has a matching row.
    public var searchMatchesNothing: Bool {
        search.isActive && (sidebar?.sections ?? []).allSatisfy { filteredRows($0).isEmpty }
    }
}
