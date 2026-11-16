// SPDX-License-Identifier: GPL-3.0-or-later

/// What the monitor offers on a platform. The Mac has everything; the iPad shows the same
/// layout without agent sessions (there is no terminal) and without graph editing (the pipeline
/// graph is read-only there, and the edit controls are absent, not disabled).
public struct PlatformFeatures: Equatable, Sendable {
    public var agentSessions: Bool
    public var graphEditing: Bool

    public init(agentSessions: Bool, graphEditing: Bool) {
        self.agentSessions = agentSessions
        self.graphEditing = graphEditing
    }

    public static let mac = PlatformFeatures(agentSessions: true, graphEditing: true)
    public static let iPad = PlatformFeatures(agentSessions: false, graphEditing: false)

    /// The text scale an iPad starts at when the user has not chosen one. 0.9 makes the Mac layout
    /// (top bar, sidebar and page together) fit the 11" iPad's 1180 pt landscape width; a stored
    /// preference always wins.
    public static let iPadTextScale = 0.9

    /// The least a tappable control's hit area is, in points (Apple's touch guideline).
    public static let minTouchTarget = 44.0

    /// Actions the daemon offers that need a terminal, hidden where `agentSessions` is off.
    public func allows(action: String) -> Bool {
        action == "open_session" ? agentSessions : true
    }
}
