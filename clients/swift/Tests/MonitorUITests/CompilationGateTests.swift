// SPDX-License-Identifier: GPL-3.0-or-later

import AppKit
import MasonProtocol
import MasonTokens
import MonitorCore
import SwiftUI
import XCTest

@testable import MonitorUI

/// s238: Home's Compilation section has CPU-gate and settle steppers, and shows a refusal
/// beside them. Rendered offscreen (a hosting controller sized, nothing shown on screen).
@MainActor
final class CompilationGateTests: XCTestCase {
    private func height(_ model: MonitorModel, _ c: Mason_V1_Compilation) -> CGFloat {
        let view = CompilationSection(model: model, c: c).environment(\.theme, Theme(dark: false, scale: 1))
        return NSHostingController(rootView: view).sizeThatFits(in: CGSize(width: 900, height: 10_000)).height
    }

    func testTheSectionRendersWithTheGateSteppersAndGrowsByTheRefusal() {
        var c = Mason_V1_Compilation()
        c.limit = 3
        c.cpuMaxPct = 70
        c.settleS = 30
        let model = MonitorModel()
        let plain = height(model, c)
        XCTAssertGreaterThan(plain, 0)
        model.lastResult = "compile_cpu_max must be from 1 to 100"
        XCTAssertGreaterThan(height(model, c), plain, "the refusal is shown inline")
    }

    func testTheStepperBoundsAreTenToAHundred() {
        XCTAssertEqual(MonitorModel.cpuGateRange, 10...100)
    }
}
