// SPDX-License-Identifier: GPL-3.0-or-later

import MasonProtocol
import MonitorCore
import XCTest

@testable import MonitorUI

/// The landing queue's grouping and per-task text: pure, nothing rendered.
final class LandingItemsTests: XCTestCase {
    private func item(_ task: UInt64, batch: UInt64 = 0, position: UInt32 = 0, why: String = "") -> Mason_V1_TaskLanding {
        var i = Mason_V1_TaskLanding()
        i.task = task
        i.key = "#\(task)"
        i.title = "t\(task)"
        if batch > 0 {
            i.status = "in_batch"; i.batch = batch; i.phase = "running suites"
            i.phaseIndex = 2; i.phaseTotal = 4; i.phaseMs = 120_000
        } else {
            i.status = "queued"; i.position = position; i.why = why
        }
        return i
    }

    func testGroupsLandingAboveQueuedInQueueOrder() {
        let g = landingGroups([item(3, position: 2, why: "x"), item(1, batch: 7), item(4, position: 1, why: "x")])
        XCTAssertEqual(g.landing.map(\.task), [1])
        XCTAssertEqual(g.queued.map(\.task), [4, 3])
    }

    func testText() {
        XCTAssertEqual(landingItemText(item(1, batch: 7)), "#1 t1: batch #7, running suites, phase 2 of 4 ▰▰▱▱, 2 min")
        XCTAssertEqual(landingItemText(item(3, position: 1, why: "behind 1 batch")), "#3 t3: queued #1, behind 1 batch")
    }

    private final class Sent { var commands: [Mason_V1_Command] = [] }

    // ---- Land now (pure model: nothing rendered) ----

    private func offered(_ i: Mason_V1_TaskLanding) -> Mason_V1_TaskLanding {
        var i = i
        i.allowedActions = ["land_now"]
        return i
    }

    private func batch(_ id: UInt64, running: Bool, actions: [String]) -> Mason_V1_BatchLine {
        var b = Mason_V1_BatchLine()
        b.id = id
        b.running = running
        b.phase = "verifying"
        b.tasks = [1, 2]
        b.allowedActions = actions
        return b
    }

    @MainActor
    func testLandNowIsOfferedOnlyWhereAllowedAndTheDeviceCanAct() {
        let m = MonitorModel()
        m.connected = true
        m.isOwner = true
        XCTAssertTrue(m.canLandNow(offered(item(1, batch: 7))))
        XCTAssertTrue(m.canLandNow(offered(item(3, position: 1))))
        XCTAssertFalse(m.canLandNow(item(1, batch: 7)), "the daemon did not offer it (promoting or landed)")
        XCTAssertTrue(m.canLandNow(batch: batch(7, running: true, actions: ["land_now"])))
        XCTAssertFalse(m.canLandNow(batch: batch(7, running: true, actions: [])))
        XCTAssertFalse(m.canLandNow(batch: batch(7, running: false, actions: ["land_now"])))
        XCTAssertEqual(landNowBatches([batch(7, running: true, actions: ["land_now"]), batch(8, running: true, actions: [])]).map(\.id), [7])
        m.isOwner = false
        XCTAssertFalse(m.canLandNow(offered(item(1, batch: 7))), "not the owner's device")
        XCTAssertFalse(m.canLandNow(batch: batch(7, running: true, actions: ["land_now"])))
    }

    @MainActor
    func testLandNowAsksFirstThenSendsTheRightCommand() {
        let m = MonitorModel()
        m.connected = true
        m.isOwner = true
        let log = Sent()
        m.sender = { c in log.commands.append(c) }
        m.askLandNow(.task(5))
        XCTAssertTrue(log.commands.isEmpty, "nothing is sent before the confirm")
        XCTAssertEqual(LandNowTarget.task(5).confirmText,
                       "Land #5 now? Its remaining tests run after the merge; a red result raises an alert.")
        m.cancelLandNow()
        m.confirmLandNow()
        XCTAssertTrue(log.commands.isEmpty, "a cancelled ask sends nothing")
        m.askLandNow(.task(5))
        m.confirmLandNow()
        XCTAssertEqual(log.commands.count, 1)
        XCTAssertEqual(log.commands[0].landNow.task, 5)
        XCTAssertEqual(m.landNowLabel(.task(5)), "Landing…")
        m.askLandNow(.batch(9))
        m.confirmLandNow()
        XCTAssertEqual(log.commands.count, 2)
        XCTAssertEqual(log.commands[1].landNowBatch.batch, 9)
        XCTAssertEqual(m.landNowLabel(.batch(9)), "Landing…")
        XCTAssertNil(m.landNowLabel(.batch(5)), "a batch's pending state is not a task's")
    }

    // ---- Land now for all (pure model) ----

    private func trow(_ id: UInt64, _ actions: [String]) -> TaskRow {
        var r = TaskRow()
        r.id = id
        r.allowedActions = actions
        return r
    }

    @MainActor
    private func landingModel(owner: Bool = true) -> (MonitorModel, Sent) {
        let m = MonitorModel()
        m.connected = true
        m.isOwner = owner
        var l = LandingView()
        var b = batch(7, running: true, actions: ["land_now"])
        b.tasks = [1, 2]
        l.batches = [b]
        m.landing = l
        let log = Sent()
        m.sender = { c in log.commands.append(c) }
        return (m, log)
    }

    @MainActor
    func testLandAllNowSendsOneBatchCommandAndOnePerQueuedTask() {
        let (m, log) = landingModel()
        m.landAllNow([trow(1, ["land_now"]), trow(2, ["land_now"]), trow(3, ["land_now"]), trow(4, ["cancel"])])
        XCTAssertEqual(log.commands.filter { $0.landNowBatch.batch != 0 }.map(\.landNowBatch.batch), [7])
        XCTAssertEqual(log.commands.filter { $0.landNow.task != 0 }.map(\.landNow.task), [3])
        XCTAssertEqual(log.commands.count, 2)
        XCTAssertEqual(m.landNotice, "Land now sent for 3 tasks (1 batch)")
    }

    @MainActor
    func testLandAllNowSendsNothingWhenNothingAllowsItOrTheDeviceCannotAct() {
        let (m, log) = landingModel()
        m.landAllNow([trow(4, ["cancel"])])
        XCTAssertTrue(log.commands.isEmpty)
        XCTAssertEqual(m.landNotice, "Nothing to land: no task here allows it")
        m.isOwner = false
        XCTAssertNotNil(m.landDisabledReason)
        m.landAllNow([trow(3, ["land_now"])])
        XCTAssertTrue(log.commands.isEmpty)
        XCTAssertEqual(m.landNotice, m.landDisabledReason)
    }

    @MainActor
    func testLandingRowsOfferLandNowOnlyWhenAllowedAndCanAct() {
        let (m, _) = landingModel()
        let rows = [trow(1, ["land_now"]), trow(4, ["cancel"])]
        XCTAssertEqual(m.landableRows(rows).map(\.id), [1])
        XCTAssertTrue(m.canAct && rows[0].allowedActions.contains("land_now"))
        m.isOwner = false
        XCTAssertFalse(m.canAct && rows[0].allowedActions.contains("land_now"))
    }
}
