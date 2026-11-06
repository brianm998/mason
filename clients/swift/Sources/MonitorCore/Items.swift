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
        return Item(id: "\(prefix):\(id)", texts: texts, actions: allowedActions, order: order)
    }
}

extension Mason_V1_DecisionCard {
    func item(task: UInt64) -> Item {
        var texts = [ask, why]
        for (i, o) in options.enumerated() {
            texts.append("\(i + 1). \(o.label)\(o.recommended ? " (recommended)" : "")")
        }
        texts.append(contentsOf: evidence)
        var actions = (1...max(options.count, 1)).prefix(options.count).map { "option:\($0)" }
        actions.append("other")
        return Item(id: "card:\(task)", texts: texts, actions: actions)
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
                out.append(Item(id: "tile:\(c.state)", texts: [tileCount(c), c.state], order: i))
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
                out.append(a.card.item(task: a.task.id))
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
        if case .task = page, let d = taskDetail, d.hasRow {
            out.append(d.row.item(prefix: "detail", order: 0))
            for (i, s) in d.steps.enumerated() {
                out.append(Item(id: "step:\(s.run)", texts: [s.name, s.outcome], order: i))
            }
            if d.hasCard { out.append(d.card.item(task: d.row.id)) }
        }
        return out
    }
}
