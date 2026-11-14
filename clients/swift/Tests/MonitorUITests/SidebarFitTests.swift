// SPDX-License-Identifier: GPL-3.0-or-later

import AppKit
import MasonProtocol
import MasonTokens
import MonitorCore
import SwiftUI
import XCTest

@testable import MonitorUI

/// s147: at the sidebar's floor width a crowded row and a long section header fit the panel.
/// Pieces give way (model, typical range, then the line wraps; the title truncates) instead of
/// asking for more width than the panel has, which used to clip both edges.
@MainActor
final class SidebarFitTests: XCTestCase {
    private func needed<V: View>(_ view: V, width: CGFloat, scale: Double) -> CGSize {
        let host = NSHostingController(rootView: view.environment(\.theme, Theme(dark: false, scale: scale)))
        return host.sizeThatFits(in: CGSize(width: width, height: 10_000))
    }

    private func crowdedRow() -> TaskRow {
        var r = TaskRow()
        r.id = 1234
        r.key = "#1234"
        r.title = "Make the sidebar fit every row at narrow widths without clipping either edge"
        r.project = "a-project-with-a-long-name"
        r.state = "developing"
        r.activity = "running"
        r.inStateMs = 3_725_000
        r.typical = "usually 15 to 20 min"
        r.display = "deep (Opus medium, extended thinking)"
        r.progress = ["explore", "develop", "test", "review", "land"].enumerated().map { (i: Int, step: String) -> Mason_V1_ProgressCell in
            var c = Mason_V1_ProgressCell()
            c.step = step
            c.state = i < 1 ? .filled : (i == 1 ? .part : .empty)
            if i == 1 {
                c.done = 212
                c.total = 340
                c.unit = "tests"
                c.message = "running the slow integration suite against every fixture"
            }
            return c
        }
        return r
    }

    func testRowAndHeaderFitAtTheFloorWidth() {
        let model = MonitorModel()
        model.connected = true
        var section = SidebarSection()
        section.state = "needs_attention"
        section.title = "Waiting for you, with a section title far too long for the panel"
        section.count = 12345
        let floor = CGFloat(MasonTokens.sizeSidebarWidthMin)
        for scale in [1.0, 1.5] {
            let width = floor * CGFloat(MonitorModel.sidebarFactor(scale: scale))
            let row = needed(RowView(row: crowdedRow(), selected: false, canAct: true, clock: model.live, go: { _ in }, approve: { _ in }), width: width, scale: scale)
            XCTAssertLessThanOrEqual(row.width, width + 0.5, "row at \(scale)x needs \(row.width) of \(width)")
            let header = needed(SectionHeader(model: model, section: section), width: width, scale: scale)
            XCTAssertLessThanOrEqual(header.width, width + 0.5, "header at \(scale)x needs \(header.width) of \(width)")
        }
    }
}
