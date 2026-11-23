// SPDX-License-Identifier: GPL-3.0-or-later

import AppKit
import MasonProtocol
import MasonTokens
import MonitorCore
import SwiftUI
import XCTest

@testable import MonitorUI

/// s673: the proposed page lists the sidebar's rows and shows Approve only where the row allows
/// it. Rendered offscreen (ImageRenderer); nothing appears on screen.
@MainActor
final class ProposedPageTests: XCTestCase {
    private func row(_ id: UInt64, actions: [String]) -> TaskRow {
        var r = TaskRow()
        r.id = id; r.key = "#\(id)"; r.title = "task \(id)"; r.project = "web"; r.state = "proposed"
        r.allowedActions = actions
        return r
    }

    func testApproveShowsOnlyForRowsThatAllowItAndOnlyToThoseWhoCanAct() {
        let yes = row(1, actions: ["approve"]), no = row(2, actions: ["cancel"])
        XCTAssertTrue(StageRows.showsApprove(page: "proposed", canAct: true, row: yes))
        XCTAssertFalse(StageRows.showsApprove(page: "proposed", canAct: true, row: no))
        XCTAssertFalse(StageRows.showsApprove(page: "proposed", canAct: false, row: yes))
        XCTAssertFalse(StageRows.showsApprove(page: "queued", canAct: true, row: yes))
    }

    func testTheCappedFailedPageShowsNOfM() {
        let model = MonitorModel()
        var sb = SidebarView()
        var s = SidebarSection()
        s.state = "failed"
        s.rows = [row(1, actions: ["retry"]), row(2, actions: [])]
        s.count = 7
        sb.sections = [s]
        model.sidebar = sb
        XCTAssertEqual(model.sectionCountText("failed"), "2 of 7")
        let view = SectionPage(model: model, name: "failed").environment(\.theme, Theme(dark: false, scale: 1))
            .frame(width: 800, height: 400)
        XCTAssertNotNil(ImageRenderer(content: view).cgImage)
    }

    func testTheProposedPageRendersOffscreenWithItsRows() {
        let model = MonitorModel()
        var sb = SidebarView()
        var s = SidebarSection()
        s.state = "proposed"
        s.rows = [row(1, actions: ["approve"]), row(2, actions: [])]
        sb.sections = [s]
        model.sidebar = sb
        XCTAssertEqual(model.sectionRows("proposed").map(\.id), [1, 2])
        let view = SectionPage(model: model, name: "proposed").environment(\.theme, Theme(dark: false, scale: 1))
            .frame(width: 800, height: 400)
        let r = ImageRenderer(content: view)
        XCTAssertNotNil(r.cgImage)
    }
}
