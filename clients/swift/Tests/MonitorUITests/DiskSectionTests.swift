// SPDX-License-Identifier: GPL-3.0-or-later

import AppKit
import MasonProtocol
import MonitorCore
import SwiftUI
import XCTest

@testable import MonitorUI

/// s591: Home's Disk section beside the CPU section. Rendered offscreen (hosting views that are
/// never put in a window, ImageRenderer); nothing appears on screen.
@MainActor
final class DiskSectionTests: XCTestCase {
    private let gb: UInt64 = 1_000_000_000

    private func history(volumes: Int) -> Mason_V1_DiskHistory {
        var h = Mason_V1_DiskHistory()
        h.spanMs = 3_600_000
        let names = [("a", "sp"), ("b", "Macintosh HD")].prefix(volumes)
        h.volumes = names.map { var v = Mason_V1_DiskVolume(); v.id = $0.0; v.name = $0.1; v.mount = "/" + $0.1; return v }
        h.points = (0..<4).map { i in
            var p = Mason_V1_DiskPoint()
            p.atMs = UInt64(i) * 5000
            p.readings = names.enumerated().map { (k, n) in
                var r = Mason_V1_DiskReading()
                r.volume = n.0; r.freeBytes = (400 - UInt64(i) - UInt64(k) * 100) * gb; r.totalBytes = 1800 * gb
                return r
            }
            return p
        }
        return h
    }

    private func body(_ h: Mason_V1_DiskHistory, hidden: Set<String> = []) -> some View {
        var palette = DiskColors()
        let colors = palette.assign(h.volumes.map(\.id))
        return DiskSectionBody(
            history: h, chart: diskChart(h, hidden: hidden, colors: colors), hidden: hidden, colors: colors
        ).environment(\.theme, Theme(dark: false, scale: 1)).frame(width: 400)
    }

    private func host<V: View>(_ v: V, width: CGFloat) -> NSHostingView<V> {
        let view = NSHostingView(rootView: v)
        view.frame = NSRect(x: 0, y: 0, width: width, height: view.fittingSize.height)
        view.layoutSubtreeIfNeeded()
        view.frame.size.height = max(view.fittingSize.height, 10)
        view.layoutSubtreeIfNeeded()
        return view
    }

    /// The view's pixels as 8-bit sRGB RGBA, `width * 4` bytes a row. The renderer's own image
    /// is redrawn into a context of that exact layout: its native format differs between macOS
    /// versions (row padding, 16-bit or wide-gamut channels), and on GitHub's macOS 26 runner
    /// reading it as packed RGBA8 sampled the wrong pixels.
    private func pixels(_ v: some View, size: CGSize) -> [UInt8] {
        let r = ImageRenderer(content: v.frame(width: size.width, height: size.height))
        r.scale = 1
        guard let cg = r.cgImage else { return [] }
        let w = Int(size.width), h = Int(size.height)
        var px = [UInt8](repeating: 0, count: w * h * 4)
        let drawn = px.withUnsafeMutableBytes { buf -> Bool in
            guard let space = CGColorSpace(name: CGColorSpace.sRGB),
                let ctx = CGContext(
                    data: buf.baseAddress, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w * 4,
                    space: space, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
            else { return false }
            ctx.draw(cg, in: CGRect(x: 0, y: 0, width: w, height: h))
            return true
        }
        return drawn ? px : []
    }

    func testTwoVolumesHaveACheckboxEachAndOneVolumeHasNone() {
        XCTAssertTrue(DiskSectionBody.hasCheckboxes(volumeCount: 2))
        XCTAssertFalse(DiskSectionBody.hasCheckboxes(volumeCount: 1))
        // The legend row of one volume is drawn without the control, so the sections differ in look.
        XCTAssertNotEqual(
            pixels(body(history(volumes: 2)), size: CGSize(width: 400, height: 400)),
            pixels(body(history(volumes: 1)), size: CGSize(width: 400, height: 400)))
    }

    func testAnUncheckedVolumesLineIsGoneButItsLegendRowStays() {
        let h = history(volumes: 2)
        let theme = Theme(dark: false, scale: 1)
        XCTAssertEqual(diskChart(h, hidden: ["b"]).series.map(\.id), ["a"])
        XCTAssertNotEqual(
            pixels(DiskChartView(chart: diskChart(h, hidden: [])).environment(\.theme, theme), size: CGSize(width: 300, height: 120)),
            pixels(DiskChartView(chart: diskChart(h, hidden: ["b"])).environment(\.theme, theme), size: CGSize(width: 300, height: 120)))
        // The legend still lists both volumes: the section is as tall as with both shown, plus the
        // header drops one name, so it is drawn (non-empty) and keeps both rows.
        let v = host(body(h, hidden: ["b"]), width: 400)
        XCTAssertGreaterThan(v.fittingSize.height, host(body(history(volumes: 1)), width: 400).fittingSize.height)
    }

    func testAllHiddenSaysNoVolumeSelected() {
        let h = history(volumes: 2)
        let all = host(body(h, hidden: ["a", "b"]), width: 400).fittingSize.height
        let some = host(body(h, hidden: ["b"]), width: 400).fittingSize.height
        XCTAssertNotEqual(all, some, "the chart is replaced by the one-line message")
        XCTAssertEqual(diskChart(h, hidden: ["a", "b"]).series.count, 0)
    }

    func testSideBySideAtAWideWidthWithDiskRightOfCpuAtThreeToTwo() {
        // CPU takes 3/5 of 990: x 0..594, gap 10, disk from 604 to 1000.
        let f = CpuDiskRow.placement(width: 1000, heights: (40, 40), stacked: false, spacing: 10)
        XCTAssertEqual(f.cpu, CGRect(x: 0, y: 0, width: 594, height: 40))
        XCTAssertEqual(f.disk, CGRect(x: 604, y: 0, width: 396, height: 40))
    }

    func testStackedAtANarrowWidthCpuFirst() {
        let f = CpuDiskRow.placement(width: 500, heights: (40, 40), stacked: false, spacing: 10)
        XCTAssertEqual(f.cpu, CGRect(x: 0, y: 0, width: 500, height: 40), "CPU on top at full width")
        XCTAssertEqual(f.disk, CGRect(x: 0, y: 50, width: 500, height: 40), "disk below it")
        // A phone stacks even at a wide width.
        let wide = CpuDiskRow.placement(width: 1000, heights: (40, 40), stacked: true, spacing: 10)
        XCTAssertEqual(wide.cpu, CGRect(x: 0, y: 0, width: 1000, height: 40))
        XCTAssertEqual(wide.disk, CGRect(x: 0, y: 50, width: 1000, height: 40))
    }

    func testTheCpuSectionStillRendersAtTheNarrowerWidth() {
        var usage = Mason_V1_CpuUsage()
        usage.busyPermille = 500; usage.cores = 18; usage.sampledMs = 1000
        var s = Mason_V1_CpuShare()
        s.kind = "task"; s.task = 287; s.label = "#287"; s.title = String(repeating: "A very long title ", count: 8); s.permille = 234
        usage.shares = [s]
        let layout = cpuLayout(usage, nowMs: 1000, colors: [287: 0])!
        let v = CpuSectionBody(model: MonitorModel(), layout: layout, chart: nil, expanded: .constant(false), tip: .constant(nil))
            .environment(\.theme, Theme(dark: false, scale: 1))
        let narrow = host(v.frame(width: 540), width: 540)
        XCTAssertGreaterThan(narrow.fittingSize.height, 0)
        XCTAssertEqual(narrow.fittingSize.width, 540, accuracy: 0.5)
    }
}
