// SPDX-License-Identifier: GPL-3.0-or-later

import MonitorCore
import XCTest

@testable import MonitorUI

/// A tile's single tap is click count 1 and a double tap is 2, on both platforms, through
/// `tileTapped`. A single click on a failed tile selects it; a double click opens it.
@MainActor
final class TileTapTests: XCTestCase {
    func testSingleAndDoubleReachTileClickedAsTheMacDoes() {
        let a = MonitorModel()
        let b = MonitorModel()
        for clicks in [1, 2] {
            tileTapped(model: a, state: "failed", clicks: clicks)
            b.tileClicked(state: "failed", clickCount: clicks)
            XCTAssertEqual(String(describing: a.page), String(describing: b.page), "clicks \(clicks)")
        }
    }
}
