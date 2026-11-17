// SPDX-License-Identifier: GPL-3.0-or-later

import Foundation

/// One thing in the sidebar's scrolling part. Home and search are not here: they are
/// pinned above the scrolling part and never scroll.
public enum SidebarEntry: Identifiable {
    case header(SidebarSection)
    case row(TaskRow)
    case rule
    case pageLink(title: String, page: Page)

    /// The entry's identity: by what it is (a section's state, a task's id), never by position,
    /// so a task that moves to another section keeps its row.
    /// The id of a section's header entry, for `ScrollViewReader.scrollTo`.
    public static func headerId(_ state: String) -> String { "h:\(state)" }

    public var id: String {
        switch self {
        case .header(let s): return Self.headerId(s.state)
        case .row(let r): return "r:\(r.id)"
        case .rule: return "rule"
        case .pageLink(let t, _): return "p:\(t)"
        }
    }
}

/// What `sidebarEntries` depends on: the same key gives the same entries, so a redraw that
/// changed none of these reuses the last build instead of filtering every row again.
struct SidebarEntriesKey: Equatable {
    let revision: UInt64
    let search: SearchState
    let expanded: [Bool]
}

extension MonitorModel {
    /// The accessibility identifiers of the pinned area, in reading order (the search box's
    /// own toggles follow `search` as `search:name`, `search:title`, `search:body`).
    public static let sidebarPinnedIds = ["sidebar:home", "search"]

    /// The scrolling part, in reading order: sections with their rows, then the page links.
    public var sidebarEntries: [SidebarEntry] {
        let sections = sidebar?.sections ?? []
        let key = SidebarEntriesKey(revision: sidebarRevision, search: search, expanded: sections.map { isExpanded($0) })
        if let c = sidebarEntriesCache, c.key == key { return c.entries }
        let entries = buildSidebarEntries()
        sidebarEntriesCache = (key, entries)
        return entries
    }

    private func buildSidebarEntries() -> [SidebarEntry] {
        var out: [SidebarEntry] = []
        for sec in sidebar?.sections ?? [] {
            out.append(.header(sec))
            if isExpanded(sec) { out.append(contentsOf: filteredRows(sec).map { .row($0) }) }
        }
        out.append(.rule)
        out.append(.pageLink(title: "Landing", page: .landing))
        out.append(.pageLink(title: "LLM usage", page: .usage))
        out.append(.pageLink(title: "Pipeline", page: .pipeline))
        out.append(.pageLink(title: "Health", page: .health))
        out.append(.pageLink(title: "Settings", page: .settings))
        return out
    }

    /// Where a Home tile's section is in the scrolling part, or nil when it is not there.
    public func scrollIndex(forState state: String) -> Int? {
        sidebarEntries.firstIndex { if case .header(let s) = $0 { return s.state == state } else { return false } }
    }
}
