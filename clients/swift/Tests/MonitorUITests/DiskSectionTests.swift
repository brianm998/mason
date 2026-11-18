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

    /// Counts the checkboxes in the view's accessibility tree (SwiftUI draws its own controls, so
    /// there are no NSButton subviews to find).
    private func checkboxes(in root: NSView) -> Int {
        var n = 0
        func walk(_ e: Any?, _ depth: Int) {
            guard depth < 40, let e else { return }
            if let el = e as? NSAccessibilityElementProtocol {
                if let role = (el as? NSObject)?.accessibilityRole(), role == .checkBox { n += 1 }
            }
            let kids = (e as? NSObject)?.accessibilityChildren() ?? []
            for k in kids { walk(k, depth + 1) }
        }
        walk(root, 0)
        return n
    }

    private func pixels(_ v: some View, size: CGSize) -> [UInt8] {
        let r = ImageRenderer(content: v.frame(width: size.width, height: size.height))
        r.scale = 1
        guard let cg = r.cgImage else { return [] }
        let rep = NSBitmapImageRep(cgImage: cg)
        return Array(UnsafeBufferPointer(start: rep.bitmapData, count: rep.bytesPerRow * rep.pixelsHigh))
    }

    func testTwoVolumesHaveACheckboxEachAndOneVolumeHasNone() {
        XCTAssertEqual(checkboxes(in: host(body(history(volumes: 2)), width: 400)), 2)
        XCTAssertEqual(checkboxes(in: host(body(history(volumes: 1)), width: 400)), 0)
    }

    func testAnUncheckedVolumesLineIsGoneButItsLegendRowStays() {
        let h = history(volumes: 2)
        let shown = diskChart(h, hidden: [])
        let hidden = diskChart(h, hidden: ["b"])
        XCTAssertEqual(hidden.series.map(\.id), ["a"])
        XCTAssertNotEqual(
            pixels(DiskChartView(chart: shown).environment(\.theme, Theme(dark: false, scale: 1)), size: CGSize(width: 300, height: 120)),
            pixels(DiskChartView(chart: hidden).environment(\.theme, Theme(dark: false, scale: 1)), size: CGSize(width: 300, height: 120)))
        // The section still lists both legend rows (one checkbox each, one unchecked).
        let v = host(body(h, hidden: ["b"]), width: 400)
        XCTAssertEqual(checkboxes(in: v), 2)
        XCTAssertGreaterThan(v.fittingSize.height, 0)
    }

    func testAllHiddenSaysNoVolumeSelected() {
        let h = history(volumes: 2)
        let all = host(body(h, hidden: ["a", "b"]), width: 400).fittingSize.height
        let some = host(body(h, hidden: ["b"]), width: 400).fittingSize.height
        XCTAssertNotEqual(all, some, "the chart is replaced by the one-line message")
        XCTAssertEqual(diskChart(h, hidden: ["a", "b"]).series.count, 0)
    }

    // A red 30x20-ish box and a blue one stand for CPU and disk.
    private func row(stacked: Bool) -> some View {
        CpuDiskRow(stacked: stacked, spacing: 10) {
            Color.red.frame(maxWidth: .infinity).frame(height: 40)
            Color.blue.frame(maxWidth: .infinity).frame(height: 40)
        }
    }

    private func isColour(_ px: [UInt8], _ x: Int, _ y: Int, width: Int, red: Bool) -> Bool {
        let i = y * width * 4 + x * 4
        guard i + 3 < px.count else { return false }
        return red ? (px[i] > 200 && px[i + 2] < 60) : (px[i + 2] > 200 && px[i] < 60)
    }

    func testSideBySideAtAWideWidthWithDiskRightOfCpuAtThreeToTwo() {
        let w = 1000
        let px = pixels(row(stacked: false), size: CGSize(width: w, height: 90))
        // Stacked would be 90 tall with blue below red; side by side puts both on the top row.
        XCTAssertTrue(isColour(px, 100, 45, width: w, red: true))
        XCTAssertTrue(isColour(px, 900, 45, width: w, red: false))
        // CPU takes 3/5 of 990: red up to 594, gap, blue after 604.
        XCTAssertTrue(isColour(px, 590, 45, width: w, red: true))
        XCTAssertTrue(isColour(px, 610, 45, width: w, red: false))
        XCTAssertFalse(isColour(px, 100, 80, width: w, red: true))
    }

    func testStackedAtANarrowWidthCpuFirst() {
        let w = 500
        let px = pixels(row(stacked: false), size: CGSize(width: w, height: 90))
        XCTAssertTrue(isColour(px, 400, 20, width: w, red: true), "CPU on top at full width")
        XCTAssertTrue(isColour(px, 400, 70, width: w, red: false), "disk below it")
        // A phone stacks even at a wide width.
        let wide = pixels(row(stacked: true), size: CGSize(width: 1000, height: 90))
        XCTAssertTrue(isColour(wide, 900, 70, width: 1000, red: false))
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
