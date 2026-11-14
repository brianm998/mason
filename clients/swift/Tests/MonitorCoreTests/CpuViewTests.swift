// SPDX-License-Identifier: GPL-3.0-or-later

import MasonProtocol
import XCTest

@testable import MonitorCore

@MainActor
final class CpuViewTests: XCTestCase {
    private func task(_ id: UInt64, _ permille: UInt32) -> Mason_V1_CpuShare {
        var s = Mason_V1_CpuShare()
        s.task = id
        s.kind = "task"
        s.label = "#\(id)"
        s.title = "Title \(id)"
        s.permille = permille
        return s
    }

    private func fixed(_ kind: String, _ permille: UInt32) -> Mason_V1_CpuShare {
        var s = Mason_V1_CpuShare()
        s.kind = kind
        s.label = kind
        s.permille = permille
        return s
    }

    private func usage(_ shares: [Mason_V1_CpuShare], busy: Int32 = 840, sampled: UInt64 = 100_000) -> Mason_V1_CpuUsage {
        var u = Mason_V1_CpuUsage()
        u.busyPermille = busy
        u.cores = 18
        u.sampledMs = sampled
        u.shares = shares
        return u
    }

    private var five: [Mason_V1_CpuShare] { [task(287, 234), task(5, 200), task(9, 150), fixed("masond", 40), fixed("other", 5)] }

    func testSegmentsAndLegendKeepTheDaemonsOrder() {
        let l = cpuLayout(usage(five), nowMs: 101_000, colors: [287: 0, 5: 1, 9: 2])!
        XCTAssertEqual(l.header, "CPU 84% busy · 18 cores")
        XCTAssertNil(l.hidden)
        XCTAssertEqual(l.rows.map(\.label), ["#287", "#5", "#9", "masond", "other"])
        // "other" is under 1% of the bar: no segment, but it keeps its row.
        XCTAssertEqual(l.segments.map(\.label), ["#287", "#5", "#9", "masond"])
    }

    func testWidthsAreProportionalWithIdleAsTheRemainder() {
        let l = cpuLayout(usage(five), nowMs: 101_000, colors: [:])!
        XCTAssertEqual(l.segments[0].permille, 234)
        XCTAssertEqual(l.idlePermille, 1000 - (234 + 200 + 150 + 40 + 5))
    }

    func testAShareUnderOnePercentHasNoSegmentButKeepsItsLegendRow() {
        let l = cpuLayout(usage([task(1, 500), task(2, 9)]), nowMs: 101_000, colors: [:])!
        XCTAssertEqual(l.segments.map(\.label), ["#1"])
        XCTAssertEqual(l.rows.map(\.label), ["#1", "#2"])
    }

    func testRowsPastEightFoldIntoOneMoreRowWithTheRightTotal() {
        let shares = (1...11).map { task(UInt64($0), UInt32(100 - $0)) }
        let l = cpuLayout(usage(shares), nowMs: 101_000, colors: [:])!
        XCTAssertEqual(l.rows.count, 8)
        XCTAssertEqual(l.more.count, 3)
        XCTAssertEqual(l.morePermille, 91 + 90 + 89)
        XCTAssertEqual(l.moreText, "3 more, 27.0% together")
    }

    func testATaskKeepsItsColourWhenAnotherOvertakesItAndAFreedColourIsReused() {
        var c = CpuColors()
        XCTAssertEqual(c.assign([10, 20]), [10: 0, 20: 1])
        let second = c.assign([20, 10, 5])
        XCTAssertEqual(second[10], 0)
        XCTAssertEqual(second[20], 1)
        XCTAssertEqual(second[5], 2)
        let third = c.assign([20, 5, 30])
        XCTAssertEqual(third[30], 0)
        XCTAssertEqual(third[20], 1)
        XCTAssertEqual(third[5], 2)
    }

    func testFixedBucketsUseTheirOwnColoursAndNames() {
        let l = cpuLayout(usage([fixed("masond", 100), fixed("other", 300), task(3, 200)]), nowMs: 101_000, colors: [3: 4])!
        let by = Dictionary(uniqueKeysWithValues: l.rows.map { ($0.label, $0) })
        XCTAssertEqual(by["masond"]?.color, "masond")
        XCTAssertEqual(by["other"]?.color, "other")
        XCTAssertEqual(by["#3"]?.color, "5")
        XCTAssertEqual(by["masond"]?.name, "MASON daemon")
        XCTAssertEqual(by["other"]?.name, "Everything else")
    }

    func testStaleAndMissingReadingsHideTheSegments() {
        let stale = cpuLayout(usage(five), nowMs: 100_000 + Cpu.staleMs + 1, colors: [:])!
        XCTAssertNotNil(stale.hidden)
        XCTAssertTrue(stale.segments.isEmpty)
        let missing = cpuLayout(usage(five, busy: -1), nowMs: 101_000, colors: [:])!
        XCTAssertNotNil(missing.hidden)
        XCTAssertTrue(missing.segments.isEmpty)
        XCTAssertNil(cpuLayout(nil, nowMs: 101_000, colors: [:]))
    }

    func testTheTaskPageSaysHowMuchOrThatItIsNotRunning() {
        XCTAssertEqual(Format.cpuTaskLine(permille: 234), "CPU: 23.4% of the machine")
        XCTAssertEqual(Format.cpuTaskLine(permille: -1), "CPU: not running")
    }

    func testTheModelKeepsColoursAcrossAReorderedReading() {
        let m = MonitorModel()
        func home(_ shares: [Mason_V1_CpuShare]) -> Mason_V1_View {
            var h = Mason_V1_HomeView()
            h.cpu = usage(shares)
            var v = Mason_V1_View()
            v.body = .home(h)
            return v
        }
        m.apply(home([task(1, 300), task(2, 200)]))
        XCTAssertEqual(m.cpu.colors[1], 0)
        XCTAssertEqual(m.cpu.colors[2], 1)
        m.apply(home([task(2, 400), task(1, 100), task(3, 50)]))
        XCTAssertEqual(m.cpu.colors[1], 0)
        XCTAssertEqual(m.cpu.colors[2], 1)
        XCTAssertEqual(m.cpu.colors[3], 2)
    }
}
