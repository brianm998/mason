// SPDX-License-Identifier: GPL-3.0-or-later

import XCTest
@testable import MonitorCore

/// The node views: this Mac's switch, and a coordinator's list of nodes, read from the daemon's JSON.
final class NodesModelTests: XCTestCase {
    func testASwitchOffNodeSaysNothingFromOtherMacsRunsHere() {
        let n = NodeSelf(json: #"""
        {"serving": true, "switch": {"on": false}, "why_not": "the owner turned remote work off",
         "jobs": {"running": 0, "max": 2}, "coordinators": [{"name": "imac"}]}
        """#)
        XCTAssertTrue(n.serving)
        XCTAssertFalse(n.switchOn)
        XCTAssertEqual(n.coordinators, ["imac"])
        XCTAssertEqual(n.status, "Off: nothing from other Macs runs here.")
    }

    func testJobsThatAreStillRunningFinishWhenTheSwitchIsOff() {
        let n = NodeSelf(json: #"{"serving": true, "switch": {"on": false}, "jobs": {"running": 2, "max": 4}}"#)
        XCTAssertEqual(n.status, "Off. 2 jobs still running here will finish.")
        let one = NodeSelf(json: #"{"serving": true, "switch": {"on": false}, "jobs": {"running": 1, "max": 4}}"#)
        XCTAssertEqual(one.status, "Off. 1 job still running here will finish.")
    }

    func testAnOnNodeSaysWhyItTakesNothingWhenItDoesNot() {
        let hot = NodeSelf(json: #"{"serving": true, "switch": {"on": true}, "why_not": "too hot: die 96 °C", "jobs": {"running": 0, "max": 2}}"#)
        XCTAssertEqual(hot.status, "On, but not taking work now: too hot: die 96 °C.")
        let ok = NodeSelf(json: #"{"serving": true, "switch": {"on": true}, "jobs": {"running": 1, "max": 3}}"#)
        XCTAssertEqual(ok.status, "On. 1 of 3 jobs running.")
        let idle = NodeSelf(json: #"{"serving": true, "switch": {"on": true}, "jobs": {"running": 0, "max": 3}}"#)
        XCTAssertEqual(idle.status, "On: taking work from other Macs.")
    }

    func testAMacThatDoesNotServeHasNoSwitch() {
        XCTAssertFalse(NodeSelf(json: #"{"serving": false, "switch": {"on": false}}"#).serving)
        XCTAssertFalse(NodeSelf(json: "not json").serving)
        XCTAssertFalse(NodeSelf().serving)
    }

    func testTheCoordinatorsListShowsEachNodeAndHowManyTakeWork() throws {
        let s = NodesSnapshot(json: #"""
        {"enabled": true, "prefer": "nodes", "nodes": [
          {"name": "studio", "nid": "n1", "paired": true, "usable": true, "state": "accepting",
           "cpu_pct": 23, "thermal": {"die_c": 61.4}, "jobs": {"running": 1, "max": 4}},
          {"name": "mini", "nid": "n2", "paired": true, "usable": false,
           "state": "not accepting (the owner turned remote work off)"},
          {"name": "found", "nid": "n3", "paired": false, "usable": false, "state": "found over mDNS, not paired"}
        ]}
        """#)
        XCTAssertTrue(s.enabled)
        XCTAssertTrue(s.worthShowing)
        XCTAssertEqual(s.nodes.count, 3)
        XCTAssertEqual(s.header, "1 of 2 nodes taking work")
        let studio = try XCTUnwrap(s.nodes.first)
        XCTAssertEqual(studio.load, "CPU 23%, 61 °C, 1 of 4 jobs")
        XCTAssertEqual(s.nodes[1].load, "")
        XCTAssertEqual(studio.id, "n1")
    }

    func testTheSectionHidesUntilThereIsANodeOrNodesAreOn() {
        XCTAssertFalse(NodesSnapshot(json: #"{"enabled": false, "nodes": []}"#).worthShowing)
        XCTAssertTrue(NodesSnapshot(json: #"{"enabled": true, "nodes": []}"#).worthShowing)
        XCTAssertEqual(NodesSnapshot(json: #"{"enabled": true, "nodes": []}"#).header, "No node is paired.")
        XCTAssertEqual(NodesSnapshot(json: #"{"enabled": false, "nodes": [{"name": "x", "paired": true}]}"#).header,
                       "0 of 1 node taking work (nodes are off)")
    }

    func testEachNodesUpdateIsShownAndOnlyAStuckOneOffersUpdateNow() throws {
        let s = NodesSnapshot(json: #"""
        {"enabled": true, "prefer": "nodes", "nodes": [
          {"name": "marmot", "nid": "n1", "paired": true, "usable": true, "state": "accepting",
           "update": "building for arm64", "update_state": "building"},
          {"name": "mini", "nid": "n2", "paired": true, "usable": false, "state": "lost (6 polls without an answer)",
           "update": "will update when it returns", "update_state": "pending"},
          {"name": "studio", "nid": "n3", "paired": true, "usable": true, "state": "accepting",
           "update": "up to date", "update_state": "up-to-date"},
          {"name": "air", "nid": "n4", "paired": true, "usable": true, "state": "accepting",
           "update": "failed (the node said no)", "update_state": "failed"}
        ]}
        """#)
        XCTAssertEqual(s.nodes.map(\.canUpdateNow), [false, true, false, true])
        XCTAssertEqual(s.nodes[0].update, "building for arm64")
        XCTAssertEqual(
            s.updateSummary,
            "marmot: building for arm64; mini: will update when it returns; air: failed (the node said no)"
        )
    }

    func testTheUpdateSummaryIsQuietWhenEveryNodeIsUpToDateOrTheDaemonSaysNothing() {
        let done = NodesSnapshot(json: #"""
        {"enabled": true, "nodes": [
          {"name": "a", "paired": true, "update": "up to date", "update_state": "up-to-date"},
          {"name": "b", "paired": true, "update": "up to date", "update_state": "up-to-date"}]}
        """#)
        XCTAssertEqual(done.updateSummary, "2 nodes up to date")
        let one = NodesSnapshot(json: #"{"nodes": [{"name": "a", "paired": true, "update": "up to date", "update_state": "up-to-date"}]}"#)
        XCTAssertEqual(one.updateSummary, "1 node up to date")
        // An older daemon sends no update, an unpaired node has none.
        let old = NodesSnapshot(json: #"{"nodes": [{"name": "a", "paired": true}, {"name": "b", "paired": false}]}"#)
        XCTAssertNil(old.updateSummary)
        XCTAssertFalse(old.nodes[0].canUpdateNow)
    }
}
