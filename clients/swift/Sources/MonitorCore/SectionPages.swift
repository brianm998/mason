// SPDX-License-Identifier: GPL-3.0-or-later

import Foundation
import MasonProtocol

/// What the section detail pages (`Page.section`) read. They draw the same view-model rows as
/// the sidebar (project-scoped, in the daemon's order: not waiting first, then age).
extension MonitorModel {
    /// The sidebar section for a stage name, `nil` while the daemon has not sent it.
    public func sectionSidebar(_ name: String) -> SidebarSection? {
        sidebar?.sections.first { $0.state == name }
    }

    /// The sidebar sections whose tasks a stage's tile counts: the stage's own section and, for
    /// `proposed`, the tracker's imports (`proposed_imports`), which the daemon lists apart but
    /// counts in the same tile. In the sidebar's order.
    public func tileSections(_ name: String) -> [SidebarSection] {
        let names = name == "proposed" ? ["proposed", "proposed_imports"] : [name]
        return names.compactMap { sectionSidebar($0) }
    }

    /// The task rows behind a stage's tile and on its page, in the sidebar's order.
    public func sectionRows(_ name: String) -> [TaskRow] { tileSections(name).flatMap(\.rows) }

    /// The page's title.
    public static func sectionTitle(_ name: String) -> String {
        switch name {
        case "active_models": return "Active models"
        case "landing": return "Landing queue"
        case "queued": return "Queued"
        case "cpu": return "CPU"
        case "disk": return "Disk"
        case "github": return "GitHub"
        case "compilation": return "Compilation"
        case "exploring": return "Exploring"
        case "developing": return "Developing"
        case "testing": return "Testing"
        case "review": return "Review"
        case "needs_attention": return "Needs you"
        case "proposed": return "Proposed"
        default: return name
        }
    }

    /// "12 of 40 suites" for a task whose running step counts, else "".
    public static func suiteProgress(_ row: TaskRow) -> String {
        guard let c = row.progress.first(where: { $0.state == .part && $0.run != 0 }), c.hasRealTotal else { return "" }
        return "\(c.done) of \(c.total) \(c.unit.isEmpty ? "suites" : c.unit)"
    }

    /// The rows Skip testing applies to: those whose actions include it.
    public func skippableRows(_ rows: [TaskRow]) -> [TaskRow] {
        rows.filter { $0.allowedActions.contains("skip_test") }
    }

    /// Skip testing for every row that allows it: the sidebar's command, once per task.
    /// An empty `step` is sent as it is: the daemon reads it as the task's test step.
    public func skipAllTests(_ rows: [TaskRow]) {
        if let why = skipDisabledReason { skipNotice = why; return }
        let targets = skippableRows(rows)
        guard !targets.isEmpty else {
            skipNotice = "Nothing to skip: no task here allows it"
            return
        }
        for r in targets { skipTest(r.id, step: r.step) }
        skipNotice = "Skipped testing for \(targets.count) task\(targets.count == 1 ? "" : "s")"
    }

    /// Why Skip testing cannot be used, or nil when it can; the button is disabled with this as its help.
    public var skipDisabledReason: String? {
        if let why = readOnlyReason { return "Read-only: \(why)" }
        if !isOwner { return "Only the owner can skip testing" }
        return nil
    }
}
