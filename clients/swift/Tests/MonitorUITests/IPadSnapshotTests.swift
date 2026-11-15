// SPDX-License-Identifier: GPL-3.0-or-later

import ImageIO
import MasonProtocol
import MonitorCore
import SwiftUI
import XCTest

@testable import MonitorUI

/// s447: the monitor renders offscreen at both iPad landscape sizes, light and dark, and the top
/// bar fits those widths. No window, no XCUITest, no timing.
@MainActor
final class IPadSnapshotTests: XCTestCase {
    private func model() -> MonitorModel {
        let model = MonitorModel()
        model.connected = true
        var h = HeaderView()
        h.attention = 3
        h.selfProject = "mason"
        h.projects = ["mason", "a-project-with-a-long-name"]
        h.counts = [("needs_attention", true, 3), ("queued", false, 12), ("developing", false, 4), ("landed", false, 120)]
            .map { (s: String, w: Bool, n: UInt32) -> StateCount in
                var c = StateCount()
                c.state = s
                c.label = s
                c.wantsYou = w
                c.count = n
                return c
            }
        model.header = h
        return model
    }

    func testRendersAtBothSizesLightAndDark() throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("ipad-s447-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: dir) }
        let names = try IPadSnapshot.write(model(), page: "home", to: dir)
        XCTAssertEqual(names.count, 4)
        XCTAssertEqual(Set(names).count, 4)
        for size in IPadSnapshot.sizes {
            for dark in [false, true] {
                let name = IPadSnapshot.fileName("home", size: size, dark: dark)
                XCTAssertTrue(names.contains(name), name)
                XCTAssertTrue(name.contains(IPadSnapshot.sizeName(size)))
                let data = try Data(contentsOf: dir.appendingPathComponent(name))
                let src = CGImageSourceCreateWithData(data as CFData, nil)
                let img = src.flatMap { CGImageSourceCreateImageAtIndex($0, 0, nil) }
                XCTAssertEqual(img?.width, Int(size.width * 2), name)
                XCTAssertEqual(img?.height, Int(size.height * 2), name)
            }
        }
    }

    func testTopBarFitsEveryIPadWidth() {
        let least = IPadSnapshot.topBarLeastWidth(model())
        for size in IPadSnapshot.sizes {
            XCTAssertLessThanOrEqual(least, size.width, "the top bar needs at least \(least) pt of \(size.width)")
        }
    }
}
