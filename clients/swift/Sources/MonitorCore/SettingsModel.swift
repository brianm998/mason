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

/// `GET /review`: the nightly pipeline review's settings, its last and next run.
public struct ReviewSettings: Equatable, Sendable {
    /// The frequencies offered by name, in days.
    public static let presets: [(name: String, days: Int)] = [("Daily", 1), ("Weekly", 7)]
    public static let explanation = "An agent asks where the pipeline was stuck, slow, over-tested, untested, crashing or needing you, and files what it finds as proposals for you to approve."

    public var enabled = true
    /// Local time of day, "HH:MM".
    public var at = "00:00"
    public var everyDays = 1
    public var model = ""
    public var effort = ""
    public var maxProposals = 10
    /// MASON's own project, where the review is filed; empty when none is registered.
    public var project = ""
    public var problem: String? = nil
    public var runningTask: UInt64? = nil
    public var nextRun: String? = nil
    public var lastTask: UInt64? = nil
    public var lastAt: String? = nil
    public var lastSummary = ""
    /// The proposals the last completed review filed.
    public var filed: [UInt64] = []
    public var lastStatus = ""

    public init() {}

    public init(json: String) {
        guard let data = json.data(using: .utf8),
              let v = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return }
        let s = v["settings"] as? [String: Any] ?? [:]
        enabled = s["enabled"] as? Bool ?? true
        at = s["at"] as? String ?? "00:00"
        everyDays = s["every_days"] as? Int ?? 1
        model = s["model"] as? String ?? ""
        effort = s["effort"] as? String ?? ""
        maxProposals = s["max_proposals"] as? Int ?? 10
        project = v["project"] as? String ?? ""
        if let p = v["problem"] as? String, !p.isEmpty { problem = p }
        runningTask = (v["running_task"] as? NSNumber)?.uint64Value
        nextRun = v["next_run_text"] as? String
        if let l = v["last"] as? [String: Any] {
            lastTask = (l["task"] as? NSNumber)?.uint64Value
            lastAt = l["completed_text"] as? String
            lastSummary = l["summary"] as? String ?? ""
            filed = (l["filed"] as? [NSNumber] ?? []).map { $0.uint64Value }
        }
        lastStatus = v["last_status"] as? String ?? ""
    }

    /// "HH:MM" as minutes after midnight; nil when it is not a time.
    public static func minutes(_ at: String) -> Int? {
        let p = at.split(separator: ":", omittingEmptySubsequences: false)
        guard p.count == 2, let h = Int(p[0]), let m = Int(p[1]), (0..<24).contains(h), (0..<60).contains(m) else { return nil }
        return h * 60 + m
    }

    public static func timeText(minutes: Int) -> String {
        String(format: "%02d:%02d", minutes / 60, minutes % 60)
    }

    /// The time as a date the time picker can show (the day itself means nothing).
    public static func date(at: String, calendar: Calendar = .current) -> Date {
        let m = minutes(at) ?? 0
        return calendar.date(from: DateComponents(year: 2001, month: 1, day: 15, hour: m / 60, minute: m % 60)) ?? Date(timeIntervalSinceReferenceDate: 0)
    }

    /// The "HH:MM" of a date the time picker gave back.
    public static func timeText(_ date: Date, calendar: Calendar = .current) -> String {
        let c = calendar.dateComponents([.hour, .minute], from: date)
        return timeText(minutes: (c.hour ?? 0) * 60 + (c.minute ?? 0))
    }

    public var everyText: String { everyDays == 1 ? "Every day" : "Every \(everyDays) days" }

    /// The task numbers of the last run's proposals, as text a task link is made from.
    public var filedText: String {
        filed.isEmpty ? "No proposals filed" : "Proposals filed: " + filed.map { "#\($0)" }.joined(separator: ", ")
    }

    public var nextText: String {
        if let t = runningTask { return "Running now: #\(t)" }
        if project.isEmpty { return "No project of MASON's own is registered, so no review runs" }
        if !enabled { return "Next run: none (off)" }
        return "Next run: \(nextRun ?? "not known")"
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

    /// `POST /review`: one setting of the pipeline review.
    public static func reviewEnabled(_ on: Bool) -> String { encode(["enabled": on]) }
    public static func reviewAt(_ at: String) -> String { encode(["at": at]) }
    public static func reviewEvery(days: Int) -> String { encode(["every_days": days]) }

    public static func settingsPath(project: String) -> String { "/projects/\(project)/settings" }
}
