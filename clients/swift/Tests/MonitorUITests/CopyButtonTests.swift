// SPDX-License-Identifier: GPL-3.0-or-later

import AppKit
import SwiftUI
import XCTest

@testable import MonitorUI

/// s329: the task detail's branch and worktree copy buttons are labelled and copy the exact value.
@MainActor
final class CopyButtonTests: XCTestCase {
    func testBothButtonsAreLabelledAndCarryTheValues() {
        let t = CopyTarget.detail(branch: "mason/mason/t1", worktree: "/w/mason/1")
        XCTAssertEqual(t.map(\.label), ["Copy branch name", "Copy worktree"])
        var copied: [String] = []
        t.forEach { copied.append($0.value) }
        XCTAssertEqual(copied, ["mason/mason/t1", "/w/mason/1"])
    }

    func testEmptyValuesHideTheirButton() {
        XCTAssertEqual(CopyTarget.detail(branch: "", worktree: "/w").map(\.label), ["Copy worktree"])
        XCTAssertEqual(CopyTarget.detail(branch: "b", worktree: "").map(\.label), ["Copy branch name"])
        XCTAssertTrue(CopyTarget.detail(branch: "", worktree: "").isEmpty)
    }

    func testCopiedReplacesTheLabel() {
        let t = CopyTarget(label: "Copy worktree", value: "/w")
        XCTAssertEqual(t.shown(copied: false), "Copy worktree")
        XCTAssertEqual(t.shown(copied: true), "Copied")
    }

    func testButtonRenders() {
        let host = NSHostingView(rootView: CopyValueButton(target: CopyTarget(label: "Copy worktree", value: "/w")) { _ in }
            .environment(\.theme, Theme(dark: false, scale: 1)))
        XCTAssertGreaterThan(host.fittingSize.width, 0)
    }
}
