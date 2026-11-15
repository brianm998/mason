// SPDX-License-Identifier: GPL-3.0-or-later

import XCTest
@testable import MonitorCore

final class RootChoiceTests: XCTestCase {
    func testRegularWidthIsPad() {
        XCTAssertEqual(RootChoice.choose(regularWidth: true), .pad)
    }

    func testCompactWidthIsPhone() {
        XCTAssertEqual(RootChoice.choose(regularWidth: false), .phone)
    }

    func testUnknownWidthIsPhone() {
        XCTAssertEqual(RootChoice.choose(regularWidth: nil), .phone)
    }
}
