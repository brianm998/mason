// SPDX-License-Identifier: GPL-3.0-or-later

import XCTest

@testable import MonitorCore

/// The words of Home's Hosts section and the task page's "Running on" line (#1156).
final class HostsTests: XCTestCase {
    static let json = #"""
    {"status": "ok", "github": {"enabled": true}, "hosts": [
      {"kind": "local", "name": "this Mac", "status": "held for heat: die 95 °C", "runs": []},
      {"kind": "node", "name": "marmot", "status": "accepting, 2 of 4 jobs", "runs": [
        {"task": {"id": "7", "key": "mason #7", "title": "Fix the thing", "project": "mason",
                  "state": "developing", "activity": "writing the tests"},
         "kind": "agent", "name": "develop", "host": "node:marmot", "sinceMs": "1000"},
        {"kind": "batch", "name": "b12", "host": "node:marmot"}]},
      {"kind": "github", "name": "GitHub", "status": "6 of 20 runners", "runs": [
        {"task": {"id": "9", "key": "mason #9", "title": "Other", "state": "testing"},
         "kind": "check", "name": "cargo-workspace", "host": "github:macos",
         "sinceMs": "61000", "url": "https://github.com/o/r/actions/runs/1"}]}]}
    """#

    static func home() throws -> HomeView { try HomeView(jsonString: json) }

    func testDecodesAHomeViewWithHosts() throws {
        let h = try Self.home()
        XCTAssertEqual(h.hosts.count, 3)
        XCTAssertEqual(h.hosts.map { $0.runs.count }, [0, 2, 1])
    }

    func testTitles() throws {
        let h = try Self.home().hosts
        XCTAssertEqual(h.map(Hosts.title), ["This Mac", "marmot", "GitHub"])
    }

    func testIdleHostSaysIdle() throws {
        let h = try Self.home().hosts
        XCTAssertEqual(Hosts.count(h[0]), "idle")
        XCTAssertEqual(Hosts.count(h[1]), "2 runs")
        XCTAssertEqual(Hosts.compactTitle(h[1]), "marmot · 2 runs")
    }

    func testRunLineHasStateActivityWhatAndDuration() throws {
        let r = try Self.home().hosts[1].runs[0]
        XCTAssertEqual(Hosts.label(r), "mason #7 Fix the thing")
        XCTAssertEqual(Hosts.runLine(r, now: 61_000), "developing: writing the tests · agent develop · 1 min")
    }

    func testLongActivityIsShortenedToOneLine() {
        var r = HostRun()
        r.task.id = 1
        r.task.state = "developing"
        r.task.activity = String(repeating: "x", count: 200) + "\nsecond line"
        let line = Hosts.stateLine(r)
        XCTAssertFalse(line.contains("\n"))
        XCTAssertLessThanOrEqual(line.count, "developing: ".count + Hosts.activityLimit)
        XCTAssertTrue(line.hasSuffix("…"))
    }

    func testARunWithNoTaskHasNoLink() throws {
        let r = try Self.home().hosts[1].runs[1]
        XCTAssertFalse(Hosts.hasLink(r))
        XCTAssertEqual(Hosts.label(r), "batch b12")
        XCTAssertEqual(Hosts.stateLine(r), "")
        XCTAssertTrue(Hosts.hasLink(try Self.home().hosts[1].runs[0]))
    }

    func testAnOlderDaemonYieldsNoSection() {
        XCTAssertNil(Hosts.visible(HomeView()))
        XCTAssertNil(Hosts.visible(nil))
        XCTAssertNil(Hosts.runningOn([], now: 0, known: false))
    }

    func testASingleMachineWithNothingRunningHidesTheSection() throws {
        var h = HomeView()
        var host = HostView()
        host.kind = "local"
        host.name = "this Mac"
        h.hosts = [host]
        XCTAssertNil(Hosts.visible(h))
        XCTAssertNotNil(Hosts.visible(h, nodesWorthShowing: true))
        h.github.enabled = true
        XCTAssertNotNil(Hosts.visible(h))
    }

    func testRunningOnLine() throws {
        let runs = try Self.home().hosts[2].runs
        XCTAssertEqual(Hosts.runningOn(runs, now: 121_000),
                       "Running on: GitHub (check cargo-workspace, 1 min)")
        XCTAssertEqual(Hosts.runningOn([], now: 0), "Running on: not running anywhere now")
    }
}
