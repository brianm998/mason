// SPDX-License-Identifier: GPL-3.0-or-later

import AppKit
import MonitorCore
import SwiftUI
import XCTest

@testable import MonitorUI

/// s540, offscreen: the Health page draws the UI-thread line from the numbers it is given.
@MainActor
final class HealthLineTests: XCTestCase {
    func testLineFromInjectedNumbers() {
        let w = LatenessWindow(pings: 2400, late: 31, worstMs: 640, totalLateMs: 9000)
        XCTAssertEqual(HealthPage.uiThreadLine(w), "UI thread: worst 640 ms late in the last 10 minutes, 31 late pings")
        let page = HealthPage(model: MonitorModel(), lateness: { w })
        let host = NSHostingView(rootView: page.environment(\.theme, Theme(dark: false, scale: 1)).frame(width: 600, height: 400))
        XCTAssertGreaterThan(host.fittingSize.height, 0)
    }
}
