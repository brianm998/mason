// SPDX-License-Identifier: GPL-3.0-or-later

import MasonProtocol
import XCTest

@testable import MonitorCore

@MainActor
final class DiskViewTests: XCTestCase {
    private func volume(_ id: String, _ name: String, _ mount: String, _ roles: [String]) -> Mason_V1_DiskVolume {
        var v = Mason_V1_DiskVolume()
        v.id = id; v.name = name; v.mount = mount; v.roles = roles
        return v
    }

    private func reading(_ v: String, _ free: UInt64, _ total: UInt64) -> Mason_V1_DiskReading {
        var r = Mason_V1_DiskReading()
        r.volume = v; r.freeBytes = free; r.totalBytes = total
        return r
    }

    private func point(_ at: UInt64, _ rs: [Mason_V1_DiskReading]) -> Mason_V1_DiskPoint {
        var p = Mason_V1_DiskPoint()
        p.atMs = at; p.readings = rs
        return p
    }

    private var a: Mason_V1_DiskVolume { volume("a", "sp", "/Volumes/sp", ["data", "worktrees"]) }
    private var b: Mason_V1_DiskVolume { volume("b", "Macintosh HD", "/", ["project", "cache"]) }
    private let gb: UInt64 = 1_000_000_000

    private var two: Mason_V1_DiskHistory {
        var h = Mason_V1_DiskHistory()
        h.spanMs = 300_000; h.stepMs = 5000; h.volumes = [a, b]
        h.points = [
            point(1000, [reading("a", 400 * gb, 1800 * gb), reading("b", 100 * gb, 500 * gb)]),
            point(6000, [reading("a", 399 * gb, 1800 * gb)]),
            point(11000, [reading("a", 398 * gb, 1800 * gb), reading("b", 90 * gb, 500 * gb)]),
        ]
        return h
    }

    func testOneVolumeIsOneSeriesScaledToItsTotal() {
        var h = two
        h.volumes = [a]
        h.points = h.points.map { var p = $0; p.readings = p.readings.filter { $0.volume == "a" }; return p }
        let c = diskChart(h, hidden: [])
        XCTAssertEqual(c.times, [1000, 6000, 11000])
        XCTAssertEqual(c.series.map(\.id), ["a"])
        XCTAssertEqual(c.series[0].free, [400 * gb, 399 * gb, 398 * gb])
        XCTAssertEqual(c.maxTotal, 1800 * gb)
    }

    func testSeveralVolumesGetOneSeriesEachAndAGapWhereAbsent() {
        let c = diskChart(two, hidden: [])
        XCTAssertEqual(c.series.map(\.id), ["a", "b"])
        XCTAssertEqual(c.series[1].free, [100 * gb, nil, 90 * gb])
        XCTAssertEqual(c.maxTotal, 1800 * gb)
        XCTAssertEqual(c.series[1].tip(at: 1), "Macintosh HD: not read")
        XCTAssertEqual(c.series[0].tip(at: 1), "sp: 399 GB free of 1.8 TB")
    }

    func testHidingAVolumeRemovesItsSeriesAndRescalesY() {
        let c = diskChart(two, hidden: ["a"])
        XCTAssertEqual(c.series.map(\.id), ["b"])
        XCTAssertEqual(c.maxTotal, 500 * gb)
        let none = diskChart(two, hidden: ["a", "b"])
        XCTAssertTrue(none.series.isEmpty)
        XCTAssertEqual(none.maxTotal, 1)
    }

    func testColoursStayPutWhenOneIsHidden() {
        let all = Dictionary(uniqueKeysWithValues: diskChart(two, hidden: []).series.map { ($0.id, $0.color) })
        let hiddenA = Dictionary(uniqueKeysWithValues: diskChart(two, hidden: ["a"]).series.map { ($0.id, $0.color) })
        XCTAssertEqual(all["b"], hiddenA["b"])
        XCTAssertNotEqual(all["a"], all["b"])
        var p = DiskColors()
        XCTAssertEqual(p.assign(["a", "b"]), ["a": 0, "b": 1])
        XCTAssertEqual(p.assign(["b", "c"]), ["b": 1, "c": 0])
    }

    func testChartIndexIsTheNearestPoint() {
        let c = diskChart(two, hidden: [])
        XCTAssertEqual(diskChartIndex(c, fraction: 0), 0)
        XCTAssertEqual(diskChartIndex(c, fraction: 0.5), 1)
        XCTAssertEqual(diskChartIndex(c, fraction: 5), 2)
        XCTAssertNil(diskChartIndex(diskChart(Mason_V1_DiskHistory(), hidden: []), fraction: 0.5))
    }

    func testBytesUseDecimalUnitsLikeFinder() {
        XCTAssertEqual(Format.bytes(0), "0 B")
        XCTAssertEqual(Format.bytes(999), "999 B")
        XCTAssertEqual(Format.bytes(1000), "1.0 KB")
        XCTAssertEqual(Format.bytes(999 * gb), "999 GB")
        XCTAssertEqual(Format.bytes(999_600_000_000), "1.0 TB")
        XCTAssertEqual(Format.bytes(1000 * gb), "1.0 TB")
        XCTAssertEqual(Format.bytes(9_900_000_000), "9.9 GB")
        XCTAssertEqual(Format.bytes(412_300_000_000), "412 GB")
        XCTAssertEqual(Format.bytes(1800 * gb), "1.8 TB")
        XCTAssertEqual(Format.freeOf(412 * gb, 1800 * gb), "412 GB free of 1.8 TB")
    }

    func testTheLatestReadingSkipsAGapAndTheTipNamesMountAndRoles() {
        let l = diskLatest(two, id: "b")
        XCTAssertEqual(l?.free, 90 * gb)
        XCTAssertEqual(l?.total, 500 * gb)
        XCTAssertEqual(diskLatest(two, id: "a")?.free, 398 * gb)
        XCTAssertNil(diskLatest(two, id: "zzz"))
        XCTAssertEqual(diskVolumeTip(mount: a.mount, roles: a.roles), "/Volumes/sp: data, worktrees")
        XCTAssertEqual(diskVolumeTip(mount: b.mount, roles: b.roles), "/: projects, cache")
    }

    func testHiddenVolumesSurviveARestartAndAVanishedIdIsDropped() {
        let prefs = MemoryPreferences()
        let m = MonitorModel(preferences: prefs)
        m.disk.update(history: two)
        m.toggleVolume("b")
        XCTAssertEqual(m.disk.hidden, ["b"])
        XCTAssertEqual(MonitorModel(preferences: prefs).disk.hidden, ["b"])
        m.toggleVolume("b")
        XCTAssertEqual(MonitorModel(preferences: prefs).disk.hidden, [])

        m.toggleVolume("a"); m.toggleVolume("b")
        let again = MonitorModel(preferences: prefs)
        var one = two
        one.volumes = [a]
        again.disk.update(history: one)
        XCTAssertEqual(again.disk.hidden, ["a"])
        XCTAssertEqual(MonitorModel(preferences: prefs).disk.hidden, ["a"])
    }

    func testAnOlderDaemonSendsNoDiskHistoryAndTheSectionStaysHidden() {
        let m = MonitorModel()
        XCTAssertFalse(m.disk.seen)
        m.disk.update(history: two)
        XCTAssertTrue(m.disk.seen)
    }

    func testPickingASpanDropsTheOldHistory() {
        let m = MonitorModel()
        m.disk.update(history: two)
        m.setDiskSpan(.day)
        XCTAssertNil(m.disk.history)
        XCTAssertEqual(m.disk.span.ms, 86_400_000)
    }

    func testTheWorktreeTotalsAreListedAfterTheirSum() {
        XCTAssertEqual(diskWorktreeLines(two), [])
        var w = Mason_V1_WorktreeUsage()
        w.kind = "task worktree"; w.state = "canceled"; w.count = 43; w.bytes = 188 * gb; w.freeNextBytes = 188 * gb
        var l = Mason_V1_WorktreeUsage()
        l.kind = "landing scratch"; l.state = "idle"; l.count = 12; l.bytes = 26 * gb
        var h = two
        h.worktrees = [w, l]
        XCTAssertEqual(diskWorktreeLines(h), [
            "Worktrees 214 GB · next sweep frees 188 GB",
            "task worktree, canceled: 43, 188 GB",
            "landing scratch, idle: 12, 26 GB",
        ])
    }
}
