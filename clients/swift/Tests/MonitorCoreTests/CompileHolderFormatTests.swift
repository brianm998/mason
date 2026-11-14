// SPDX-License-Identifier: GPL-3.0-or-later

import XCTest

@testable import MonitorCore

final class CompileHolderFormatTests: XCTestCase {
    func testShortTitleUnchanged() {
        XCTAssertEqual(Format.shortTitle("Fix the build"), "Fix the build")
    }

    func testLongTitleCutAtWordWithEllipsis() {
        let t = "Known red ledger record a test failure that was already red on develop"
        XCTAssertEqual(Format.shortTitle(t), "Known red ledger record a test failure...")
        XCTAssertLessThanOrEqual(Format.shortTitle(t).count, Format.compileTitleLimit + 3)
    }

    func testLongWordIsCutAtLimit() {
        let t = String(repeating: "x", count: 60)
        XCTAssertEqual(Format.shortTitle(t), String(repeating: "x", count: 40) + "...")
    }

    func testHolderText() {
        XCTAssertEqual(Format.compileHolder(project: "mason", title: "Fix"), "(mason) - Fix -")
        XCTAssertEqual(Format.compileHolder(project: "mason", title: ""), "(mason)")
        XCTAssertEqual(Format.compileHolder(project: "", title: ""), "")
    }
}
