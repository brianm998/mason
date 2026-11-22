// SPDX-License-Identifier: GPL-3.0-or-later

import Foundation
import MasonProtocol

/// One row of the notification bar: what it is, not where it sits.
public struct NotificationRow: Equatable, Identifiable {
    public enum Kind: String { case client, push, health, paused, banner, pending }
    /// Stable across refreshes and reordering; what a dismissal is kept under.
    public let key: String
    public var id: String { key }
    public let kind: Kind
    /// The accessibility id of the row (and of its button).
    public let uiID: String
    public let text: String
    /// The action button's title, if the row has one.
    public let button: String?
    public let icon: String?
    /// The daemon banner's kind, which picks its image.
    public let daemonKind: String?
    /// Index into `header.banners` (daemon banners, and the expanded pause).
    public let bannerIndex: Int?
    /// Collapsed pause banners drawn as icons in this row.
    public let collapsed: [Int]
    /// The pending issue's number.
    public let issue: Int?
    /// Which client row this is (`connection`, `daemon-update`, …).
    public let clientID: String?
}

extension MonitorModel {
    public static let visibleNotificationRows = 3
    /// The share of the window the notification bar may take.
    public static let notificationWindowShare = 0.6

    static func bannerKey(_ b: Mason_V1_Banner) -> String {
        sanitizeKey("banner:\(b.kind):\(b.project):\(b.mode):\(b.session):\(b.text)")
    }

    private static func sanitizeKey(_ s: String) -> String {
        s.replacingOccurrences(of: "\n", with: " ")
    }

    /// An older daemon sends no level: its banners read as warnings, so they stay rows.
    static func isInfo(_ b: Mason_V1_Banner) -> Bool { b.level == "info" }

    /// The masond update is only progressing (fetching, building, testing, restarting).
    public var daemonUpdateIsInfo: Bool { updateInfo?.daemonLevel == "info" }

    /// Everything the bar leaves out, one line each, newest first as the daemon sent them.
    public var infoNotices: [String] {
        var out: [String] = []
        if !daemonUpdateText.isEmpty && daemonUpdateIsInfo { out.append(daemonUpdateText) }
        for b in header?.banners ?? [] where Self.isInfo(b) { out.append(b.text) }
        return out
    }

    /// The top bar's status line: "masond updating · 2 CI jobs being fixed". Empty when there
    /// is nothing quiet to say.
    public var infoSummary: String {
        var parts: [String] = []
        if !daemonUpdateText.isEmpty && daemonUpdateIsInfo { parts.append("masond updating") }
        var jobs = 0
        var other = 0
        for b in header?.banners ?? [] where Self.isInfo(b) {
            if let r = b.text.range(of: #"(\d+) red CI jobs? being fixed"#, options: .regularExpression) {
                jobs += Int(b.text[r].prefix { $0.isNumber }) ?? 0
            } else {
                other += 1
            }
        }
        if jobs > 0 { parts.append("\(jobs) CI job\(jobs == 1 ? "" : "s") being fixed") }
        if other > 0 { parts.append(other == 1 ? "1 notice" : "\(other) notices") }
        return parts.joined(separator: " · ")
    }

    public var monitorUpdateText: String? {
        if let doing = updating { return doing }
        guard updateAvailable else { return updateError }
        return updateError ?? "A newer monitor is available (\(String(latestMonitor.prefix(8))))."
    }

    /// Every notification now showing or dismissed, in display order.
    public func allNotificationRows() -> [NotificationRow] {
        var rows: [NotificationRow] = []
        func client(_ id: String, _ text: String, button: String? = nil, icon: String? = nil) {
            rows.append(NotificationRow(key: id, kind: .client, uiID: id, text: text, button: button, icon: icon, daemonKind: nil,
                                        bannerIndex: nil, collapsed: [], issue: nil, clientID: id))
        }
        if !connected {
            client("connection", linkStatus ?? (reconnecting ? "Daemon restarting, reconnecting…" : "Connection lost. Reconnecting…"), icon: "connection_lost")
        }
        if daemonDown { client("daemon-down", "Daemon down: MASON has lost its daemon. Check that masond is running.", icon: "daemon_down") }
        if let why = readOnlyReason { client("read-only", "Read-only: \(why)", icon: "read_only") }
        if !daemonUpdateText.isEmpty && !daemonUpdateIsInfo {
            client("daemon-update", daemonUpdateText,
                   button: canUpdateDaemon ? (downloadURL.isEmpty ? daemonUpdateButton : "Download") : nil)
        }
        if let text = monitorUpdateText {
            client("monitor-update", text,
                   button: canCancelMonitorUpdate ? "Cancel" : (updateAvailable && updating == nil ? "Update monitor" : nil), icon: "update")
        }
        let banners = header?.banners ?? []
        let firstPause = banners.firstIndex { $0.kind == "paused" }
        for (i, b) in banners.enumerated() {
            // Info is progress or already being handled: the top bar's status line, not a row.
            if Self.isInfo(b) { continue }
            let icon: String? = nil
            let key = Self.bannerKey(b)
            switch b.kind {
            case "push_auth":
                rows.append(NotificationRow(key: key, kind: .push, uiID: "push-auth:\(Self.pushAuthProject(b))", text: b.text,
                    button: canAct ? "Choose SSH key…" : nil, icon: icon, daemonKind: b.kind, bannerIndex: i, collapsed: [], issue: nil, clientID: nil))
            case "health":
                rows.append(NotificationRow(key: key, kind: .health, uiID: "health-session:\(b.session)", text: b.text,
                    button: "Open session", icon: icon, daemonKind: b.kind, bannerIndex: i, collapsed: [], issue: nil, clientID: nil))
            case "paused":
                guard i == firstPause else { continue }
                for row in pauseRows() {
                    let k = row.expanded.map { Self.bannerKey(banners[$0]) }
                        ?? Self.sanitizeKey("paused-collapsed:" + row.collapsed.map { Self.bannerKey(banners[$0]) }.joined(separator: ","))
                    let first = row.expanded ?? row.collapsed.first
                    rows.append(NotificationRow(key: k, kind: .paused, uiID: row.expanded == nil ? "paused-collapsed-row" : "paused-row:\(row.expanded!)",
                        text: first.map { banners[$0].text } ?? "", button: nil, icon: nil, daemonKind: nil, bannerIndex: row.expanded,
                        collapsed: row.collapsed, issue: nil, clientID: nil))
                }
            default:
                rows.append(NotificationRow(key: key, kind: .banner, uiID: "banner:\(i)", text: b.text, button: nil, icon: icon, daemonKind: b.kind,
                                            bannerIndex: i, collapsed: [], issue: nil, clientID: nil))
            }
        }
        for p in header?.pendingIssues ?? [] {
            rows.append(NotificationRow(key: "issue:\(p.number)", kind: .pending, uiID: "pending:\(p.project):\(p.number)",
                text: p.title, button: nil, icon: nil, daemonKind: nil, bannerIndex: nil, collapsed: [], issue: Int(p.number), clientID: nil))
        }
        return rows
    }

    private var dismissedKeys: [String] {
        _ = prefsRevision  // observed: a dismissal redraws the bar
        return (prefs.string(NotificationKeys.dismissed) ?? "").split(separator: "\n").map(String.init)
    }

    /// The rows the bar shows: everything not dismissed. Forgets dismissals whose row is gone.
    public func notificationRows() -> [NotificationRow] {
        let all = allNotificationRows()
        pruneDismissed(present: Set(all.map(\.key)))
        let hidden = Set(dismissedKeys)
        return all.filter { !hidden.contains($0.key) }
    }

    /// Dismissed rows that are present now.
    public func hiddenNotificationCount() -> Int {
        let hidden = Set(dismissedKeys)
        return allNotificationRows().filter { hidden.contains($0.key) }.count
    }

    public func dismissNotification(_ key: String) {
        var keys = dismissedKeys
        if !keys.contains(key) { keys.append(key) }
        prefs.setString(keys.joined(separator: "\n"), for: NotificationKeys.dismissed)
        prefsRevision &+= 1
    }

    /// Shows again every dismissed row that is present now.
    public func showHiddenNotifications() {
        let present = Set(allNotificationRows().map(\.key))
        let kept = dismissedKeys.filter { !present.contains($0) }
        prefs.setString(kept.joined(separator: "\n"), for: NotificationKeys.dismissed)
        prefsRevision &+= 1
    }

    func pruneDismissed(present: Set<String>? = nil) {
        // Before the first header the daemon's rows are unknown, so nothing can be said to be gone.
        guard header != nil else { return }
        let live = present ?? Set(allNotificationRows().map(\.key))
        let keys = dismissedKeys
        let kept = keys.filter { live.contains($0) }
        guard kept.count != keys.count else { return }
        prefs.setString(kept.joined(separator: "\n"), for: NotificationKeys.dismissed)
    }

    /// The chosen height of the scrolling bar (nil: the default, three rows).
    public var notificationHeight: Double? { _ = prefsRevision; return prefs.double(NotificationKeys.height) }

    public func setNotificationHeight(_ h: Double) {
        prefs.setDouble(h, for: NotificationKeys.height)
        prefsRevision &+= 1
    }

    /// At least one row, at most every row, and at most the window's share.
    public static func clampNotificationHeight(_ h: Double, oneRow: Double, total: Double, window: Double) -> Double {
        let upper = max(oneRow, min(total, window * notificationWindowShare))
        return min(max(h, oneRow), upper)
    }
}

enum NotificationKeys {
    static let dismissed = "notifications.dismissed.keys"
    static let height = "notifications.height"
}
