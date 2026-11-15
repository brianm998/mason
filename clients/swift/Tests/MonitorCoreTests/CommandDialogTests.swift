// SPDX-License-Identifier: GPL-3.0-or-later

import XCTest

@testable import MonitorCore

final class CommandDialogTests: XCTestCase {
    func testNoCommandMeansNoDialog() {
        XCTAssertNil(Format.commandDialog(label: "#88", command: ""))
    }

    func testDialogIsTitledWithTheDescriptionAndHoldsTheCommand() {
        let d = Format.commandDialog(label: "python3 - <<'EOF' …", command: "python3 - <<'EOF'\nprint(1)\nEOF")
        XCTAssertEqual(d?.title, "python3 - <<'EOF' …")
        XCTAssertEqual(d?.command, "python3 - <<'EOF'\nprint(1)\nEOF")
    }

    @MainActor func testOpenKeepsTheCommandAndCloseClears() {
        let m = MonitorModel()
        XCTAssertNil(m.commandDialog)
        m.openCommand(label: "", command: "")
        XCTAssertNil(m.commandDialog, "a row without a command opens nothing")
        m.openCommand(label: "make", command: "make -j8\nmake test")
        XCTAssertEqual(m.commandDialog?.command, "make -j8\nmake test")
        // The row leaves the queue: the model's dialog is a copy and does not change.
        XCTAssertEqual(m.commandDialog?.title, "make")
        m.closeCommand()
        XCTAssertNil(m.commandDialog)
    }
}
