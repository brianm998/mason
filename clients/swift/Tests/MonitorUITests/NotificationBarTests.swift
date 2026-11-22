// SPDX-License-Identifier: GPL-3.0-or-later

import AppKit
import MasonProtocol
import MonitorCore
import SwiftUI
import XCTest

@testable import MonitorUI

/// s586: the notification bar shows three rows, then a scroll view with a resize handle; every
/// row has a dismiss X; a button brings dismissed rows back. Offscreen only: no window.
@MainActor
final class NotificationBarTests: XCTestCase {
    private func model(rows: Int, text: (Int) -> String = { "Notification number \($0)" }) -> MonitorModel {
        let m = MonitorModel()
        m.connected = true
        var h = HeaderView()
        h.banners = (0..<rows).map { i in
            var b = Mason_V1_Banner()
            b.kind = "info"
            b.project = "p"
            b.text = text(i)
            return b
        }
        m.header = h
        return m
    }

    /// The frames the bar reported, by id.
    private var probed: [String: CGRect] = [:]

    private func laidOut(_ m: MonitorModel, width: CGFloat = 600) -> NSHostingView<AnyView> {
        probed = [:]
        let view = NSHostingView(rootView: AnyView(
            BannersView(model: m, windowHeight: 2000, onProbe: { [self] in probed = $0 })
                .environment(\.theme, Theme(dark: false, scale: 1))))
        view.frame = CGRect(x: 0, y: 0, width: width, height: 2000)
        // Row heights arrive as preferences, so lay out until they have been applied.
        for _ in 0..<5 {
            view.layoutSubtreeIfNeeded()
            RunLoop.main.run(until: Date().addingTimeInterval(0.02))
        }
        return view
    }

    private func height(_ view: NSHostingView<AnyView>) -> CGFloat { view.fittingSize.height }

    private func identifiers(_ view: NSView) -> Set<String> { Set(probed.keys) }

    func testTenRowsAreAsTallAsThreePlusTheHandle() {
        let three = height(laidOut(model(rows: 3)))
        let ten = height(laidOut(model(rows: 10)))
        XCTAssertGreaterThan(three, 0)
        XCTAssertEqual(ten, three + 10, accuracy: 1, "ten rows show three rows and the 10 pt handle")
    }

    func testThreeRowsHaveNoScrollViewAndNoHandle() {
        let ids = identifiers(laidOut(model(rows: 3)))
        XCTAssertFalse(ids.contains("notifications-resize"))
        XCTAssertFalse(ids.contains("notifications-scroll"))
        let more = identifiers(laidOut(model(rows: 4)))
        XCTAssertTrue(more.contains("notifications-resize"), "ids: \(more.sorted())")
    }

    func testEveryRowHasADismissButton() {
        let m = model(rows: 3)
        let ids = identifiers(laidOut(m))
        for key in m.notificationRows().map(\.key) {
            XCTAssertTrue(ids.contains("dismiss:\(key)"), "no dismiss button for \(key)")
        }
    }

    func testShowHiddenButtonAppearsAndTheTopRowEndsBeforeIt() {
        let m = model(rows: 2, text: { "A long notification, number \($0), that has to wrap beside the button " + String(repeating: "word ", count: 12) })
        XCTAssertFalse(identifiers(laidOut(m)).contains("show-hidden-notifications"))
        m.dismissNotification(m.notificationRows()[1].key)
        let view = laidOut(m)
        _ = view
        guard let button = probed["show-hidden-notifications"], let text = probed["banner:0-text"] else {
            return XCTFail("button or top row text not found: \(probed.keys.sorted())")
        }
        XCTAssertLessThanOrEqual(text.maxX, button.minX + 0.5)
    }

    func testPauseRowHasNoXmarkButItsDismiss() {
        let m = MonitorModel()
        m.connected = true
        var b = Mason_V1_Banner()
        b.kind = "paused"
        b.project = "p"
        b.mode = "all"
        b.text = "p is paused"
        var h = HeaderView()
        h.banners = [b]
        m.header = h
        let ids = identifiers(laidOut(m))
        XCTAssertTrue(ids.contains("collapse-pause:0"))
        XCTAssertEqual(ids.filter { $0.hasPrefix("dismiss:") }.count, 1)
    }

    /// s927: a 1 pt line sits on each boundary between neighbouring rows, none above or below.
    func testDividersSitBetweenRowsOnly() {
        let m = model(rows: 3, text: { $0 == 1 ? "A long notification that has to wrap onto several lines. " + String(repeating: "word ", count: 30) : "Short \($0)" })
        _ = laidOut(m, width: 400)
        let dividers = probed.filter { $0.key.hasPrefix("divider:") }
        XCTAssertEqual(dividers.count, 2, "ids: \(probed.keys.sorted())")
        guard let one = probed["divider:1"], let two = probed["divider:2"],
              let t0 = probed["banner:0-text"], let t1 = probed["banner:1-text"], let t2 = probed["banner:2-text"]
        else { return XCTFail("frames missing: \(probed.keys.sorted())") }
        XCTAssertEqual(one.height, 1, accuracy: 0.01)
        XCTAssertEqual(two.height, 1, accuracy: 0.01)
        XCTAssertGreaterThan(t1.height, t0.height, "the middle row wraps")
        XCTAssertLessThanOrEqual(t0.maxY, one.minY + 0.5)
        XCTAssertLessThanOrEqual(one.maxY, t1.minY + 0.5)
        XCTAssertLessThanOrEqual(t1.maxY, two.minY + 0.5)
        XCTAssertLessThanOrEqual(two.maxY, t2.minY + 0.5)
        XCTAssertGreaterThan(one.minY, t0.minY, "nothing above the first row")
    }
}
