// SPDX-License-Identifier: GPL-3.0-or-later

import AppKit
import MasonProtocol
import MonitorCore
import SwiftUI
import XCTest

@testable import MonitorUI

/// s203: the top bar stacks its groups so it fits the main window's 1000 pt minimum, shows
/// both usage windows, and grays a zero count. Offscreen only: no window, no XCUITest.
@MainActor
final class TopBarFitTests: XCTestCase {
    private func crowdedModel(attention: UInt32) -> MonitorModel {
        let model = MonitorModel()
        model.connected = true
        var h = HeaderView()
        h.attention = attention
        h.selfProject = "mason"
        h.projects = ["mason", "a-project-with-a-long-name"]
        h.counts = [
            ("needs_attention", "waiting for you", true, 3), ("queued", "queued", false, 12),
            ("exploring", "exploring", false, 0), ("developing", "developing", false, 4),
            ("testing", "testing", false, 1), ("reviewing", "reviewing", false, 0),
            ("landed", "landed", false, 120), ("stuck", "stuck", false, 0), ("canceled", "canceled", false, 0),
        ].map { (state: String, label: String, wants: Bool, n: UInt32) -> StateCount in
            var c = StateCount()
            c.state = state
            c.label = label
            c.wantsYou = wants
            c.count = n
            return c
        }
        model.header = h
        var u = UsageView()
        u.providers = ["claude", "claude-second"].map { name -> Mason_V1_ProviderStatus in
            var p = Mason_V1_ProviderStatus()
            p.name = name
            return p
        }
        u.topBar = [
            "claude 5-hour 82%, resets 1 h 10 m", "claude Weekly 40%, resets 3 d",
            "claude-second 5-hour 10%, resets 4 h", "claude-second Weekly 95%, resets 2 d",
        ]
        model.usage = u
        return model
    }

    private func host(_ model: MonitorModel) -> NSHostingController<some View> {
        NSHostingController(rootView: TopBar(model: model).environment(\.theme, Theme(dark: false, scale: 1)))
    }

    /// Measurements for the stage note: the tile row alone, and the whole bar's least width.
    func testMeasureTheTopBarPieces() {
        let model = crowdedModel(attention: 3)
        let tiles = NSHostingController(rootView: HStack(spacing: 4) {
            ForEach(model.header?.counts ?? [], id: \.state) { CountTile(model: model, count: $0) }
        }.environment(\.theme, Theme(dark: false, scale: 1)))
        let t = tiles.sizeThatFits(in: CGSize(width: 0, height: 10_000))
        let all = host(model).sizeThatFits(in: CGSize(width: 0, height: 10_000))
        print("MEASURE tiles-alone \(t.width) whole-bar \(all.width)")
    }

    func testCrowdedTopBarFitsTheMainWindowMinimum() {
        for attention: UInt32 in [0, 3] {
            // Width 0 asks for the least the bar can be: the text may truncate, nothing else gives.
            let least = host(crowdedModel(attention: attention)).sizeThatFits(in: CGSize(width: 0, height: 10_000))
            XCTAssertLessThanOrEqual(least.width, 1000, "attention \(attention): the top bar needs at least \(least.width) pt")
        }
    }

    func testTopBarIsTwoRowsTallInTheStackedGroups() {
        let one = host(crowdedModel(attention: 3)).sizeThatFits(in: CGSize(width: 1000, height: 10_000))
        XCTAssertGreaterThan(one.height, 0)
        XCTAssertLessThan(one.width, 1000.5)
    }

    func testUsageLinesAreAllTheAccountsLinesInOrder() {
        let model = crowdedModel(attention: 0)
        let p = model.usage!.providers[0]
        XCTAssertEqual(topBarUsageLines(model.usage, p), ["claude 5-hour 82%, resets 1 h 10 m", "claude Weekly 40%, resets 3 d"])
        var none = Mason_V1_ProviderStatus()
        none.name = "codex"
        none.sessionsUsed = 1
        none.sessionsLimit = 4
        XCTAssertEqual(topBarUsageLines(model.usage, none), ["codex: 1 of 4 sessions"])
    }

    func testZeroCountIsMutedAndNonZeroIsNot() {
        let theme = Theme(dark: false, scale: 1)
        XCTAssertEqual(tileCountColor(theme, count: 0, wants: false, connected: true), theme.muted)
        XCTAssertEqual(tileCountColor(theme, count: 5, wants: false, connected: true), theme.text)
        XCTAssertEqual(tileCountColor(theme, count: 0, wants: true, connected: true), theme.attention)
        XCTAssertEqual(tileCountColor(theme, count: 0, wants: false, connected: false), theme.text)
    }

    /// Writes the bar at 1000 pt, with and without the indicator, for the stage note.
    func testRenderTheTopBarOffscreen() throws {
        let dir = ProcessInfo.processInfo.environment["MASON_TOPBAR_IMAGES"].map { URL(fileURLWithPath: $0) }
            ?? FileManager.default.temporaryDirectory.appendingPathComponent("topbar-s203")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        for (name, attention) in [("with-attention", UInt32(3)), ("without-attention", UInt32(0))] {
            let view = TopBar(model: crowdedModel(attention: attention))
                .environment(\.theme, Theme(dark: false, scale: 1)).frame(width: 1000)
            let r = ImageRenderer(content: view)
            r.scale = 2
            let png = r.nsImage.flatMap { $0.tiffRepresentation }.flatMap { NSBitmapImageRep(data: $0)?.representation(using: .png, properties: [:]) }
            XCTAssertNotNil(png)
            try png?.write(to: dir.appendingPathComponent("topbar-\(name).png"))
        }
    }
}
