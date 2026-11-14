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

    private func host(_ m: MonitorModel) -> NSHostingView<AnyView> {
        let h = NSHostingView(rootView: AnyView(
            SidebarPane(model: m, width: 360).environment(\.theme, Theme(dark: false, scale: 1))))
        h.frame = NSRect(x: 0, y: 0, width: 360, height: 20_000)
        h.layoutSubtreeIfNeeded()
        return h
    }

    private func settle(_ h: NSView) {
        RunLoop.main.run(until: Date().addingTimeInterval(0.05))
        h.layoutSubtreeIfNeeded()
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

    func testAProgressPushEvaluatesOnlyItsRow() {
        let m = fixture()
        let h = host(m)
        settle(h)
        let before = RowView.evaluations
        m.apply(progress: progress(task: 7, done: 5))
        settle(h)
        let used = RowView.evaluations - before
        XCTAssertGreaterThan(used, 0)
        XCTAssertLessThanOrEqual(used, 2)
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
        XCTAssertLessThanOrEqual(used, 2 * rowChanging)
    }
}
