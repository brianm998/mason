// SPDX-License-Identifier: GPL-3.0-or-later

import MasonProtocol
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
}
