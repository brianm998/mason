// SPDX-License-Identifier: GPL-3.0-or-later

import AppKit
import MasonTokens
import MonitorCore
import SwiftUI
import XCTest

@testable import MonitorUI

/// Task 1020: a long blocker list wraps by whole links in the sidebar row and on the task page.
@MainActor
final class BlockerLineTests: XCTestCase {
    private func size<V: View>(_ view: V, width: CGFloat) -> CGSize {
        NSHostingController(rootView: view.environment(\.theme, Theme(dark: false, scale: 1)))
            .sizeThatFits(in: CGSize(width: width, height: 10_000))
    }

    private func row(blockers n: Int) -> TaskRow {
        var r = TaskRow()
        r.id = 1019
        r.key = "#1019"
        r.title = "A held task"
        r.project = "trd"
        r.state = "proposed"
        r.blockers = (0..<n).map { UInt64(990 + $0) }
        r.whyWaiting = "Proposed, held behind " + r.blockers.map { "#\($0)" }.joined(separator: ", ")
        return r
    }

    private func view(_ r: TaskRow) -> some View {
        let model = MonitorModel()
        return RowView(row: r, selected: false, canAct: false, clock: model.live, store: model.progress,
                       go: { _ in }, approve: { _ in })
    }

    private func links(_ a: AttributedString) -> [String] {
        a.runs.filter { $0.link != nil }.map { String(a[$0.range].characters) }
    }

    func testTwentyNineBlockersDoNotMakeTheRowTall() {
        let floor = CGFloat(MasonTokens.sizeSidebarWidthMin)
        for width in [floor, 300] {
            let many = size(view(row(blockers: 29)), width: width).height
            let one = size(view(row(blockers: 1)), width: width).height
            let small = size(Text("x").font(Theme(dark: false, scale: 1).small()), width: 300).height
            XCTAssertLessThanOrEqual(many, one + 2 * small + 1, "29 blockers: \(many) vs 1 blocker: \(one) at \(width)")
        }
    }

    func testSidebarLineNamesThreeAndCountsTheRest() {
        let a = BlockerLine.attributed(label: "blocked by", ids: row(blockers: 29).blockers, limit: 3,
                                       moreTask: 1019, muted: .gray, link: .blue)
        XCTAssertEqual(links(a), ["#990", "#991", "#992", "26 more"])
        XCTAssertEqual(String(a.characters), "blocked by #990, #991, #992 and 26 more")
    }

    func testTwoBlockersAreBothLinksWithoutMore() {
        let a = BlockerLine.attributed(label: "blocked by", ids: [990, 991], limit: 3,
                                       moreTask: 1019, muted: .gray, link: .blue)
        XCTAssertEqual(links(a), ["#990", "#991"])
        XCTAssertFalse(String(a.characters).contains("more"))
    }

    func testPageLineWrapsOnWholeLinks() {
        let ids = (0..<29).map { UInt64(990 + $0) }
        let line = BlockerLine(label: "waiting for", ids: ids, font: .body, go: { _ in })
        let narrow = size(line, width: 390)
        let wide = size(line, width: 900)
        let one = size(BlockerLine(label: "waiting for", ids: [990], font: .body, go: { _ in }), width: 900)
        XCTAssertGreaterThan(narrow.height, wide.height)
        XCTAssertGreaterThan(narrow.height, one.height * 2, "it wraps onto several lines")
        XCTAssertLessThan(narrow.height, one.height * 12, "and not one character per line")
        XCTAssertEqual(links(BlockerLine.attributed(label: "waiting for", ids: ids, limit: nil, moreTask: 0,
                                                    muted: .gray, link: .blue)).count, 29)
    }
}
