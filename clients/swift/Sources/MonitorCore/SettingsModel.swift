// SPDX-License-Identifier: GPL-3.0-or-later

import Foundation

/// The Settings page's data (s430): a project's proposal policy, schedules and tracker, and
/// the global notification routes, as the daemon reports them. Pure: it parses JSON and builds
/// the request bodies, and it never holds a secret (the daemon sends only references).
public struct SettingsSnapshot: Equatable, Sendable {
    public struct Schedule: Equatable, Sendable {
        public var name: String
        public var kind: String
        public var every: String
        public var title: String
    }
    public struct Tracker: Equatable, Sendable {
        public var intervalS: Int
        public var label: String
        public var importAs: String
        public var requireLabel: Bool
        /// `env:NAME`, `file:/path`, or "set in file, not shown"; empty when there is none.
        public var token: String
    }

    public static let dispositionKeys = ["agent_normal", "agent_blocks", "outside_normal", "outside_blocks"]
    public static let introspectionKeys = ["low_small", "low_large", "high_small", "high_large"]
    public static let dispositions = ["proposed", "queued"]

    public var project = ""
    /// The four disposition keys, by key.
    public var dispositions: [String: String] = [:]
    public var introspection: [String: String] = [:]
    public var perTask = 0
    public var perDay = 0
    public var schedules: [Schedule] = []
    public var tracker: Tracker? = nil

    public init() {}

    public init(json: String) {
        guard let data = json.data(using: .utf8),
              let v = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return }
        project = v["project"] as? String ?? ""
        let p = v["proposals"] as? [String: Any] ?? [:]
        for k in Self.dispositionKeys { dispositions[k] = p[k] as? String ?? "proposed" }
        let i = p["introspection"] as? [String: Any] ?? [:]
        for k in Self.introspectionKeys { introspection[k] = i[k] as? String ?? "proposed" }
        perTask = p["per_task"] as? Int ?? 0
        perDay = p["per_day"] as? Int ?? 0
        schedules = (v["schedules"] as? [[String: Any]] ?? []).map {
            Schedule(name: $0["name"] as? String ?? "?", kind: $0["kind"] as? String ?? "task",
                     every: $0["every"] as? String ?? "", title: $0["title"] as? String ?? "")
        }
        if let t = v["tracker"] as? [String: Any] {
            tracker = Tracker(intervalS: t["interval_s"] as? Int ?? 60, label: t["label"] as? String ?? "",
                              importAs: t["import_as"] as? String ?? "proposed",
                              requireLabel: t["require_label"] as? Bool ?? false,
                              token: t["token"] as? String ?? "")
        }
    }
}

/// `GET /remote`: where runs go (#831). The global mode, and for each project whether it has
/// `[remote_runs]`, where it runs, its effective mode and why (`source`).
public struct RemoteSettings: Equatable, Sendable {
    public struct Project: Equatable, Sendable {
        public var project: String
        public var remote: Bool
        public var runsOn: String
        public var mode: String
        /// `project`, `global` or `no [remote_runs]`.
        public var source: String

        /// The source in words, as the page shows it.
        public var sourceText: String {
            switch source {
            case "project": return "set for this project"
            case "global": return "follows the global mode"
            default: return "always runs here: the project file has no [remote_runs]"
            }
        }
        /// A project with no `[remote_runs]` cannot be changed.
        public var canChange: Bool { remote && source != "no [remote_runs]" }
    }

    public static let modes = ["local", "remote", "favor-local", "favor-remote"]
    public static let explanation = "local: this machine. remote: GitHub. favor-local / favor-remote: prefer one, spill to the other."

    public var mode = "local"
    public var problem: String? = nil
    public var changedBy: String? = nil
    public var changedAt: String? = nil
    public var projects: [Project] = []

    public init() {}

    public init(json: String) {
        guard let data = json.data(using: .utf8),
              let v = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return }
        mode = v["mode"] as? String ?? "local"
        if let p = v["problem"] as? String, !p.isEmpty { problem = p }
        if let c = v["changed_by"] as? String, !c.isEmpty { changedBy = c }
        if let c = v["changed_at"] as? String, !c.isEmpty { changedAt = c }
        else if let n = v["changed_at"] as? NSNumber { changedAt = n.stringValue }
        projects = (v["projects"] as? [[String: Any]] ?? []).map {
            Project(project: $0["project"] as? String ?? "?", remote: $0["remote"] as? Bool ?? false,
                    runsOn: $0["runs_on"] as? String ?? "", mode: $0["mode"] as? String ?? "local",
                    source: $0["source"] as? String ?? "")
        }
    }

    public func project(_ name: String) -> Project? { projects.first { $0.project == name } }

    /// "changed by owner at ..." or nil.
    public var changedText: String? {
        guard let by = changedBy else { return nil }
        return changedAt.map { "Changed by \(by) at \($0)" } ?? "Changed by \(by)"
    }
}

/// `GET /notify/settings`: where each alert goes.
public struct NotifySettings: Equatable, Sendable {
    public struct Route: Equatable, Sendable {
        public var condition: String
        /// Channel and the seconds it waits after the alert.
        public var rules: [Rule]
    }
    public struct Rule: Equatable, Sendable {
        public var channel: String
        public var afterS: Int
    }

    public var routes: [Route] = []
    public var channels: [String] = []
    public var quiet: [String: String] = [:]
    public var reminderAfterS = 0
    public var reminderRepeatS = 0
    public var configured: [String: Bool] = [:]
    public var error: String? = nil

    public init() {}

    public init(json: String) {
        guard let data = json.data(using: .utf8),
              let v = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return }
        error = v["error"] as? String
        routes = (v["routing"] as? [[String: Any]] ?? []).map { r in
            Route(condition: r["condition"] as? String ?? "?",
                  rules: (r["rules"] as? [[String: Any]] ?? []).map {
                      Rule(channel: $0["channel"] as? String ?? "?", afterS: $0["after_s"] as? Int ?? 0)
                  })
        }
        channels = v["channels"] as? [String] ?? []
        quiet = (v["quiet"] as? [String: Any] ?? [:]).compactMapValues { $0 as? String }
        reminderAfterS = v["reminder_after_s"] as? Int ?? 0
        reminderRepeatS = v["reminder_repeat_s"] as? Int ?? 0
        configured = (v["configured"] as? [String: Any] ?? [:]).compactMapValues { $0 as? Bool }
    }

    /// Does the condition send to the channel?
    public func sends(_ condition: String, to channel: String) -> Bool {
        routes.first { $0.condition == condition }?.rules.contains { $0.channel == channel } ?? false
    }
}

/// The request bodies the page sends. Each is the smallest change: one key.
public enum SettingsPayload {
    private static func encode(_ o: [String: Any]) -> String {
        guard let d = try? JSONSerialization.data(withJSONObject: o, options: [.sortedKeys]),
              let s = String(data: d, encoding: .utf8) else { return "{}" }
        return s
    }

    public static func disposition(key: String, value: String) -> String {
        encode(["proposals": [key: value]])
    }
    public static func introspection(key: String, value: String) -> String {
        encode(["proposals": ["introspection": [key: value]]])
    }
    public static func cap(key: String, value: Int) -> String {
        encode(["proposals": [key: value]])
    }
    public static func schedule(name: String, every: String) -> String {
        encode(["schedules": [name: ["every": every]]])
    }
    public static func tracker(key: String, value: Any) -> String {
        encode(["tracker": [key: value]])
    }
    /// Turn a channel on for a condition (`delay` like "0" or "5m"), or off (`nil`).
    public static func route(condition: String, channel: String, delay: String?) -> String {
        let v: Any = delay.map { $0 as Any } ?? NSNull()
        return encode(["routing": [condition: [channel: v]]])
    }
    public static func quiet(channel: String, window: String?) -> String {
        let v: Any = window.map { $0 as Any } ?? NSNull()
        return encode(["quiet": [channel: v]])
    }
    public static func testNotification(channel: String) -> String { encode(["channel": channel]) }
    public static func runSchedule(project: String, name: String) -> String {
        encode(["name": "\(project)/\(name)"])
    }

    /// `POST /remote`: the global mode, or one project's when `project` is given.
    public static func remoteMode(_ mode: String, project: String? = nil) -> String {
        var o: [String: Any] = ["mode": mode]
        if let project { o["project"] = project }
        return encode(o)
    }

    public static func settingsPath(project: String) -> String { "/projects/\(project)/settings" }
}
