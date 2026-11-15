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

    /// The task rows of a stage's page, in the sidebar's order.
    public func sectionRows(_ name: String) -> [TaskRow] { sectionSidebar(name)?.rows ?? [] }

    /// The page's title.
    public static func sectionTitle(_ name: String) -> String {
        switch name {
        case "active_models": return "Active models"
        case "landing": return "Landing queue"
        case "queued": return "Queued"
        case "cpu": return "CPU"
        case "compilation": return "Compilation"
        case "exploring": return "Exploring"
        case "developing": return "Developing"
        case "testing": return "Testing"
        case "review": return "Review"
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
    public func skipAllTests(_ rows: [TaskRow]) {
        guard canAct else { return }
        for r in skippableRows(rows) { skipTest(r.id, step: r.step) }
    }
}
