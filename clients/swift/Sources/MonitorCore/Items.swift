// SPDX-License-Identifier: GPL-3.0-or-later

import Foundation
import MasonProtocol

/// One thing on screen, as the conformance runner compares it: an id, the
/// texts a person can read in it, the actions it offers, its place among its
/// siblings. (The same shape `conformance` builds from the server's views.)
public struct Item: Equatable, Sendable {
    public var id: String
    public var texts: [String]
    public var actions: [String]
    public var order: Int

    public init(id: String, texts: [String], actions: [String] = [], order: Int = 0) {
        self.id = id
        self.texts = texts
        self.actions = actions.sorted()
        self.order = order
    }

    public var json: [String: Any] {
        ["id": id, "texts": texts, "actions": actions, "order": order]
    }
}

public typealias TaskRow = Mason_V1_TaskRow

extension TaskRow {
    /// The row as a client shows it: key, title, project, state, why waiting.
    func item(prefix: String, order: Int) -> Item {
        var texts = [key, title, project, state]
        if !whyWaiting.isEmpty { texts.append(whyWaiting) }
        // The task page says what it waits for and what waits for it.
        if prefix == "detail" {
            if !blockers.isEmpty { texts.append("waiting for " + blockers.map { "#\($0)" }.joined(separator: ", ")) }
            if !blocks.isEmpty { texts.append("blocks " + blocks.map { "#\($0)" }.joined(separator: ", ")) }
        }
        return Item(id: "\(prefix):\(id)", texts: texts, actions: allowedActions, order: order)
    }
}

extension Mason_V1_DecisionCard {
    func item(task: UInt64, owner: Bool = true) -> Item {
        var texts = [ask, why]
        for (i, o) in options.enumerated() {
            texts.append("\(i + 1). \(o.label)\(o.recommended ? " (recommended)" : "")")
        }
        texts.append(contentsOf: evidence)
        var actions: [String] = []
        if owner {
            actions = (1...max(options.count, 1)).prefix(options.count).map { "option:\($0)" }
            actions.append("other")
        }
        return Item(id: "card:\(task)", texts: texts, actions: actions)
    }
}

extension Mason_V1_Approval {
    /// What an approval shows: what the task waits on in full; Approve and Reject for the owner only.
    func item(task: UInt64, owner: Bool = true) -> Item {
        var texts = [waitingFor]
        texts.append(contentsOf: lines)
        if !diffHash.isEmpty { texts.append("diff \(diffHash)") }
        if !branch.isEmpty { texts.append(branch) }
        if !diffCommand.isEmpty { texts.append(diffCommand) }
        return Item(id: "approval:\(task)", texts: texts, actions: owner ? ["approve", "reject"] : [])
    }
}

extension MonitorModel {
    /// Everything this client is showing, as items: what the conformance run
    /// reads. Built from the same state the views draw, so the two cannot
    /// disagree; every view also carries the item's id as its accessibility
    /// identifier, so a harness reading the real accessibility tree finds the
    /// same ids.
    public func visibleItems() -> [Item] {
        var out: [Item] = []
        if let h = header {
            out.append(Item(id: "status", texts: [connected ? h.status : "Not connected"]))
            for (i, c) in h.counts.enumerated() {
                out.append(Item(id: "tile:\(c.state)", texts: [tileCount(c), tileLabel(c)], order: i))
            }
            for (i, b) in h.banners.enumerated() {
                out.append(Item(id: "banner:\(i)", texts: [b.text], order: i))
            }
        }
        if let s = sidebar {
            for (i, sec) in s.sections.enumerated() {
                let open = isExpanded(sec)
                out.append(Item(
                    id: "section:\(sec.state)",
                    texts: [sec.title, String(sec.count), open ? "expanded" : "collapsed"],
                    order: i
                ))
                if open {
                    for (j, r) in filteredRows(sec).enumerated() {
                        out.append(r.item(prefix: "row:\(sec.state)", order: j))
                    }
                }
            }
        }
        if page == .home, let h = home {
            for a in h.attention where a.hasTask && a.hasCard {
                out.append(a.card.item(task: a.task.id, owner: canAct))
            }
            for a in h.attention where a.hasTask && a.hasApproval {
                out.append(a.approval.item(task: a.task.id, owner: canAct))
            }
            for (i, a) in h.active.enumerated() {
                out.append(Item(
                    id: "active:\(a.task)",
                    texts: [a.model, a.project, "#\(a.task)", a.step],
                    order: i
                ))
            }
            out.append(Item(
                id: "sessions",
                texts: ["\(h.sessionsUsed) of \(h.sessionsLimit)"],
                actions: ["increase", "decrease"]
            ))
        }
        if page == .pipeline, let e = editor { out.append(contentsOf: e.items()) }
        if case .task = page, let d = taskDetail, d.hasRow {
            out.append(d.row.item(prefix: "detail", order: 0))
            for (i, s) in d.steps.enumerated() {
                out.append(Item(id: "step:\(s.run)", texts: [s.name, s.outcome], order: i))
            }
            if d.hasCard { out.append(d.card.item(task: d.row.id, owner: canAct)) }
            if d.hasApproval { out.append(d.approval.item(task: d.row.id, owner: canAct)) }
        }
        return out
    }
}

extension Mason_V1_EditorView {
    /// What the pipeline editor shows, as items (the same list the daemon's
    /// `conformance::editor_items` builds from the same view).
    public func items() -> [Item] {
        var out: [Item] = []
        for (i, l) in lanes.enumerated() { out.append(Item(id: "lane:\(l.category)", texts: [l.title], order: i)) }
        for (i, n) in nodes.enumerated() {
            var texts = [n.name, n.kind, n.category]
            if !n.summary.isEmpty { texts.append(n.summary) }
            if n.isStart { texts.append("start") }
            texts.append(contentsOf: n.problems)
            out.append(Item(id: "node:\(n.name)", texts: texts, order: i))
        }
        for (i, x) in edges.enumerated() {
            var texts = [x.from, x.to, x.kind == "on_red" ? "on red" : "next"]
            if x.bound > 0 { texts.append("at most \(x.bound) times") }
            out.append(Item(id: "edge:\(x.from):\(x.kind):\(x.to)", texts: texts, order: i))
        }
        for (i, r) in grid.rows.enumerated() {
            var texts = [r.suite, r.level]
            for (c, on) in zip(grid.columns, r.cells) where on { texts.append(c.replacingOccurrences(of: "_", with: " ")) }
            out.append(Item(id: "grid:\(r.suite)", texts: texts, order: i))
        }
        for (i, p) in problems.enumerated() { out.append(Item(id: "problem:\(i)", texts: [p], order: i)) }
        var actions = ["simulate"]
        if canUndo { actions.append("undo") }
        if canRedo { actions.append("redo") }
        if dirty { actions.append(contentsOf: ["discard", "apply"]) }
        out.append(Item(id: "draft", texts: [dirty ? "changed" : "unchanged"], actions: actions))
        for (i, c) in changes.enumerated() { out.append(Item(id: "change:\(i)", texts: [c], order: i)) }
        return out
    }
}
