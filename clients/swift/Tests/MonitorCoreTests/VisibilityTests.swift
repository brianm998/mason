// SPDX-License-Identifier: GPL-3.0-or-later

import MasonProtocol
import XCTest

@testable import MonitorCore

/// Task #973: while the window is hidden the model holds the sidebar, header and progress, and the
/// subscription asks for header and sidebar only. No test waits for a clock.
@MainActor
extension MonitorCoreTests {
    private func vRow(_ id: UInt64, state: String, actions: [String] = []) -> TaskRow {
        var r = TaskRow()
        r.id = id
        r.key = "#\(id)"
        r.title = "t"
        r.state = state
        r.allowedActions = actions
        return r
    }

    private func vSidebar(_ rows: [TaskRow]) -> Mason_V1_ServerMessage {
        // One section per distinct state, in order, as the daemon sends them.
        var sb = SidebarView()
        for st in rows.map(\.state).reduce(into: [String]()) { if !$0.contains($1) { $0.append($1) } } {
            var s = SidebarSection()
            s.state = st
            s.rows = rows.filter { $0.state == st }
            sb.sections.append(s)
        }
        var v = Mason_V1_View()
        v.body = .sidebar(sb)
        var u = Mason_V1_ViewUpdate()
        u.view = v
        var m = Mason_V1_ServerMessage()
        m.view = u
        return m
    }

    func testHiddenSidebarIsHeldButStillChimes() {
        let m = MonitorModel()
        m.apply(vSidebar([vRow(1, state: "running")]))
        var chimes = 0
        m.onAttention = { chimes += 1 }
        m.visible = false
        let sub = ChangeCounter { _ = m.sidebar }
        m.apply(vSidebar([vRow(1, state: "running"), vRow(2, state: "needs_attention")]))
        XCTAssertEqual(sub.count, 0)
        XCTAssertEqual(chimes, 1)
        XCTAssertTrue(m.isKnownTask(2), "logic reads the held sidebar")
        m.visible = true
        XCTAssertEqual(sub.count, 1, "the held sidebar is published once")
        XCTAssertEqual(m.sidebar?.sections.flatMap(\.rows).count, 2)
        sub.cancel()
    }

    func testSubscriptionNamesOnlyHeaderAndSidebarWhileHidden() async throws {
        let model = MonitorModel()
        let sockets = Box<[FakeSocket]>([])
        let c = connection(model, info: { DaemonInfo(port: 7423, pid: 1, startedMs: 10) }, sockets: sockets)
        c.start()
        await waitUntil("the socket") { sockets.value.count == 1 }
        sockets.value[0].push(try welcomeJSON())
        await waitUntil("connected") { model.connected }
        func lastViews() -> [Mason_V1_ViewKind]? {
            sockets.value[0].sent.compactMap { text -> [Mason_V1_ViewKind]? in
                guard let m = try? Mason_V1_ClientMessage(jsonString: text), case .subscribe(let s)? = m.body else { return nil }
                return s.views
            }.last
        }
        model.go(.landing)
        await waitUntil("page views") { lastViews()?.contains(.landing) == true }
        model.visible = false
        await waitUntil("hidden views") { lastViews() == [.header, .sidebar] }
        model.visible = true
        await waitUntil("page views again") { lastViews()?.contains(.landing) == true }
        c.stop()
    }

    func testHundredHiddenProgressMessagesPublishOnce() {
        let m = MonitorModel()
        var c = ProgressCell()
        c.step = "develop"
        c.state = .part
        var row = vRow(1, state: "running")
        row.progress = [c]
        m.apply(vSidebar([row]))
        var scheduled = 0
        m.scheduleProgressFlush = { _ in scheduled += 1 }
        m.visible = false
        let sub = ChangeCounter { _ = m.progress.entry(1).cells }
        for i in 1...100 {
            var p = Mason_V1_StepProgress()
            p.task = 1
            var n = c
            n.done = UInt32(i)
            p.progress = n
            m.apply(.with { $0.stepProgress = p })
        }
        XCTAssertEqual(sub.count, 0)
        XCTAssertEqual(scheduled, 0)
        m.visible = true
        XCTAssertEqual(sub.count, 1)
        XCTAssertEqual(m.progressCells(task: 1).first?.done, 100)
        sub.cancel()
    }

    func testPendingCommandSettlesWhileHidden() {
        let m = MonitorModel()
        m.connected = true
        m.isOwner = true
        m.apply(vSidebar([vRow(7, state: "proposed", actions: ["approve"])]))
        m.sender = { _ in }
        m.visible = false
        m.approve(7)
        var r = Mason_V1_CommandResult()
        r.ok = true
        r.id = m.pending.keys.first ?? ""
        m.apply(.with { $0.result = r })
        m.apply(vSidebar([vRow(7, state: "approved")]))
        XCTAssertTrue(m.pending.isEmpty)
    }
}
