// SPDX-License-Identifier: GPL-3.0-or-later

import AppKit
import MonitorCore
import SwiftUI
import XCTest

@testable import MonitorUI

/// The node switch and the coordinator's Nodes section drawn offscreen (ImageRenderer; nothing is
/// put on screen). No golden files: the states are compared with each other, and when
/// `NODES_SNAPSHOT_DIR` names a folder the pictures are written there for a person to look at.
@MainActor
final class NodesSnapshotTests: XCTestCase {
    private func selfView(_ json: String) -> MonitorModel {
        let m = MonitorModel()
        m.nodeSelf = NodeSelf(json: json)
        return m
    }

    private func nodesView(_ json: String) -> MonitorModel {
        let m = MonitorModel()
        m.nodes = NodesSnapshot(json: json)
        return m
    }

    /// `bare`: the view alone, so one that shows nothing yields no picture.
    private func render<V: View>(_ v: V, dark: Bool, name: String, bare: Bool = false) -> [UInt8]? {
        let themed = v.environment(\.theme, Theme(dark: dark, scale: 1))
            .environment(\.colorScheme, dark ? .dark : .light)
        let content: AnyView = bare
            ? AnyView(themed)
            : AnyView(
                themed.padding(16).frame(width: 520, alignment: .leading)
                    .background(dark ? Color.black : Color.white))
        let r = ImageRenderer(content: content)
        r.scale = 2
        guard let cg = r.cgImage, cg.width > 100, cg.height > 40,
              let png = NSBitmapImageRep(cgImage: cg).representation(using: .png, properties: [:])
        else { return nil }
        if let dir = ProcessInfo.processInfo.environment["NODES_SNAPSHOT_DIR"] {
            try? FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
            try? png.write(to: URL(fileURLWithPath: dir).appendingPathComponent("\(name)-\(dark ? "dark" : "light").png"))
        }
        return [UInt8](png)
    }

    func testEveryStateOfTheSwitchRendersAndDiffers() {
        let states = [
            "off": #"{"serving": true, "switch": {"on": false}, "jobs": {"running": 0, "max": 2}, "coordinators": [{"name": "imac"}]}"#,
            "draining": #"{"serving": true, "switch": {"on": false}, "jobs": {"running": 2, "max": 4}}"#,
            "hot": #"{"serving": true, "switch": {"on": true}, "why_not": "too hot: die 96 °C", "jobs": {"running": 0, "max": 2}}"#,
            "busy": #"{"serving": true, "switch": {"on": true}, "jobs": {"running": 1, "max": 3}}"#,
            "idle": #"{"serving": true, "switch": {"on": true}, "jobs": {"running": 0, "max": 3}}"#,
        ]
        var seen: [[UInt8]] = []
        for (name, json) in states.sorted(by: { $0.key < $1.key }) {
            for dark in [false, true] {
                guard let img = render(NodeAcceptToggle(model: selfView(json)), dark: dark, name: "switch-\(name)")
                else { return XCTFail("\(name) did not render") }
                XCTAssertFalse(seen.contains(img), "\(name) looks like another state")
                seen.append(img)
            }
        }
    }

    func testAMacThatDoesNotServeDrawsNoSwitch() {
        XCTAssertNil(render(NodeAcceptToggle(model: selfView(#"{"serving": false}"#)), dark: false, name: "switch-none", bare: true))
    }

    func testTheNodesSectionDrawsEachKindOfNode() {
        let json = #"""
        {"enabled": true, "prefer": "nodes", "nodes": [
          {"name": "studio", "nid": "a1", "paired": true, "usable": true, "state": "accepting",
           "cpu_pct": 41, "thermal": {"die_c": 63.4}, "jobs": {"running": 2, "max": 6}},
          {"name": "mini", "nid": "b2", "paired": true, "usable": false,
           "state": "not accepting (the owner turned remote work off)"},
          {"name": "old-air", "nid": "c3", "paired": true, "usable": false,
           "state": "lost (6 polls without an answer)"},
          {"name": "laptop", "nid": "d4", "paired": true, "usable": false,
           "state": "local network access denied by macOS"},
          {"name": "newcomer", "nid": "e5", "paired": false, "usable": false, "state": "found, not paired"}
        ]}
        """#
        let full = render(NodesSection(model: nodesView(json)), dark: false, name: "nodes")
        XCTAssertNotNil(full)
        XCTAssertNotNil(render(NodesSection(model: nodesView(json)), dark: true, name: "nodes"))
        let off = #"{"enabled": false, "nodes": [{"name": "studio", "nid": "a1", "paired": true, "usable": false, "state": "accepting"}]}"#
        XCTAssertNotEqual(full, render(NodesSection(model: nodesView(off)), dark: false, name: "nodes-off"))
        // Nothing found and nodes off: no section at all.
        XCTAssertNil(render(NodesSection(model: nodesView(#"{"enabled": false, "nodes": []}"#)), dark: false, name: "nodes-none", bare: true))
    }
}
