// SPDX-License-Identifier: GPL-3.0-or-later

import AppKit
import MasonProtocol
import MasonTokens
import MonitorCore
import SwiftUI
import XCTest

@testable import MonitorCore
@testable import MonitorUI

/// Task #522: a task that is in the sidebar draws its title and actions before the task view
/// arrives. Rendered offscreen; no timing.
@MainActor
final class PendingTaskPageTests: XCTestCase {
    func testAnOpenedTaskDrawsTheSidebarRowBeforeTheTaskViewArrives() throws {
        let m = MonitorModel()
        m.connected = true
        var r = TaskRow()
        r.id = 7
        r.key = "#7"
        r.title = "Fix the thing"
        r.project = "web"
        r.state = "proposed"
        r.allowedActions = ["cancel"]
        var sec = SidebarSection()
        sec.state = "proposed"
        sec.rows = [r]
        var sb = SidebarView()
        sb.sections = [sec]
        var v = Mason_V1_View()
        v.body = .sidebar(sb)
        XCTAssertNil(m.sidebarRow(7))
        m.apply(v)
        let r7 = try XCTUnwrap(m.sidebarRow(7))
        let d = TaskDetailPage.placeholder(r7)
        XCTAssertTrue(d.hasRow)
        XCTAssertEqual(d.row.title, "Fix the thing")
        XCTAssertEqual(d.row.allowedActions, ["cancel"])
        XCTAssertNil(m.taskDetail, "no task view has arrived")
        let page = TaskDetailPage(model: m, id: 7).environment(\.theme, Theme(dark: false, scale: 1))
        let size = NSHostingController(rootView: page).sizeThatFits(in: CGSize(width: 900, height: 10_000))
        XCTAssertGreaterThan(size.height, 0)
    }
}
