// SPDX-License-Identifier: GPL-3.0-or-later

import MasonProtocol
import XCTest

@testable import MonitorCore

/// s459: the phone's navigation (list, then detail) for every page, its status line, tile taps,
/// and the session stepper's command.
@MainActor
final class PhoneNavTests: XCTestCase {
    private let pages: [Page] = [.home, .task(7), .landing, .usage, .health, .settings,
                                 .section("active_models"), .section("cpu"), .section("compilation"),
                                 .section("queued"), .section("exploring"), .section("developing"),
                                 .section("testing"), .section("review")]

    func testEveryPagePushesAndPops() {
        for page in pages {
            var n = PhoneNav()
            XCTAssertEqual(n.screen, .sidebar)
            n.select(page)
            XCTAssertEqual(n.screen, .detail(page), "\(page)")
            n.select(page)
            XCTAssertEqual(n.stack, [page], "choosing the page on show does not push it twice")
            n.back()
            XCTAssertEqual(n.screen, .sidebar, "\(page)")
        }
    }

    func testPagesStackAndPopOneAtATime() {
        var n = PhoneNav()
        n.select(.section("developing"))
        n.select(.task(3))
        XCTAssertEqual(n.stack, [.section("developing"), .task(3)])
        n.back()
        XCTAssertEqual(n.screen, .detail(.section("developing")))
        n.replace([])
        XCTAssertEqual(n.screen, .sidebar, "a swipe back reports the shorter stack")
    }

    func testThePipelineEditorIsNotOnAPhone() {
        var n = PhoneNav()
        n.select(.pipeline)
        XCTAssertEqual(n.screen, .sidebar)
        XCTAssertFalse(PhoneLayout.hasPage(.pipeline))
        XCTAssertTrue(PhoneLayout.phoneHasNo.contains("graph editing"))
        XCTAssertTrue(pages.allSatisfy(PhoneLayout.hasPage))
    }

    func testEveryMacPageExceptTheEditorHasAPhonePage() {
        // The Mac's pages: Page's cases and its section pages.
        let mac: [Page] = [.home, .task(1), .landing, .usage, .pipeline, .health, .settings]
            + Page.sectionNames.compactMap(Page.sectionPage)
        for p in mac where p != .pipeline { XCTAssertTrue(PhoneLayout.hasPage(p), "\(p)") }
    }

    func testAPageTheModelOpensIsFollowedButHomeIsNotPushedOverTheSidebar() {
        var n = PhoneNav()
        n.follow(.home)
        XCTAssertEqual(n.screen, .sidebar)
        n.follow(.task(9))
        XCTAssertEqual(n.screen, .detail(.task(9)))
        n.follow(.home)
        XCTAssertEqual(n.screen, .detail(.home))
    }

    func testStatusLine() {
        XCTAssertEqual(PhoneLayout.statusLine(connected: true, reconnecting: false, paused: false, attention: 0), "Connected")
        XCTAssertEqual(PhoneLayout.statusLine(connected: true, reconnecting: false, paused: true, attention: 1), "Connected · Paused · 1 needs you")
        XCTAssertEqual(PhoneLayout.statusLine(connected: false, reconnecting: true, paused: false, attention: 0), "Reconnecting…")
        XCTAssertEqual(PhoneLayout.statusLine(connected: false, reconnecting: false, paused: false, attention: 0), "Offline")
        XCTAssertEqual(PhoneLayout.statusLine(connected: true, reconnecting: false, paused: false, attention: 3), "Connected · 3 need you")
    }

    func testATileTapPushesTheLoneTaskOrTheStatePage() {
        XCTAssertEqual(PhoneLayout.tileTarget(state: "developing", rows: [4], connected: true), .task(4))
        XCTAssertEqual(PhoneLayout.tileTarget(state: "developing", rows: [4, 5], connected: true), .section("developing"))
        XCTAssertEqual(PhoneLayout.tileTarget(state: "developing", rows: [], connected: true), .section("developing"))
        XCTAssertEqual(PhoneLayout.tileTarget(state: "landing", rows: [4, 5], connected: true), .landing)
        XCTAssertEqual(PhoneLayout.tileTarget(state: "failed", rows: [4, 5], connected: true), .section("failed"))
        XCTAssertEqual(PhoneLayout.tileTarget(state: "failed", rows: [4], connected: true), .task(4))
        for s in ["needs_attention", "proposed"] {
            XCTAssertEqual(PhoneLayout.tileTarget(state: s, rows: [4, 5], connected: true), .section(s))
            XCTAssertEqual(PhoneLayout.tileTarget(state: s, rows: [4], connected: true), .task(4))
        }
        XCTAssertNil(PhoneLayout.tileTarget(state: "developing", rows: [4], connected: false))
    }

    func testTheSessionStepperSendsWhatTheMacSends() {
        let m = MonitorModel()
        var sent: [Mason_V1_Command] = []
        m.sender = { sent.append($0) }
        var home = HomeView()
        home.sessionsLimit = 4
        m.home = home
        m.stepSessionLimit(by: 1)
        m.stepSessionLimit(by: -1)
        home.sessionsLimit = 0
        m.home = home
        m.stepSessionLimit(by: -1)
        XCTAssertEqual(sent.count, 3)
        let units: [UInt32] = sent.compactMap { if case .setLimit(let l)? = $0.body { return l.units } else { return nil } }
        XCTAssertEqual(units, [5, 3, 0])
        if case .setLimit(let l)? = sent[0].body { XCTAssertEqual(l.resource, "llm:claude") } else { XCTFail() }
    }
}
