// SPDX-License-Identifier: GPL-3.0-or-later

import MasonProtocol
import XCTest

@testable import MonitorUI

/// The CI line under a batch: pure text, nothing rendered on screen.
final class BatchCiTests: XCTestCase {
    private func ci(_ q: UInt32, _ r: UInt32, _ p: UInt32, _ f: UInt32, url: String = "") -> Mason_V1_BatchCi {
        var w = Mason_V1_WorkflowJobs()
        w.workflow = "pr-gate"
        w.queued = q
        w.running = r
        w.passed = p
        w.failed = f
        var c = Mason_V1_BatchCi()
        c.workflows = [w]
        c.url = url
        return c
    }

    func testCountsAndFailed() {
        let c = ci(1, 2, 7, 1, url: "https://github.com/o/r/actions/runs/9")
        XCTAssertEqual(batchCiText(c), "CI: pr-gate: 11 jobs · 1 queued · 2 running · 7 passed · 1 failed")
        XCTAssertTrue(batchCiFailed(c))
        XCTAssertEqual(batchCiURL(c)?.absoluteString, "https://github.com/o/r/actions/runs/9")
    }

    func testNoJobStartedIsQueuedForARunner() {
        XCTAssertEqual(batchCiText(ci(3, 0, 0, 0)), "CI: pr-gate: queued for a runner")
        XCTAssertEqual(batchCiText(Mason_V1_BatchCi()), "CI: queued for a runner")
        XCTAssertFalse(batchCiFailed(ci(3, 0, 0, 0)))
    }

    func testOnlyHttpAddressesOpen() {
        XCTAssertNil(batchCiURL(ci(0, 0, 1, 0, url: "file:///etc/passwd")))
        XCTAssertNil(batchCiURL(ci(0, 0, 1, 0)))
    }
}
