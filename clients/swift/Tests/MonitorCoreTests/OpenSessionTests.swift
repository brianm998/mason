// SPDX-License-Identifier: GPL-3.0-or-later

import MasonProtocol
import XCTest

@testable import MonitorCore

/// Open session, without a window: a stub launcher stands in for Terminal.
@MainActor
final class OpenSessionTests: XCTestCase {
    private func model(launchWorks: Bool, launched: Box, copied: Box) -> MonitorModel {
        let m = MonitorModel()
        m.cliPath = "/cli/mason"
        m.launcher = { argv in launched.lines.append(argv.joined(separator: " ")); return launchWorks }
        m.copier = { copied.lines.append($0) }
        return m
    }

    final class Box { var lines: [String] = [] }

    private func result(ok: Bool = true, json: String = "", error: String = "") -> Mason_V1_CommandResult {
        var r = Mason_V1_CommandResult()
        r.id = "session:42"; r.ok = ok; r.json = json; r.error = error
        return r
    }

    func testIdleLaunchesMasonTaskOpen() {
        let l = Box(), c = Box()
        let m = model(launchWorks: true, launched: l, copied: c)
        m.handle(result: result(json: #"{"idle":true,"worktree":"/w","provider":"codex"}"#))
        XCTAssertEqual(l.lines, ["/cli/mason task open 42"])
        XCTAssertTrue(c.lines.isEmpty)
        XCTAssertNil(m.sessionNotice)
    }

    func testAFailingLauncherCopiesAndSaysSo() {
        let l = Box(), c = Box()
        let m = model(launchWorks: false, launched: l, copied: c)
        m.handle(result: result(json: #"{"idle":true}"#))
        XCTAssertEqual(c.lines, ["/cli/mason task open 42"])
        XCTAssertTrue(m.sessionNotice?.contains("Copied") == true)
    }

    func testADaemonRefusalShowsItsText() {
        let m = model(launchWorks: true, launched: Box(), copied: Box())
        m.handle(result: result(ok: false, error: "no such task"))
        XCTAssertEqual(m.sessionNotice, "no such task")
    }
}
