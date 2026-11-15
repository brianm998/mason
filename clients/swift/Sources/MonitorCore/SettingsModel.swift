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

    public static func settingsPath(project: String) -> String { "/projects/\(project)/settings" }
}
