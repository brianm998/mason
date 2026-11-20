// SPDX-License-Identifier: GPL-3.0-or-later

import AppKit
import MasonProtocol
import MonitorCore
import SwiftUI
import XCTest

@testable import MonitorUI

/// Task #519: a view redraws only when a property it read changes, and a daemon message that
/// carries what the model already holds changes nothing. Counted by body evaluations, never by
/// time; the views are hosted offscreen and never ordered in.
@MainActor
final class FollowOnlyWhatTheyShowTests: XCTestCase {
    private struct Counts: Equatable {
        var topBar = 0, pane = 0, header = 0, home = 0, rows = 0
    }

    private func counts() -> Counts {
        Counts(topBar: TopBar.evaluations, pane: SidebarPane.evaluations, header: SectionHeader.evaluations,
               home: HomePage.evaluations, rows: RowView.evaluations)
    }

    private func headerView(status: String) -> Mason_V1_ServerMessage {
        var h = HeaderView()
        h.status = status
        h.projects = ["web"]
        return view { $0.header = h }
    }

    private func homeView() -> Mason_V1_ServerMessage {
        var h = HomeView()
        h.status = "all quiet"
        return view { $0.home = h }
    }

    private func usageView() -> Mason_V1_ServerMessage {
        var u = UsageView()
        u.note = "no accounts"
        return view { $0.usage = u }
    }

    private func view(_ set: (inout Mason_V1_View) -> Void) -> Mason_V1_ServerMessage {
        var v = Mason_V1_View()
        set(&v)
        var u = Mason_V1_ViewUpdate()
        u.view = v
        var m = Mason_V1_ServerMessage()
        m.view = u
        return m
    }

    private func announceClaim() -> Mason_V1_ServerMessage {
        var r = Mason_V1_CommandResult()
        r.id = "announce"
        r.ok = true
        var m = Mason_V1_ServerMessage()
        m.result = r
        return m
    }

    private func fixture() -> MonitorModel {
        let m = MonitorModel()
        m.connected = true
        var sb = SidebarView()
        var sec = SidebarSection()
        sec.state = "developing"
        sec.title = "Developing"
        sec.count = 25
        for id in 1...25 as ClosedRange<UInt64> {
            var r = TaskRow()
            r.id = id
            r.key = "#\(id)"
            r.title = "task \(id)"
            r.project = "web"
            r.state = "developing"
            sec.rows.append(r)
        }
        sb.sections = [sec]
        m.sidebar = sb
        m.apply(headerView(status: "busy"))
        m.apply(homeView())
        return m
    }

    private var hostWindows: [NSWindow] = []

    private func host(_ m: MonitorModel) -> NSHostingView<AnyView> {
        let h = NSHostingView(rootView: AnyView(
            VStack(spacing: 0) {
                TopBar(model: m)
                HStack(spacing: 0) {
                    SidebarPane(model: m, width: 360)
                    HomePage(model: m)
                }
            }
            .environment(\.theme, Theme(dark: false, scale: 1))))
        h.frame = NSRect(x: 0, y: 0, width: 1400, height: 3000)
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
        RunLoop.main.run(until: Date().addingTimeInterval(0.05))
        h.layoutSubtreeIfNeeded()
    }

    func testANewUsageViewRedrawsNeitherTheBarNorThePaneNorTheHeadersNorHome() {
        let m = fixture()
        let h = host(m)
        settle(h)
        let before = counts()
        XCTAssertGreaterThan(before.header, 0)
        XCTAssertGreaterThan(before.home, 0)
        m.apply(usageView())
        settle(h)
        XCTAssertEqual(counts(), before)
        XCTAssertEqual(m.usage?.note, "no accounts")
    }

    func testASuccessfulResultWithNothingShownRedrawsNothing() {
        let m = fixture()
        let h = host(m)
        settle(h)
        let before = counts()
        for _ in 1...5 {
            m.apply(announceClaim())
            settle(h)
        }
        XCTAssertEqual(counts(), before)
    }

    func testTheSameDaemonViewTwiceRedrawsNothing() {
        let m = fixture()
        let h = host(m)
        settle(h)
        let before = counts()
        m.apply(headerView(status: "busy"))
        m.apply(homeView())
        m.apply(usageView())
        settle(h)
        let afterFirstUsage = counts()
        XCTAssertEqual(afterFirstUsage.rows, before.rows)
        m.apply(headerView(status: "busy"))
        m.apply(homeView())
        m.apply(usageView())
        settle(h)
        XCTAssertEqual(counts(), afterFirstUsage)
    }

    /// The exit test: announce claims, identical views and one changed header. Only the header's
    /// readers run, and no row.
    func testOnlyAChangedHeaderRedrawsTheBar() {
        let m = fixture()
        let h = host(m)
        settle(h)
        let before = counts()
        for _ in 1...3 {
            m.apply(announceClaim())
            m.apply(headerView(status: "busy"))
            m.apply(homeView())
            settle(h)
        }
        XCTAssertEqual(counts(), before, "nothing changed, nothing is evaluated")
        m.apply(headerView(status: "busy, 2 waiting"))
        settle(h)
        let after = counts()
        XCTAssertGreaterThan(after.topBar, before.topBar, "the changed header redraws the bar")
        XCTAssertEqual(after.rows, before.rows, "no row")
        XCTAssertEqual(after.pane, before.pane, "not the sidebar pane")
        XCTAssertEqual(after.home, before.home, "not the home page")
    }
}
