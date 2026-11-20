// SPDX-License-Identifier: GPL-3.0-or-later

import AppKit
import MasonProtocol
import MonitorCore
import SwiftUI
import XCTest

@testable import MonitorUI

/// Task #518: the sidebar list is lazy and each row's meta line is one layout. Counted by body
/// evaluations and by the chosen variant, never by time; hosted offscreen, never ordered in.
@MainActor
final class LazySidebarTests: XCTestCase {
    private func section(_ state: String, _ title: String, from: UInt64, count: Int) -> SidebarSection {
        var sec = SidebarSection()
        sec.state = state
        sec.title = title
        sec.count = UInt32(count)
        for id in from..<(from + UInt64(count)) {
            var r = TaskRow()
            r.id = id
            r.key = "#\(id)"
            r.title = "task \(id)"
            r.project = "web"
            r.state = state
            sec.rows.append(r)
        }
        return sec
    }

    /// 517 rows: 38 developing, Landed (411) and Canceled (68) expanded.
    private func fixture() -> MonitorModel {
        let m = MonitorModel()
        m.connected = true
        var sb = SidebarView()
        sb.sections = [
            section("developing", "Developing", from: 1, count: 38),
            section("landed", "Landed", from: 1000, count: 411),
            section("canceled", "Canceled", from: 5000, count: 68),
        ]
        m.sidebar = sb
        for s in m.sidebar?.sections ?? [] { if !m.isExpanded(s) { m.toggle(s) } }
        return m
    }

    private var hostWindows: [NSWindow] = []

    private func host(_ m: MonitorModel, height: CGFloat) -> NSHostingView<AnyView> {
        let h = NSHostingView(rootView: AnyView(
            SidebarPane(model: m, width: 360).environment(\.theme, Theme(dark: false, scale: 1))))
        h.frame = NSRect(x: 0, y: 0, width: 360, height: height)
        // A window that is never ordered in: a window-less host evaluates no bodies on the CI runner.
        let w = NSWindow(contentRect: h.frame, styleMask: [.titled], backing: .buffered, defer: true)
        w.isReleasedWhenClosed = false
        w.contentView = h
        hostWindows.append(w)
        h.layoutSubtreeIfNeeded()
        // Force a real draw into a bitmap: SwiftUI evaluates bodies only once drawn, and the window is never ordered in.
        if let rep = h.bitmapImageRepForCachingDisplay(in: h.bounds) { h.cacheDisplay(in: h.bounds, to: rep) }
        return h
    }

    private func settle(_ h: NSView) {
        RunLoop.main.run(until: Date().addingTimeInterval(0.1))
        h.layoutSubtreeIfNeeded()
    }

    func testOnlyRowsNearTheViewportAreEvaluated() {
        let m = fixture()
        RowView.evaluations = 0
        let h = host(m, height: 600)
        settle(h)
        XCTAssertGreaterThan(RowView.evaluations, 0)
        XCTAssertLessThan(RowView.evaluations, 100, "of \(m.sidebarEntries.count) entries")
    }

    func testScrollingToAHeaderFarDownRealisesIt() {
        let m = fixture()
        let h = host(m, height: 600)
        settle(h)
        SectionHeader.realised = []
        m.scrollTarget = "canceled"
        for _ in 0..<10 where !SectionHeader.realised.contains("canceled") { settle(h) }
        func scrollers(_ v: NSView) -> [NSScrollView] {
            (v as? NSScrollView).map { [$0] } ?? [] + v.subviews.flatMap(scrollers)
        }
        let ys = scrollers(h).map { $0.contentView.bounds.origin.y }
        XCTAssertTrue(SectionHeader.realised.contains("canceled"), "scroll offsets \(ys), realised \(SectionHeader.realised)")
        XCTAssertNil(m.scrollTarget)
    }

    func testMetaLineGivesWayModelThenTypicalThenWraps() {
        func pick(_ w: CGFloat) -> MetaFit {
            MetaFit.choose(
                available: w, spacing: 8, duration: 40, typical: 60, cells: 50, model: 80, icon: 16)
        }
        XCTAssertEqual(pick(400), .all)
        XCTAssertEqual(pick(260), .noModel)
        XCTAssertEqual(pick(150), .noTypical)
        XCTAssertEqual(pick(100), .wrapped)
        XCTAssertEqual(
            MetaFit.choose(available: 100, spacing: 8, duration: 40, typical: nil, cells: 20, model: nil, icon: 16),
            .all)
    }
}
