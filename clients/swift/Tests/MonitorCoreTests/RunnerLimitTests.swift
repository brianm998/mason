// SPDX-License-Identifier: GPL-3.0-or-later

import MasonProtocol
import XCTest

@testable import MonitorCore

/// #1261: the runner limit's minus and plus send `SetLimit` for `gha` (and `gha-macos`) and stop at
/// 0 and at the account ceiling.
@MainActor
final class RunnerLimitTests: XCTestCase {
    private func model(limit: UInt32, ceiling: UInt32, mac: UInt32 = 4) -> (MonitorModel, () -> [Mason_V1_Command]) {
        let m = MonitorModel()
        var sent: [Mason_V1_Command] = []
        m.sender = { sent.append($0) }
        var h = Mason_V1_HomeView()
        h.runnerPools = [
            .with { $0.name = "gha"; $0.limit = limit; $0.ceiling = ceiling; $0.used = 5; $0.mason = 3; $0.other = 2; $0.waiting = 1 },
            .with { $0.name = "gha-macos"; $0.limit = mac; $0.ceiling = 6 },
        ]
        m.home = h
        return (m, { sent })
    }

    private func limit(_ c: Mason_V1_Command) -> String? {
        if case .setLimit(let l)? = c.body { return "\(l.resource)=\(l.units)" }
        return nil
    }

    func testPlusAndMinusSendTheNewLimitForGha() {
        let (m, sent) = model(limit: 16, ceiling: 20)
        m.stepRunnerLimit("gha", by: 1)
        m.stepRunnerLimit("gha", by: -1)
        XCTAssertEqual(sent().count, 2)
        XCTAssertTrue(limit(sent()[0]) == "gha=17")
        XCTAssertTrue(limit(sent()[1]) == "gha=15")
        XCTAssertEqual(sent()[0].actor, m.identity)
    }

    func testTheLimitStopsAtTheCeilingAndAtZero() {
        let (top, sentTop) = model(limit: 20, ceiling: 20)
        top.stepRunnerLimit("gha", by: 1)
        XCTAssertTrue(sentTop().isEmpty, "plus at the ceiling sends nothing")
        let (zero, sentZero) = model(limit: 0, ceiling: 20)
        zero.stepRunnerLimit("gha", by: -1)
        XCTAssertTrue(sentZero().isEmpty, "minus at 0 sends nothing")
        let (over, sentOver) = model(limit: 25, ceiling: 20)
        over.stepRunnerLimit("gha", by: -1)
        XCTAssertTrue(limit(sentOver()[0]) == "gha=20", "a limit above the ceiling steps down to it")
    }

    func testMacosPoolHasItsOwnCeiling() {
        let (m, sent) = model(limit: 16, ceiling: 20, mac: 6)
        m.stepRunnerLimit("gha-macos", by: 1)
        XCTAssertTrue(sent().isEmpty)
        m.stepRunnerLimit("gha-macos", by: -1)
        XCTAssertTrue(limit(sent()[0]) == "gha-macos=5")
    }

    func testWordsSayWhatTheLimitDoes() {
        var p = Mason_V1_RunnerPool()
        p.name = "gha"; p.limit = 0; p.ceiling = 20
        XCTAssertTrue(RunnerLimit.notes(p).contains { $0.contains("no check goes to GitHub") })
        XCTAssertTrue(RunnerLimit.notes(p).contains { $0.contains("running finish") })
        p.limit = 25
        XCTAssertTrue(RunnerLimit.detail(p).contains("ceiling caps the limit to 20"))
        XCTAssertEqual(RunnerLimit.effective(p), 20)
    }

    func testNoPoolsFromAnOlderDaemonMeansNoControl() {
        let m = MonitorModel()
        m.home = Mason_V1_HomeView()
        XCTAssertTrue(m.runnerPools.isEmpty)
    }
}
