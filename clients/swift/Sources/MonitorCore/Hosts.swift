// SPDX-License-Identifier: GPL-3.0-or-later

import Foundation

/// Where work runs (#1156): the words for Home's Hosts section and the task page's "Running on"
/// line. Pure, so they are tested without a screen. The data is `HomeView.hosts` and
/// `TaskDetailView.runningOn`; an older daemon sends neither, and then nothing is shown.
public enum Hosts {
    /// An activity longer than this is cut, with an ellipsis, to keep a row on one line.
    public static let activityLimit = 60

    /// "This Mac", the node's name, "GitHub".
    public static func title(_ h: HostView) -> String {
        switch h.kind {
        case "local": return "This Mac"
        case "github": return "GitHub"
        default: return h.name.isEmpty ? h.kind : h.name
        }
    }

    /// The hosts to draw, or nil when the section is hidden: an older daemon sent none, or no host
    /// has a run, there are no nodes and GitHub runs are off (a single-machine install).
    public static func visible(_ home: HomeView?, nodesWorthShowing: Bool = false) -> [HostView]? {
        guard let home, !home.hosts.isEmpty else { return nil }
        let anyRun = home.hosts.contains { !$0.runs.isEmpty }
        let hasNodes = nodesWorthShowing || home.hosts.contains { $0.kind == "node" }
        return (anyRun || hasNodes || home.github.enabled) ? home.hosts : nil
    }

    /// "idle", or "3 runs" / "1 run".
    public static func count(_ h: HostView) -> String {
        h.runs.isEmpty ? "idle" : "\(h.runs.count) \(h.runs.count == 1 ? "run" : "runs")"
    }

    /// One line of a host's header on a phone: "marmot · 2 runs".
    public static func compactTitle(_ h: HostView) -> String { "\(title(h)) · \(count(h))" }

    /// A task's activity cut to one line.
    public static func shorten(_ s: String, limit: Int = activityLimit) -> String {
        let one = s.split(whereSeparator: { $0 == "\n" || $0 == "\r" }).first.map(String.init) ?? ""
        let t = one.trimmingCharacters(in: .whitespaces)
        guard t.count > limit else { return t }
        return String(t.prefix(max(limit - 1, 1))).trimmingCharacters(in: .whitespaces) + "…"
    }

    /// "agent develop", "check cargo-workspace".
    public static func what(_ r: HostRun) -> String {
        [r.kind, r.name].filter { !$0.isEmpty }.joined(separator: " ")
    }

    /// The task's key and title, "mason #12 Fix the thing"; a run no task owns shows its label.
    public static func label(_ r: HostRun) -> String {
        if r.hasTask, r.task.id != 0 {
            return "\(r.task.key) \(r.task.title)".trimmingCharacters(in: .whitespaces)
        }
        return what(r)
    }

    /// True when the row's label opens a task.
    public static func hasLink(_ r: HostRun) -> Bool { r.hasTask && r.task.id != 0 }

    /// The task's pipeline state in words ("developing"), then its activity on one line.
    public static func stateLine(_ r: HostRun) -> String {
        guard hasLink(r) else { return "" }
        let state = r.task.state.replacingOccurrences(of: "_", with: " ")
        return [state, shorten(r.task.activity)].filter { !$0.isEmpty }.joined(separator: ": ")
    }

    /// How long it has run: "4 min"; empty when its start is not known.
    public static func duration(_ r: HostRun, now: UInt64) -> String {
        guard r.sinceMs > 0 else { return "" }
        return Format.duration(ms: now > r.sinceMs ? now - r.sinceMs : 0)
    }

    /// The row's text after the link: "developing: writing tests · agent develop · 4 min".
    public static func runLine(_ r: HostRun, now: UInt64) -> String {
        var parts = [stateLine(r)]
        if hasLink(r) { parts.append(what(r)) }
        parts.append(duration(r, now: now))
        return parts.filter { !$0.isEmpty }.joined(separator: " · ")
    }

    /// The task page's line: "Running on: marmot (agent develop, 4 min), GitHub (check x, 1 min)";
    /// "not running anywhere now" with none. Nil when the daemon is too old to say.
    public static func runningOn(_ runs: [HostRun], now: UInt64, known: Bool = true) -> String? {
        guard known else { return nil }
        if runs.isEmpty { return "Running on: not running anywhere now" }
        return "Running on: " + runs.map { runningOnEntry($0, now: now) }.joined(separator: "; ")
    }

    /// "<host> (<kind> <name>, <duration>)".
    public static func runningOnEntry(_ r: HostRun, now: UInt64) -> String {
        let inner = [what(r), duration(r, now: now)].filter { !$0.isEmpty }.joined(separator: ", ")
        return "\(hostName(r.host)) (\(inner))"
    }

    /// `local`, `node:<name>`, `github:<runs_on>` as "This Mac", the name, "GitHub".
    public static func hostName(_ host: String) -> String {
        if host == "local" { return "This Mac" }
        if host.hasPrefix("node:") { return String(host.dropFirst(5)) }
        if host.hasPrefix("github") { return "GitHub" }
        return host
    }
}
