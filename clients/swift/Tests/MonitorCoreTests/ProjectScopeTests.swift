// SPDX-License-Identifier: GPL-3.0-or-later

import MasonProtocol
import XCTest

@testable import MonitorCore

/// s321: every project-scoped view is stamped with its project; the model drops one for another filter.
@MainActor
final class ProjectScopeTests: XCTestCase {
    func header(_ project: String, attention: UInt32) -> Mason_V1_View {
        var h = Mason_V1_HeaderView()
        h.project = project
        h.attention = attention
        h.projects = ["a", "b"]
        var v = Mason_V1_View()
        v.body = .header(h)
        return v
    }

    func testAViewStampedForAnotherProjectIsDropped() {
        let m = MonitorModel()
        m.project = "b"
        m.apply(header("a", attention: 3))
        XCTAssertNil(m.header)
        m.apply(header("b", attention: 1))
        XCTAssertEqual(m.header?.attention, 1)
        var home = Mason_V1_HomeView()
        home.project = "a"
        var v = Mason_V1_View()
        v.body = .home(home)
        m.apply(v)
        XCTAssertNil(m.home)
    }

    func testChangingTheSelectionClearsCountsUntilTheNewViewArrives() {
        let m = MonitorModel()
        var h = header("", attention: 4)
        if case .header(var x)? = h.body { x.counts = [StateCount()]; h.body = .header(x) }
        m.apply(h)
        let home = Mason_V1_HomeView()
        var hv = Mason_V1_View()
        hv.body = .home(home)
        m.apply(hv)
        m.project = "b"
        XCTAssertEqual(m.header?.attention, 0)
        XCTAssertTrue(m.header?.counts.isEmpty ?? false)
        XCTAssertEqual(m.header?.projects, ["a", "b"])
        XCTAssertNil(m.home)
        m.apply(header("", attention: 4))
        XCTAssertEqual(m.header?.attention, 0)
        m.apply(header("b", attention: 1))
        XCTAssertEqual(m.header?.attention, 1)
    }

    func testAStaleLandingViewIsNeverKept() {
        let m = MonitorModel()
        var l = Mason_V1_LandingView()
        l.project = ""
        var v = Mason_V1_View()
        v.body = .landing(l)
        m.apply(v)
        XCTAssertNotNil(m.landing)
        m.project = "b"
        XCTAssertNil(m.landing)
        m.apply(v)
        XCTAssertNil(m.landing)
    }
}
