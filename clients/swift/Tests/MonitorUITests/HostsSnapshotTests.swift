// SPDX-License-Identifier: GPL-3.0-or-later

import AppKit
import MonitorCore
import SwiftUI
import XCTest

@testable import MonitorUI

/// Home's Hosts section and the task page's Running on line drawn offscreen (ImageRenderer;
/// nothing is put on screen). No golden files: the states are compared with each other, and when
/// `HOSTS_SNAPSHOT_DIR` names a folder the pictures are written there.
@MainActor
final class HostsSnapshotTests: XCTestCase {
    // The same recorded view as MonitorCoreTests/HostsTests (the test targets cannot share code).
    private static let json = #"""
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

    private func model() throws -> MonitorModel {
        let m = MonitorModel()
        m.home = try HomeView(jsonString: Self.json)
        return m
    }

    private func render<V: View>(_ v: V, name: String, width: CGFloat = 520, bare: Bool = false) -> [UInt8]? {
        let themed = v.environment(\.theme, Theme(dark: false, scale: 1)).environment(\.colorScheme, .light)
        let content: AnyView = bare
            ? AnyView(themed)
            : AnyView(themed.padding(16).frame(width: width, alignment: .leading).background(Color.white))
        let r = ImageRenderer(content: content)
        r.scale = 2
        guard let cg = r.cgImage, cg.width > 100, cg.height > 40,
              let png = NSBitmapImageRep(cgImage: cg).representation(using: .png, properties: [:])
        else { return nil }
        if let dir = ProcessInfo.processInfo.environment["HOSTS_SNAPSHOT_DIR"] {
            try? FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
            try? png.write(to: URL(fileURLWithPath: dir).appendingPathComponent("\(name).png"))
        }
        return [UInt8](png)
    }

    func testHomeWithThreeHostsRendersAndTheSectionIsHiddenWithoutHosts() throws {
        XCTAssertNotNil(render(HostsSection(model: try model()), name: "hosts-home"))
        XCTAssertNil(render(HostsSection(model: MonitorModel()), name: "hosts-none", bare: true))
    }

    func testThePhoneLayoutRendersAndExpandingChangesIt() throws {
        let m = try model()
        let phone = render(HostsSection(model: m, compact: true), name: "hosts-phone", width: 375)
        XCTAssertNotNil(phone)
        XCTAssertNotEqual(phone, render(HostsSection(model: m), name: "hosts-wide", width: 375))
    }

    func testTheTaskPageShowsRunningOnWithTwoEntriesAndWithNone() throws {
        let m = try model()
        let runs = Array(m.home!.hosts[1].runs + m.home!.hosts[2].runs)
        let two = render(RunningOnLine(model: m, text: Hosts.runningOn(runs, now: 100_000) ?? "", runs: runs), name: "running-on-two")
        let none = render(RunningOnLine(model: m, text: Hosts.runningOn([], now: 0) ?? "", runs: []), name: "running-on-none")
        XCTAssertNotNil(two)
        XCTAssertNotNil(none)
        XCTAssertNotEqual(two, none)
    }

    func testTappingARunsTaskLinkGoesToThatTask() throws {
        let m = try model()
        let r = m.home!.hosts[1].runs[0]
        XCTAssertTrue(Hosts.hasLink(r))
        // The row's link calls exactly this.
        m.go(.task(r.task.id))
        XCTAssertEqual(m.page, .task(7))
    }
}
