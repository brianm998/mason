// SPDX-License-Identifier: GPL-3.0-or-later

import AppKit
import MasonProtocol
import MasonTokens
import MonitorCore
import SwiftUI
import XCTest

@testable import MonitorUI

/// #1261: the runner limit control renders offscreen (a hosting controller sized, nothing shown on
/// screen) and grows by the zero-limit note.
@MainActor
final class RunnerLimitSnapshotTests: XCTestCase {
    private func height(limit: UInt32) -> CGFloat {
        let m = MonitorModel()
        var h = Mason_V1_HomeView()
        h.runnerPools = [.with { $0.name = "gha"; $0.limit = limit; $0.ceiling = 20; $0.used = 4; $0.mason = 3; $0.other = 1 }]
        m.home = h
        let view = RunnerLimitRows(model: m).environment(\.theme, Theme(dark: false, scale: 1))
        return NSHostingController(rootView: view).sizeThatFits(in: CGSize(width: 900, height: 10_000)).height
    }

    func testTheControlRendersAndAZeroLimitAddsItsNote() {
        let normal = height(limit: 16)
        XCTAssertGreaterThan(normal, 0)
        XCTAssertGreaterThan(height(limit: 0), normal)
    }
}
