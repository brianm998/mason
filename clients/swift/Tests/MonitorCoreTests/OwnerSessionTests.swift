// SPDX-License-Identifier: GPL-3.0-or-later

import MasonProtocol
import XCTest

@testable import MonitorCore

/// s432: the health banner's Open session button and the sidebar's Active sessions panel.
@MainActor
final class OwnerSessionTests: XCTestCase {
    private func session(_ id: UInt64, kind: String, label: String, working: Bool = false, waiting: Bool = false) -> Mason_V1_ActiveSession {
        var s = Mason_V1_ActiveSession()
        s.id = id
        s.kind = kind
        s.label = label
        s.working = working
        s.waiting = waiting
        return s
    }

    func testThePanelCountsTaskAndHealthSessionsAndHidesAtZero() {
        XCTAssertNil(ActiveSessions.title([]))
        let both = [session(1, kind: "task", label: "#4 Fix it", working: true), session(2, kind: "health", label: "Pipeline health", waiting: true)]
        XCTAssertEqual(ActiveSessions.title(both), "Active sessions (2)")
        XCTAssertEqual(ActiveSessions.marker(both[0]), "replying")
        XCTAssertEqual(ActiveSessions.marker(both[1]), "waiting for you")
        XCTAssertEqual(ActiveSessions.marker(session(3, kind: "task", label: "x")), "")
    }

    func testOpenSessionOpensTheDialogOnThatSession() {
        let model = MonitorModel()
        var banner = Mason_V1_Banner()
        banner.kind = "health"
        banner.session = 7
        banner.text = "Pipeline health #7: Cancel the slow task?"
        var h = Mason_V1_HeaderView()
        h.banners = [banner]
        model.header = h
        XCTAssertNil(model.ownerSessionDialog)
        model.openOwnerSession(model.header!.banners[0].session)
        XCTAssertEqual(model.ownerSessionDialog, 7)
        model.closeOwnerSession()
        XCTAssertNil(model.ownerSessionDialog)
    }

    func testATranscriptIsReadFromTheDaemonsAnswer() {
        let t = OwnerSessionTranscript(json: #"{"session":{"id":7,"kind":"health","state":"open","working":false,"messages":[{"id":1,"role":"agent","text":"hello"}]}}"#)
        XCTAssertEqual(t.id, 7)
        XCTAssertEqual(t.kind, "health")
        XCTAssertEqual(t.messages.map(\.text), ["hello"])
    }
}
