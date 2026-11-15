// SPDX-License-Identifier: GPL-3.0-or-later

import AppKit
import MasonProtocol
import MonitorCore
import SwiftUI
import XCTest

@testable import MonitorUI

/// s432, offscreen: the Active sessions panel takes no room at zero and has a row per session.
@MainActor
final class ActiveSessionsTests: XCTestCase {
    private func height(_ n: Int) -> CGFloat {
        let model = MonitorModel()
        var h = Mason_V1_HeaderView()
        h.activeSessions = (0..<n).map { i in
            var s = Mason_V1_ActiveSession()
            s.id = UInt64(i + 1)
            s.label = i == 0 ? "Pipeline health" : "#\(i) A task"
            s.kind = i == 0 ? "health" : "task"
            return s
        }
        model.header = h
        let host = NSHostingView(rootView: ActiveSessionsPanel(model: model).environment(\.theme, Theme(dark: false, scale: 1)).frame(width: 260))
        return host.fittingSize.height
    }

    func testHiddenAtZeroAndGrowingWithEachSession() {
        XCTAssertEqual(height(0), 0)
        let one = height(1), two = height(2)
        XCTAssertGreaterThan(one, 0)
        XCTAssertGreaterThan(two, one)
    }
}
