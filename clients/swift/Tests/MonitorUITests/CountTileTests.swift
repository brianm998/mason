// SPDX-License-Identifier: GPL-3.0-or-later

import AppKit
import MasonProtocol
import MonitorCore
import SwiftUI
import XCTest

@testable import MonitorUI

/// Part 17: a tile's width depends only on its label and the text size. The state icon is an
/// overlay, so a count of any width, with or without the icon, leaves the tile where it was.
@MainActor
final class CountTileTests: XCTestCase {
    private func width(state: String, count: UInt32, label: String, scale: Double = 1) -> CGFloat {
        let model = MonitorModel()
        model.connected = true
        var c = Mason_V1_StateCount()
        c.state = state
        c.count = count
        c.label = label
        let host = NSHostingView(rootView: CountTile(model: model, count: c).environment(\.theme, Theme(dark: false, scale: scale)))
        return host.fittingSize.width
    }

    func testWidthIsTheSameAtZeroFortyTwoAndNineNineNine() {
        for state in ["needs_attention", "queued", "developing", "landed", "nonsense"] {
            for scale in [1.0, 1.5] {
                let widths = [0, 42, 999].map { width(state: state, count: UInt32($0), label: "waiting for you", scale: scale) }
                XCTAssertEqual(Set(widths).count, 1, "\(state) at \(scale): \(widths)")
            }
        }
    }

    func testTheIconDoesNotWidenATile() {
        XCTAssertEqual(width(state: "queued", count: 7, label: "queued"), width(state: "nonsense", count: 7, label: "queued"))
    }
}
