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

    // MARK: the fill marks the shown task (proposed and stuck only)

    private func tile(state: String, shown: Bool) -> (MonitorModel, StateCount) {
        let model = MonitorModel()
        model.connected = true
        var c = Mason_V1_StateCount()
        c.state = state
        c.count = 1
        c.label = state
        c.wantsYou = true
        var r = Mason_V1_TaskRow()
        r.id = 7
        r.state = state
        var sec = Mason_V1_SidebarSection()
        sec.state = state
        sec.count = 1
        sec.rows = [r]
        var sb = Mason_V1_SidebarView()
        sb.sections = [sec]
        model.sidebar = sb
        if shown { model.go(.task(7)) }
        return (model, c)
    }

    /// The tile's background just inside its right edge, clear of the state icon, rendered offscreen.
    private func corner(_ model: MonitorModel, _ c: StateCount, theme: Theme) -> NSColor? {
        let r = ImageRenderer(content: CountTile(model: model, count: c).environment(\.theme, theme))
        r.scale = 1
        guard let cg = r.cgImage else { return nil }
        return NSBitmapImageRep(cgImage: cg).colorAt(x: cg.width - 4, y: cg.height / 2)?.usingColorSpace(.sRGB)
    }

    private func same(_ a: NSColor?, _ b: Color) -> Bool {
        guard let a, let b = NSColor(b).usingColorSpace(.sRGB) else { return false }
        return abs(a.redComponent - b.redComponent) < 0.02 && abs(a.greenComponent - b.greenComponent) < 0.02
            && abs(a.blueComponent - b.blueComponent) < 0.02
    }

    func testProposedAndStuckFillOnlyWhileShown() {
        let theme = Theme(dark: false, scale: 1)
        for state in ["proposed", "needs_attention"] {
            let (home, c) = tile(state: state, shown: false)
            XCTAssertTrue(same(corner(home, c, theme: theme), theme.surface), "\(state) on Home keeps the surface")
            XCTAssertEqual(tileCountColor(theme, count: 1, wants: true, connected: true), theme.attention)
            let (open, c2) = tile(state: state, shown: true)
            XCTAssertTrue(same(corner(open, c2, theme: theme), theme.attentionTint), "\(state) shown is tinted")
        }
    }

    func testFailedStaysTintedOnHome() {
        let theme = Theme(dark: false, scale: 1)
        let (home, c) = tile(state: "failed", shown: false)
        XCTAssertTrue(same(corner(home, c, theme: theme), theme.attentionTint))
    }
}
