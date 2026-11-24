// SPDX-License-Identifier: GPL-3.0-or-later

import Foundation

/// Running work on other Macs (`docs/nodes.md`). Two views of the same feature:
/// on a Mac that serves as a node, the owner's switch ("Accept work from other Macs"), which is the
/// working-hours control and so takes one click; on a coordinator, the nodes it is paired with and
/// whether each takes work. Both come from the daemon's own routes (`GET /node/status`,
/// `GET /nodes`), read again every few seconds while Home is on screen.

/// `GET /node/status` of this Mac's own daemon.
public struct NodeSelf: Equatable, Sendable {
    /// `[node] serve = true`: only then is there a switch to show.
    public var serving = false
    public var switchOn = false
    /// Why a node that is on takes no work now ("too hot: die 96 °C"); `nil` when it takes work.
    public var whyNot: String?
    public var running = 0
    public var maxJobs = 0
    public var coordinators: [String] = []

    public init() {}

    public init(json: String) {
        guard let data = json.data(using: .utf8),
              let v = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return }
        serving = v["serving"] as? Bool ?? false
        switchOn = (v["switch"] as? [String: Any])?["on"] as? Bool ?? false
        if let w = v["why_not"] as? String, !w.isEmpty { whyNot = w }
        let jobs = v["jobs"] as? [String: Any]
        running = (jobs?["running"] as? NSNumber)?.intValue ?? 0
        maxJobs = (jobs?["max"] as? NSNumber)?.intValue ?? 0
        coordinators = (v["coordinators"] as? [[String: Any]] ?? []).compactMap { $0["name"] as? String }
    }

    /// The line under the switch: what the node is doing, or why it is not taking work.
    public var status: String {
        if !switchOn {
            return running > 0
                ? "Off. \(running) job\(running == 1 ? "" : "s") still running here will finish."
                : "Off: nothing from other Macs runs here."
        }
        if let w = whyNot { return "On, but not taking work now: \(w)." }
        return running > 0 ? "On. \(running) of \(maxJobs) jobs running." : "On: taking work from other Macs."
    }
}

/// One node of a coordinator, from `GET /nodes`.
public struct NodeRow: Equatable, Sendable, Identifiable {
    public var id: String { nid.isEmpty ? name : nid }
    public var name = ""
    public var nid = ""
    public var paired = false
    public var usable = false
    /// "accepting", "not accepting (the owner turned remote work off)", "lost (6 polls …)", …
    public var state = ""
    public var cpuPct: Int?
    public var dieC: Int?
    public var running = 0
    public var maxJobs = 0

    public init() {}

    public init(json v: [String: Any]) {
        name = v["name"] as? String ?? "?"
        nid = v["nid"] as? String ?? ""
        paired = v["paired"] as? Bool ?? false
        usable = v["usable"] as? Bool ?? false
        state = v["state"] as? String ?? ""
        cpuPct = (v["cpu_pct"] as? NSNumber)?.intValue
        dieC = ((v["thermal"] as? [String: Any])?["die_c"] as? NSNumber).map { Int($0.doubleValue.rounded()) }
        let jobs = v["jobs"] as? [String: Any]
        running = (jobs?["running"] as? NSNumber)?.intValue ?? 0
        maxJobs = (jobs?["max"] as? NSNumber)?.intValue ?? 0
    }

    /// "CPU 23%, 61 °C, 1 of 4 jobs" for a node that answered.
    public var load: String {
        var parts: [String] = []
        if let c = cpuPct { parts.append("CPU \(c)%") }
        if let d = dieC { parts.append("\(d) °C") }
        if maxJobs > 0 { parts.append("\(running) of \(maxJobs) jobs") }
        return parts.joined(separator: ", ")
    }
}

/// `GET /nodes`.
public struct NodesSnapshot: Equatable, Sendable {
    /// `[nodes] enabled`: runs may go to nodes.
    public var enabled = false
    public var prefer = "nodes"
    public var nodes: [NodeRow] = []

    public init() {}

    public init(json: String) {
        guard let data = json.data(using: .utf8),
              let v = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return }
        enabled = v["enabled"] as? Bool ?? false
        prefer = v["prefer"] as? String ?? "nodes"
        nodes = (v["nodes"] as? [[String: Any]] ?? []).map { NodeRow(json: $0) }
    }

    /// Show the section when there is something to say: a node paired or found, or nodes turned on.
    public var worthShowing: Bool { enabled || !nodes.isEmpty }

    /// "2 of 3 nodes taking work".
    public var header: String {
        let paired = nodes.filter { $0.paired }
        let usable = paired.filter { $0.usable }.count
        if paired.isEmpty { return enabled ? "No node is paired." : "No node is paired; nodes are off." }
        return "\(usable) of \(paired.count) node\(paired.count == 1 ? "" : "s") taking work" + (enabled ? "" : " (nodes are off)")
    }
}

extension MonitorModel {
    /// Ask for both views; each answer lands in `nodeSelf` and `nodes`.
    public func loadNodes() {
        raw("GET", "/node/status", id: "node-self")
        raw("GET", "/nodes", id: "nodes")
    }

    /// The owner's switch. The answer reloads both views.
    public func setNodeAccept(_ on: Bool) {
        raw("POST", "/node/accept", json: on ? "{\"on\":true}" : "{\"on\":false}", id: "node-accept")
    }

    /// Whether this coordinator sends runs to nodes (`[nodes] enabled`).
    public func setNodesUse(_ on: Bool) {
        raw("POST", "/nodes/use", json: on ? "{\"on\":true}" : "{\"on\":false}", id: "nodes-use")
    }
}
