// SPDX-License-Identifier: GPL-3.0-or-later

import XCTest

@testable import MonitorCore

final class AppActivityTests: XCTestCase {
    func testHoldBeginsOnceWithLatencyCriticalAndNoDisplayHold() {
        let activity = AppActivity()
        var calls: [(ProcessInfo.ActivityOptions, String)] = []
        let begin: (ProcessInfo.ActivityOptions, String) -> NSObjectProtocol = { o, r in
            calls.append((o, r))
            return NSObject()
        }
        activity.hold(begin: begin)
        activity.hold(begin: begin)
        XCTAssertEqual(calls.count, 1)
        XCTAssertTrue(calls[0].0.contains(.userInitiatedAllowingIdleSystemSleep))
        XCTAssertTrue(calls[0].0.contains(.latencyCritical))
        XCTAssertFalse(calls[0].0.contains(.idleDisplaySleepDisabled))
        XCTAssertTrue(activity.isHeld)
    }
}
