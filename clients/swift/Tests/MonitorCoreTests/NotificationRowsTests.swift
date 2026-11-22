// SPDX-License-Identifier: GPL-3.0-or-later

import MasonProtocol
import XCTest

@testable import MonitorCore

/// s586: the notification bar's rows have stable keys, can be dismissed and shown again, and the
/// bar's height is remembered and clamped.
@MainActor
final class NotificationRowsTests: XCTestCase {
    private func banner(_ kind: String, project: String = "p", mode: String = "", session: UInt64 = 0, _ text: String) -> Mason_V1_Banner {
        var b = Mason_V1_Banner()
        b.kind = kind
        b.project = project
        b.mode = mode
        b.session = session
        b.text = text
        return b
    }

    private func model(_ banners: [Mason_V1_Banner], prefs: Preferences = MemoryPreferences()) -> MonitorModel {
        let m = MonitorModel(preferences: prefs)
        m.connected = true
        setHeader(m, banners)
        return m
    }

    private func setHeader(_ m: MonitorModel, _ banners: [Mason_V1_Banner]) {
        var h = HeaderView()
        h.banners = banners
        m.header = h
    }

    private func keys(_ m: MonitorModel) -> [String] { m.notificationRows().map(\.key) }

    func testKeysSurviveReorderingAndChangeWithTheText() {
        let a = banner("ci", "CI still red on run 5; #9 covered it")
        let b = banner("info", "Something else")
        let m = model([a, b])
        let before = Set(keys(m))
        setHeader(m, [b, a])
        XCTAssertEqual(Set(keys(m)), before)
        setHeader(m, [banner("ci", "CI still red on run 6; #9 covered it"), b])
        XCTAssertNotEqual(Set(keys(m)), before)
        XCTAssertEqual(Set(keys(m)).intersection(before).count, 1)
    }

    func testDismissHidesOnlyThatRow() {
        let m = model([banner("a", "one"), banner("b", "two")])
        let all = keys(m)
        m.dismissNotification(all[0])
        XCTAssertEqual(keys(m), [all[1]])
        XCTAssertEqual(m.hiddenNotificationCount(), 1)
    }

    func testPruneForgetsAGoneRowAndItShowsAgainWhenItReturns() {
        let a = banner("a", "one")
        let m = model([a, banner("b", "two")])
        let key = keys(m)[0]
        m.dismissNotification(key)
        setHeader(m, [banner("b", "two")])
        XCTAssertEqual(m.hiddenNotificationCount(), 0)
        setHeader(m, [a, banner("b", "two")])
        XCTAssertTrue(keys(m).contains(key))
        XCTAssertEqual(m.hiddenNotificationCount(), 0)
    }

    func testShowHiddenClearsOnlyThePresentOnes() {
        let m = model([banner("a", "one"), banner("b", "two")])
        let all = keys(m)
        m.dismissNotification(all[0])
        m.dismissNotification("gone-key")
        XCTAssertEqual(m.hiddenNotificationCount(), 1)
        m.showHiddenNotifications()
        XCTAssertEqual(keys(m), all)
        XCTAssertEqual(m.hiddenNotificationCount(), 0)
    }

    func testDismissalsAndHeightSurviveANewModel() {
        let prefs = MemoryPreferences()
        let banners = [banner("a", "one"), banner("b", "two")]
        let m = model(banners, prefs: prefs)
        let key = keys(m)[0]
        m.dismissNotification(key)
        m.setNotificationHeight(123)
        let again = model(banners, prefs: prefs)
        XCTAssertEqual(again.hiddenNotificationCount(), 1)
        XCTAssertFalse(keys(again).contains(key))
        XCTAssertEqual(again.notificationHeight, 123)
    }

    func testClientRowsKeepTheirIDs() {
        let m = MonitorModel()
        m.connected = false
        XCTAssertEqual(m.notificationRows().map(\.key), ["connection"])
    }

    func testHeightClamp() {
        let clamp = MonitorModel.clampNotificationHeight
        XCTAssertEqual(clamp(5, 30, 300, 1000), 30)
        XCTAssertEqual(clamp(900, 30, 300, 1000), 300)
        XCTAssertEqual(clamp(900, 30, 900, 1000), 600)
        XCTAssertEqual(clamp(100, 30, 300, 1000), 100)
        // A tiny window never squeezes the bar below one row.
        XCTAssertEqual(clamp(100, 30, 300, 20), 30)
    }
}

/// s929: info banners make no bar row and appear in the status-line summary.
@MainActor
final class NotificationLevelTests: XCTestCase {
    private func banner(_ kind: String, _ level: String, _ text: String) -> Mason_V1_Banner {
        var b = Mason_V1_Banner()
        b.kind = kind
        b.level = level
        b.text = text
        return b
    }

    func testOnlyActionAndWarningMakeRowsAndInfoIsSummarised() {
        let m = MonitorModel(preferences: MemoryPreferences())
        m.connected = true
        var h = HeaderView()
        h.banners = [
            banner("push_auth", "action", "star: push refused"),
            banner("ci_watch", "info", "mason: 2 red CI jobs being fixed: #935, #836"),
            banner("landing_frozen", "warning", "Landing is frozen for mason"),
        ]
        var u = Mason_V1_UpdateInfo()
        u.daemonText = "Updating masond: building 4259e0bc…"
        u.daemonLevel = "info"
        h.update = u
        m.header = h
        XCTAssertEqual(m.notificationRows().map(\.text), ["star: push refused", "Landing is frozen for mason"])
        XCTAssertEqual(m.infoSummary, "masond updating · 2 CI jobs being fixed")
        XCTAssertEqual(m.infoNotices.count, 2)
    }

    func testNoLevelStaysARow() {
        let m = MonitorModel(preferences: MemoryPreferences())
        m.connected = true
        var h = HeaderView()
        h.banners = [banner("x", "", "old daemon")]
        m.header = h
        XCTAssertEqual(m.notificationRows().count, 1)
        XCTAssertEqual(m.infoSummary, "")
    }
}
