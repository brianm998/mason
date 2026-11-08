// SPDX-License-Identifier: GPL-3.0-or-later

import XCTest
@testable import SampleApp

final class SampleAppTests: XCTestCase {
    func testGreetsAStrangerWhenThereIsNoName() {
        XCTAssertEqual(greeting(for: ""), "Hello, stranger")
    }

    func testGreetsByName() {
        XCTAssertEqual(greeting(for: "Ada"), "Hello, Ada")
    }

    func testRunsOnTheLeasedDevice() {
        // MASON_DEVICE is set by MASON to the id of the device this suite was given.
        // `swift test` runs on the Mac, so this only checks the variable arrived; the
        // xcodebuild destination in .mason/mason.toml is what puts the tests on the device.
        if let id = ProcessInfo.processInfo.environment["MASON_DEVICE"] {
            XCTAssertFalse(id.isEmpty)
        }
    }
}
