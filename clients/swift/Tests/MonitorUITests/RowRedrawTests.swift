// SPDX-License-Identifier: GPL-3.0-or-later

import AppKit
import MasonProtocol
import MonitorCore
import SwiftUI
import XCTest

@testable import MonitorUI

/// Task #288: a row is redrawn only when its own data changes. Counted by body evaluations, never
/// by time; the view is rendered offscreen with ImageRenderer, never in a window.
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

    /// An offscreen ImageRenderer: no window, no display cycle. It renders synchronously, and reading
    /// the image again re-renders only what the model changed.
    private var renderers: [ImageRenderer<AnyView>] = []

    private func host(_ m: MonitorModel) -> ImageRenderer<AnyView> {
        let r = ImageRenderer(content: AnyView(
            SidebarPane(model: m, width: 360).environment(\.theme, Theme(dark: false, scale: 1))
                .frame(width: 360, height: 20_000)))
        r.scale = 1
        renderers.append(r)
        draw(r)
        return r
    }

    private func draw(_ r: ImageRenderer<AnyView>) {
        // Observation-driven updates (TaskProgress, the sidebar) reach the graph on a run-loop
        // pass, so drain the main run loop before and after reading the image (s811).
        drain()
        _ = r.cgImage
        drain()
        _ = r.cgImage
    }

    private func drain() {
        var passes = 0
        while passes < 10, RunLoop.main.run(mode: .default, before: Date()) { passes += 1 }
    }

    private func settle(_ h: ImageRenderer<AnyView>) {
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
        let live = LiveProgressCounter.n
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
        let progressViews = LiveProgressCounter.n - live
        print("s237: \(used) RowView body evaluations for \(rowChanging) row-changing messages; \(before) rows drawn before, \(progressViews) progress views")
        XCTAssertGreaterThan(before, 0, "the rows were drawn")
        XCTAssertGreaterThan(progressViews, 0, "the progress views were drawn")
        XCTAssertLessThanOrEqual(used, 2 * rowChanging)
    }
}
