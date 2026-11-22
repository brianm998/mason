// SPDX-License-Identifier: GPL-3.0-or-later

import AppKit
import MasonProtocol
import MonitorCore
import SwiftUI
import XCTest

@testable import MonitorCore
@testable import MonitorUI

/// Task #926: after a project change the sidebar starts at the top and can still be scrolled to its
/// last entry. Hosted offscreen (a window that is never ordered in); no timing assertions.
@MainActor
final class SidebarScrollAfterProjectChangeTests: XCTestCase {
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
            if id % 7 == 0 {
                r.title = "a long task title that has to wrap onto a second line in a narrow sidebar \(id)"
                r.issue = id
                r.proposal = "proposal \(id)"
                r.blockers = [1, 2]
            }
            sec.rows.append(r)
        }
        return sec
    }

    private func sidebar(project: String, landed: Int, changed: Bool = false) -> SidebarView {
        var sb = SidebarView()
        sb.project = project
        sb.sections = [
            section("developing", "Developing", from: 1, count: 38),
            section("landed", "Landed", from: 1000, count: landed),
            section("canceled", "Canceled", from: 5000, count: 68),
        ]
        if changed { sb.sections[0].rows[0].title = "changed" }
        return sb
    }

    private func viewWith(_ sb: SidebarView) -> Mason_V1_View {
        var v = Mason_V1_View()
        v.sidebar = sb
        return v
    }

    private var hostWindows: [NSWindow] = []

    /// Release the hosted views so no sidebar keeps running after the test (other suites render pixels).
    override func tearDown() {
        for w in hostWindows { w.contentView = nil; w.close() }
        hostWindows = []
        SidebarPane.realisedLinks = []
        super.tearDown()
    }

    private func host(_ m: MonitorModel) -> NSHostingView<AnyView> {
        let h = NSHostingView(rootView: AnyView(
            SidebarPane(model: m, width: 360).environment(\.theme, Theme(dark: false, scale: 1))))
        h.frame = NSRect(x: 0, y: 0, width: 360, height: 600)
        let w = NSWindow(contentRect: h.frame, styleMask: [.titled], backing: .buffered, defer: true)
        w.isReleasedWhenClosed = false
        w.contentView = h
        hostWindows.append(w)
        draw(h)
        return h
    }

    private func draw(_ h: NSView) {
        RunLoop.main.run(until: Date().addingTimeInterval(0.1))
        h.layoutSubtreeIfNeeded()
        if let rep = h.bitmapImageRepForCachingDisplay(in: h.bounds) { h.cacheDisplay(in: h.bounds, to: rep) }
    }

    private func scroller(_ v: NSView) -> NSScrollView? {
        if let s = v as? NSScrollView { return s }
        for c in v.subviews { if let s = scroller(c) { return s } }
        return nil
    }

    private func maxOffset(_ s: NSScrollView) -> CGFloat {
        max(0, (s.documentView?.frame.height ?? 0) - s.contentView.bounds.height)
    }

    /// Scroll down in steps until the end, drawing each time, so lazy rows realise and estimates settle.
    private func scrollThrough(_ h: NSView) {
        guard let s = scroller(h) else { return XCTFail("no scroll view") }
        for _ in 0..<400 {
            let y = min(s.contentView.bounds.origin.y + 400, maxOffset(s))
            s.contentView.scroll(to: NSPoint(x: 0, y: y))
            s.reflectScrolledClipView(s.contentView)
            draw(h)
            if y >= maxOffset(s) - 1 && SidebarPane.realisedLinks.contains("Settings") { break }
        }
    }

    private func model() -> MonitorModel {
        let m = MonitorModel()
        m.connected = true
        m.sidebar = sidebar(project: "", landed: 411)
        for s in m.sidebar?.sections ?? [] { if !m.isExpanded(s) { m.toggle(s) } }
        return m
    }

    func testListReachesItsEndAfterAProjectChange() {
        let m = model()
        let h = host(m)
        scrollThrough(h)
        guard let s = scroller(h) else { return XCTFail("no scroll view") }
        XCTAssertGreaterThan(s.contentView.bounds.origin.y, 0)

        m.project = "other"
        draw(h)
        m.apply(viewWith(sidebar(project: "other", landed: 300)))
        draw(h)
        guard let s2 = scroller(h) else { return XCTFail("no scroll view after the change") }
        XCTAssertLessThanOrEqual(s2.contentView.bounds.origin.y, 1, "at the top after the project change")

        m.apply(viewWith(sidebar(project: "other", landed: 300, changed: true)))
        draw(h)

        SidebarPane.realisedLinks = []
        scrollThrough(h)
        XCTAssertTrue(SidebarPane.realisedLinks.contains("Settings"), "last entry realised")
        guard let doc = s2.documentView else { return XCTFail("no document view") }
        XCTAssertGreaterThanOrEqual(s2.contentView.bounds.origin.y, maxOffset(s2) - 1, "scrolled to the end")
        XCTAssertGreaterThanOrEqual(doc.frame.height, s2.contentView.bounds.height)
        XCTAssertGreaterThan(doc.frame.height, 300 * 20, "document covers the content")
    }

    func testScrollTargetStillWorksAfterAProjectChange() {
        let m = model()
        let h = host(m)
        m.project = "other"
        draw(h)
        m.apply(viewWith(sidebar(project: "other", landed: 300)))
        for sec in m.sidebar?.sections ?? [] { if !m.isExpanded(sec) { m.toggle(sec) } }
        draw(h)
        SectionHeader.realised = []
        m.scrollTarget = "landed"
        for _ in 0..<10 where !SectionHeader.realised.contains("landed") { draw(h) }
        XCTAssertTrue(SectionHeader.realised.contains("landed"))
        XCTAssertNil(m.scrollTarget)
    }
}
