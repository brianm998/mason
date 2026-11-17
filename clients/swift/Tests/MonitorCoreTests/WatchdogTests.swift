// SPDX-License-Identifier: GPL-3.0-or-later

import XCTest

@testable import MonitorCore

final class WatchdogTests: XCTestCase {
    func testPingsOutsideTheWindowDropOut() {
        var s = LatenessStats(thresholdMs: 100, windowMs: 600_000)
        s.record(lateMs: 900, atMs: 0)
        s.record(lateMs: 150, atMs: 300_000)
        s.record(lateMs: 5, atMs: 700_000)
        XCTAssertEqual(s.window(now: 700_000), LatenessWindow(pings: 2, late: 1, worstMs: 150, totalLateMs: 150))
        XCTAssertEqual(s.sinceLaunch, LatenessWindow(pings: 3, late: 2, worstMs: 900, totalLateMs: 1050))
        XCTAssertEqual(s.window(now: 2_000_000).pings, 0)
    }

    func testStartWithTheReleaseConfigCreatesTheTimer() {
        XCTAssertEqual(WatchdogConfig.release.intervalMs, 250)
        XCTAssertEqual(WatchdogConfig.release.thresholdMs, 100)
        XCTAssertEqual(WatchdogConfig.release.logAboveMs, 1000)
        let w = MainThreadWatchdog()
        XCTAssertFalse(w.isRunning)
        w.start(config: .release)
        XCTAssertTrue(w.isRunning)
        w.stop()
        XCTAssertFalse(w.isRunning)
    }

    func testOnlyAPingOverASecondIsLogged() {
        final class Lines: @unchecked Sendable { var all: [String] = [] }
        let lines = Lines()
        let w = MainThreadWatchdog()
        w.start(config: .release, log: { lines.all.append($0) })
        w.note(lateMs: 500)
        w.note(lateMs: 1500)
        w.stop()
        XCTAssertEqual(lines.all, ["main thread late by 1500 ms"])
    }

    func testTheLineWords() {
        XCTAssertEqual(
            LatenessWindow(pings: 2000, late: 31, worstMs: 640, totalLateMs: 5000).line(windowMinutes: 10),
            "UI thread: worst 640 ms late in the last 10 minutes, 31 late pings")
        XCTAssertEqual(LatenessWindow(late: 1).line(windowMinutes: 10), "UI thread: worst 0 ms late in the last 10 minutes, 1 late ping")
    }
}
