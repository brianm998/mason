// SPDX-License-Identifier: GPL-3.0-or-later

import AppKit
import MasonProtocol
import MonitorCore
import SwiftUI
import XCTest

@testable import MonitorUI

/// Task #288: a row is redrawn only when its own data changes. Counted by body evaluations, never
/// by time; the view is hosted offscreen and never ordered in.
@MainActor
final class RowRedrawTests: XCTestCase {
    static let tasks: UInt64 = 25

    private func fixture() -> MonitorModel {
        let m = MonitorModel()
        m.connected = true
        var sb = SidebarView()
        var sec = SidebarSection()
        sec.state = "developing"
        sec.title = "Developing"
        sec.count = UInt32(Self.tasks)
        for id in 1...Self.tasks {
            var r = TaskRow()
            r.id = id
            r.key = "#\(id)"
            r.title = "task \(id)"
            r.project = "web"
            r.state = "developing"
            var c = Mason_V1_ProgressCell()
            c.step = "develop"
            c.state = .part
            c.run = 1
            c.done = 1
            c.total = 10
            r.progress = [c]
            sec.rows.append(r)
        }
        sb.sections = [sec]
        m.sidebar = sb
        return m
    }

    private func progress(task: UInt64, done: UInt32) -> Mason_V1_StepProgress {
        var p = Mason_V1_StepProgress()
        p.task = task
        var c = Mason_V1_ProgressCell()
        c.step = "develop"
        c.state = .part
        c.run = 1
        c.done = done
        c.total = 10
        p.progress = c
        p.nowMs = 1_000_000 + UInt64(done)
        return p
    }

    private var hostWindows: [NSWindow] = []

    private func host(_ m: MonitorModel) -> NSHostingView<AnyView> {
        let h = NSHostingView(rootView: AnyView(
            SidebarPane(model: m, width: 360).environment(\.theme, Theme(dark: false, scale: 1))))
        h.frame = NSRect(x: 0, y: 0, width: 360, height: 20_000)
        // A window that is never ordered in: a window-less host evaluates no bodies on the CI runner.
        let w = NSWindow(contentRect: h.frame, styleMask: [.titled], backing: .buffered, defer: true)
        w.isReleasedWhenClosed = false
        w.contentView = h
        hostWindows.append(w)
        draw(h)
        return h
    }

    /// A real display and update cycle without an on-screen window: flush pending transactions, lay
    /// out, display, and draw into a bitmap, since SwiftUI evaluates bodies only once drawn. On a
    /// headless runner one pass is not always enough, so the run loop is turned between passes.
    private func draw(_ h: NSView) {
        for _ in 0..<3 {
            CATransaction.flush()
            h.window?.layoutIfNeeded()
            h.layoutSubtreeIfNeeded()
            h.displayIfNeeded()
            if let rep = h.bitmapImageRepForCachingDisplay(in: h.bounds) { h.cacheDisplay(in: h.bounds, to: rep) }
            RunLoop.main.run(until: Date().addingTimeInterval(0.01))
        }
    }

    private func settle(_ h: NSView) {
        RunLoop.main.run(until: Date().addingTimeInterval(0.05))
        draw(h)
    }

    func testSequenceAndLogChangesEvaluateNoRows() {
        let m = fixture()
        let h = host(m)
        settle(h)
        let before = RowView.evaluations
        XCTAssertGreaterThan(before, 0, "the rows were drawn")
        for i in 1...50 {
            m.lastSeq = UInt64(i)
            var p = progress(task: 1, done: 1)
            p.progress = Mason_V1_ProgressCell()
            p.log = ["line \(i)"]
            m.apply(progress: p)
            settle(h)
        }
        XCTAssertEqual(RowView.evaluations, before)
    }

    func testAProgressPushEvaluatesOnlyItsProgressViews() {
        let m = fixture()
        let h = host(m)
        settle(h)
        let rows = RowView.evaluations
        let live = LiveProgressCounter.n
        let pane = SidebarPane.evaluations
        m.apply(progress: progress(task: 7, done: 5))
        settle(h)
        XCTAssertEqual(RowView.evaluations, rows, "no row body runs")
        XCTAssertEqual(SidebarPane.evaluations, pane)
        let used = LiveProgressCounter.n - live
        XCTAssertGreaterThan(used, 0)
        XCTAssertLessThanOrEqual(used, 4, "only task 7's progress views")
    }

    /// Exit test: a 500-row sidebar, then a burst for the 25 running tasks. No sidebar publish, no row,
    /// pane or top-bar body; progress views are evaluated in proportion to the tasks touched.
    func testBurstForRunningTasksRedrawsOnlyTheirProgressViews() {
        let m = fixture()
        var sec = SidebarSection()
        sec.state = "landed"
        sec.title = "Landed"
        for id in 1000..<1475 {
            var r = TaskRow()
            r.id = UInt64(id)
            r.key = "#\(id)"
            r.title = "landed \(id)"
            r.state = "landed"
            sec.rows.append(r)
        }
        m.sidebar?.sections.append(sec)
        let h = host(m)
        settle(h)
        let sub = ChangeCounter { _ = m.sidebar }
        let rows = RowView.evaluations
        let pane = SidebarPane.evaluations
        let bar = TopBar.evaluations
        let live = LiveProgressCounter.n
        for round in 1...5 {
            for t in 1...25 { m.apply(progress: progress(task: UInt64(t), done: UInt32(1 + round))) }
            settle(h)
        }
        sub.cancel()
        XCTAssertEqual(sub.count, 0)
        XCTAssertEqual(RowView.evaluations, rows)
        XCTAssertEqual(SidebarPane.evaluations, pane)
        XCTAssertEqual(TopBar.evaluations, bar)
        let used = LiveProgressCounter.n - live
        XCTAssertGreaterThan(used, 0)
        XCTAssertLessThanOrEqual(used, 5 * 25 * 4, "progress views of touched rows only")
    }

    /// The exit test: a recorded stream of about 2,000 messages for 25 running tasks.
    func testRecordedStreamEvaluatesRowsInProportionToRowChangingMessages() {
        let m = fixture()
        let h = host(m)
        settle(h)
        let before = RowView.evaluations
        var rowChanging = 0
        var done = [UInt32](repeating: 1, count: Int(Self.tasks) + 1)
        for i in 0..<2000 {
            switch i % 4 {
            case 0, 1:
                let t = UInt64(i % Int(Self.tasks)) + 1
                done[Int(t)] += 1
                m.apply(progress: progress(task: t, done: done[Int(t)]))
                rowChanging += 1
            case 2:
                m.lastSeq = UInt64(i)
            default:
                var p = progress(task: 1, done: 1)
                p.progress = Mason_V1_ProgressCell()
                p.log = ["l\(i)"]
                m.apply(progress: p)
            }
            if i % 8 == 0 { settle(h) }
        }
        settle(h)
        let used = RowView.evaluations - before
        print("s237: \(used) RowView body evaluations for \(rowChanging) row-changing messages")
        XCTAssertGreaterThan(used, 0, "the rows were drawn")
        XCTAssertLessThanOrEqual(used, 2 * rowChanging)
    }
}
