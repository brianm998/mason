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

    /// The state holding the most tasks among the active ones (history, landed and canceled,
    /// is left out), from the header's own counts, so it follows the project filter. A tie goes
    /// to the later stage in `activeStatesInOrder`. Nil with no tasks or while disconnected.
    public var busiestState: (state: String, count: UInt32)? {
        guard connected, let counts = header?.counts else { return nil }
        var best: (state: String, count: UInt32, rank: Int)?
        for c in counts where c.count > 0 {
            guard let rank = Self.activeStatesInOrder.firstIndex(of: c.state) else { continue }
            if best == nil || c.count > best!.count || (c.count == best!.count && rank > best!.rank) {
                best = (c.state, c.count, rank)
            }
        }
        return best.map { ($0.state, $0.count) }
    }

    /// The active states, earliest stage first.
    public static let activeStatesInOrder = [
        "queued", "exploring", "developing", "testing", "review", "landing",
        "needs_attention", "failed", "proposed",
    ]

    /// The top bar's picture: the busiest state's illustration, the idle art with no tasks.
    public var topBarMascot: MascotChoice? {
        guard connected, header != nil else { return nil }
        if let b = busiestState { return MascotChoice(id: b.state, large: false) }
        return MascotChoice(id: "all_clear", large: false)
    }

    /// "Most tasks: testing (7)"; "No active tasks" for the idle art.
    public var topBarMascotTooltip: String {
        guard let b = busiestState else { return "No active tasks" }
        let label = header?.counts.first { $0.state == b.state }.map(tileLabel) ?? b.state
        return "Most tasks: \(label) (\(b.count))"
    }

    /// Clicking the top bar picture reveals that state's sidebar section.
    public func topBarMascotClicked() {
        if let b = busiestState { reveal(state: b.state) }
    }
}
