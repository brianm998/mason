// SPDX-License-Identifier: GPL-3.0-or-later

import XCTest

@testable import MonitorCore

final class FormatPercentTests: XCTestCase {
    func testPercentRoundsDownAndClamps() {
        XCTAssertNil(Format.percent(done: 5, total: 0))
        XCTAssertEqual(Format.percent(done: 0, total: 340), 0)
        XCTAssertEqual(Format.percent(done: 212, total: 340), 62)
        XCTAssertEqual(Format.percent(done: 339, total: 340), 99)
        XCTAssertEqual(Format.percent(done: 400, total: 340), 100)
    }
}
