// SPDX-License-Identifier: GPL-3.0-or-later

import XCTest
import MasonProtocol
@testable import MonitorCore

/// #834: the placement control builds the right command for each level.
final class PlacementTests: XCTestCase {
    func model(owner: Bool = true) -> (MonitorModel, () -> [Mason_V1_Command]) {
        let m = MonitorModel()
        var sent: [Mason_V1_Command] = []
        m.sender = { sent.append($0) }
        m.identity = "brian"
        var w = Mason_V1_ServerMessage()
        w.welcome = .with { $0.protocol = 2; $0.owner = owner }
        m.apply(w)
        m.home = .with {
            $0.placement = .with { $0.mode = "local"; $0.source = "global"; $0.own = true; $0.settable = true }
        }
        m.header = .with {
            $0.projectPlacements = [.with {
                $0.project = "scratch"
                $0.placement = .with { $0.mode = "favor-remote"; $0.source = "project"; $0.own = true; $0.settable = true }
            }]
        }
        m.taskDetail = .with {
            $0.row = .with { $0.id = 7 }
            $0.placement = .with { $0.mode = "remote"; $0.source = "task"; $0.own = true; $0.settable = true }
        }
        return (m, { sent })
    }

    func testEachLevelBuildsItsCommand() {
        let (m, sent) = model()
        m.askPlacement(.global, mode: "remote")
        m.confirmPlacement()
        m.askPlacement(.project("scratch"), mode: "favor-local")
        m.confirmPlacement()
        m.askPlacement(.task(7), mode: "local")
        m.confirmPlacement()
        let got = sent().compactMap { c -> [String]? in
            guard case .setPlacement(let p)? = c.body else { return nil }
            return [p.scope, p.project, String(p.task), p.mode]
        }
        XCTAssertEqual(got, [["global", "", "0", "remote"], ["project", "scratch", "0", "favor-local"], ["task", "", "7", "local"]])
        XCTAssertEqual(sent()[0].actor, "brian")
    }

    func testFollowSendsDefault() {
        let (m, sent) = model()
        XCTAssertEqual(PlacementScope.project("scratch").followLabel, "Follow MASON")
        XCTAssertEqual(PlacementScope.task(7).followLabel, "Follow project")
        XCTAssertNil(PlacementScope.global.followLabel)
        m.askPlacement(.project("scratch"), mode: "default")
        m.confirmPlacement()
        m.cancelPlacement()
        m.askPlacement(.task(7), mode: "default")
        m.confirmPlacement()
        let modes = sent().compactMap { c -> String? in
            guard case .setPlacement(let p)? = c.body else { return nil }
            return p.mode
        }
        XCTAssertEqual(modes, ["default", "default"])
    }

    func testNothingIsSentUntilConfirmed() {
        let (m, sent) = model()
        m.askPlacement(.global, mode: "local")
        XCTAssertTrue(sent().isEmpty)
        XCTAssertNotNil(m.placementAsk)
        m.cancelPlacement()
        XCTAssertTrue(sent().isEmpty)
        XCTAssertNil(m.placementAsk)
    }

    func testTextNamesTheModeAndTheLevelThatSetIt() {
        let (m, _) = model()
        XCTAssertEqual(m.placementText(.project("scratch")), "favor-remote, from the project")
        XCTAssertEqual(m.placementText(.global), "local, from the global switch")
        XCTAssertEqual(m.placementText(.task(7)), "remote, from the task")
    }

    func testOnlyAnOwnerDeviceThatCanActGetsTheControl() {
        let (viewer, _) = model(owner: false)
        XCTAssertFalse(viewer.canSetPlacement(.global))
        let (owner, _) = model()
        XCTAssertTrue(owner.canSetPlacement(.global))
        XCTAssertTrue(owner.canFollow(.project("scratch")))
        XCTAssertFalse(owner.canFollow(.global))
        owner.readOnlyReason = "old monitor"
        XCTAssertFalse(owner.canSetPlacement(.task(7)))
    }

    func testPendingLabelShowsUntilTheViewsArrive() {
        let (m, _) = model()
        m.askPlacement(.project("scratch"), mode: "local")
        m.confirmPlacement()
        XCTAssertEqual(m.placementPendingLabel(.project("scratch")), "Changing where runs go…")
        XCTAssertNil(m.placementPendingLabel(.global))
    }

    func testRowMarkerOnlyForAnOverride() {
        var r = Mason_V1_TaskRow()
        XCTAssertNil(MonitorModel.runOnMarker(r))
        r.runOnOverride = "remote"
        XCTAssertEqual(MonitorModel.runOnMarker(r), "runs on remote")
    }
}
