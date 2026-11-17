// SPDX-License-Identifier: GPL-3.0-or-later

import MasonProtocol
import XCTest

@testable import MonitorCore

/// Task #522: a click is recorded before the command is written, and settled by the answer and then
/// by a view in which the task changed. No test waits for a clock.
@MainActor
final class PendingCommandTests: XCTestCase {
    private func row(_ id: UInt64, state: String, actions: [String]) -> TaskRow {
        var r = TaskRow()
        r.id = id
        r.key = "#\(id)"
        r.title = "t"
        r.state = state
        r.allowedActions = actions
        return r
    }

    private func sidebar(_ rows: [TaskRow]) -> Mason_V1_ServerMessage {
        var s = SidebarSection()
        s.state = rows.first?.state ?? ""
        s.rows = rows
        var sb = SidebarView()
        sb.sections = [s]
        var v = Mason_V1_View()
        v.body = .sidebar(sb)
        var u = Mason_V1_ViewUpdate()
        u.view = v
        var m = Mason_V1_ServerMessage()
        m.view = u
        return m
    }

    private func result(_ id: String, ok: Bool, error: String = "") -> Mason_V1_ServerMessage {
        var r = Mason_V1_CommandResult()
        r.id = id
        r.ok = ok
        r.error = error
        var m = Mason_V1_ServerMessage()
        m.result = r
        return m
    }

    /// The model, and the commands its fake sender received. The sender asserts that the pending
    /// entry and its label exist before the command leaves.
    private final class Sent { var commands: [Mason_V1_Command] = [] }

    private func fixture() -> (MonitorModel, Sent) {
        let m = MonitorModel()
        m.connected = true
        m.isOwner = true
        m.apply(sidebar([row(7, state: "proposed", actions: ["approve", "reject"])]))
        let sent = Sent()
        m.sender = { c in
            MainActor.assumeIsolated {
                XCTAssertEqual(m.pending[c.id]?.task, 7)
                XCTAssertEqual(m.pendingLabel(task: 7, action: "approve"), "Approving…")
                XCTAssertTrue(m.isBusy(task: 7))
            }
            sent.commands.append(c)
        }
        return (m, sent)
    }

    func testApproveIsPendingBeforeTheSenderIsCalled() {
        let (m, sent) = fixture()
        m.approve(7)
        XCTAssertEqual(sent.commands.count, 1)
        XCTAssertTrue(sent.commands[0].id.hasPrefix("act:approve:7:"))
    }

    func testASecondClickWhilePendingSendsNothing() {
        let (m, sent) = fixture()
        m.approve(7)
        m.approve(7)
        m.reject(7)
        XCTAssertEqual(sent.commands.count, 1)
    }

    func testAFailureClearsTheEntryAndShowsTheErrorOnThatTask() {
        let (m, sent) = fixture()
        m.approve(7)
        m.apply(result(sent.commands[0].id, ok: false, error: "owner only"))
        XCTAssertTrue(m.pending.isEmpty)
        XCTAssertEqual(m.actionError(task: 7), "owner only")
        XCTAssertNil(m.actionError(task: 8))
        XCTAssertFalse(m.isBusy(task: 7))
    }

    func testASuccessChangesTheLabelAndKeepsTheControlsDisabledUntilTheTaskChanges() {
        let (m, sent) = fixture()
        m.approve(7)
        m.apply(result(sent.commands[0].id, ok: true))
        XCTAssertEqual(m.pendingLabel(task: 7, action: "approve"), "Approved, waiting for the pipeline…")
        XCTAssertTrue(m.isBusy(task: 7))
        m.apply(sidebar([row(7, state: "proposed", actions: ["approve", "reject"])]))
        XCTAssertTrue(m.isBusy(task: 7), "the same state and actions: still waiting")
        m.apply(sidebar([row(7, state: "queued", actions: ["cancel"])]))
        XCTAssertTrue(m.pending.isEmpty)
        XCTAssertFalse(m.isBusy(task: 7))
    }

    func testAViewThatChangedBeforeTheResultEndsItAtOnce() {
        let (m, sent) = fixture()
        m.approve(7)
        m.apply(sidebar([row(7, state: "queued", actions: ["cancel"])]))
        m.apply(result(sent.commands[0].id, ok: true))
        XCTAssertTrue(m.pending.isEmpty)
    }

    func testAnAcceptedEntryEndsAfterThreeViewsWithoutAChange() {
        let (m, sent) = fixture()
        m.approve(7)
        m.apply(result(sent.commands[0].id, ok: true))
        for _ in 0..<(PendingCommand.viewsToWait - 1) {
            m.apply(sidebar([row(7, state: "proposed", actions: ["approve", "reject"])]))
            XCTAssertTrue(m.isBusy(task: 7))
        }
        m.apply(sidebar([row(7, state: "proposed", actions: ["approve", "reject"])]))
        XCTAssertFalse(m.isBusy(task: 7), "the action is offered again")
    }

    func testDisconnectMarksNotSentAndWelcomeClears() {
        let (m, _) = fixture()
        m.approve(7)
        m.disconnected()
        XCTAssertEqual(m.pendingLabel(task: 7, action: "approve"), "Not sent: reconnecting")
        var w = Mason_V1_Welcome()
        w.resume = .upToDate
        var msg = Mason_V1_ServerMessage()
        msg.welcome = w
        m.apply(msg)
        XCTAssertTrue(m.pending.isEmpty)
    }

    func testReadOnlyRecordsNothing() {
        let (m, sent) = fixture()
        m.readOnlyReason = "old protocol"
        m.approve(7)
        XCTAssertTrue(sent.commands.isEmpty)
        XCTAssertTrue(m.pending.isEmpty)
    }
}
