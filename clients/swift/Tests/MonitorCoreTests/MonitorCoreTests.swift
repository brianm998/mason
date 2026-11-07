// SPDX-License-Identifier: GPL-3.0-or-later

import MasonProtocol
import XCTest

@testable import MonitorCore

@MainActor
final class MonitorCoreTests: XCTestCase {
    // MARK: fixtures

    func row(_ id: UInt64, state: String, title: String = "t", project: String = "web", body: String = "", actions: [String] = []) -> TaskRow {
        var r = TaskRow()
        r.id = id
        r.key = "#\(id)"
        r.title = title
        r.project = project
        r.state = state
        r.body = body
        r.allowedActions = actions
        return r
    }

    func section(_ state: String, collapsed: Bool = false, rows: [TaskRow] = []) -> SidebarSection {
        var s = SidebarSection()
        s.state = state
        s.title = state.capitalized
        s.count = UInt32(rows.count)
        s.collapsed = collapsed
        s.rows = rows
        return s
    }

    func model(connected: Bool = true) -> MonitorModel {
        let m = MonitorModel()
        var h = HeaderView()
        h.status = "1 needs you"
        h.attention = 1
        var c = StateCount()
        c.state = "queued"
        c.count = 4
        h.counts = [c]
        m.header = h
        var sb = SidebarView()
        sb.sections = [
            section("queued", rows: [row(1, state: "queued", title: "Rate limits", body: "add headers"), row(2, state: "queued", title: "Paging", project: "ios")]),
            section("failed", collapsed: true, rows: [row(3, state: "failed", actions: ["retry", "cancel"])]),
        ]
        m.sidebar = sb
        m.connected = connected
        return m
    }

    // MARK: navigation

    func testHistoryIsBrowserStyle() {
        var h = NavHistory()
        XCTAssertFalse(h.canGoBack || h.canGoForward)
        h.go(.task(1)); h.go(.landing)
        XCTAssertEqual(h.current, .landing)
        h.goBack()
        XCTAssertEqual(h.current, .task(1))
        XCTAssertTrue(h.canGoForward)
        h.goForward()
        XCTAssertEqual(h.current, .landing)
        // Selecting something new clears what was gone forward to.
        h.goBack(); h.go(.usage)
        XCTAssertFalse(h.canGoForward)
        // Home clears the selection and is itself a step.
        h.goHome()
        XCTAssertEqual(h.current, .home)
        h.goBack()
        XCTAssertEqual(h.current, .usage)
        // Going to where you already are changes nothing.
        let before = h
        h.go(.usage)
        XCTAssertEqual(h, before)
    }

    func testDeepLinksRoundTripAndRefuseOtherThings() {
        for p in [Page.home, .task(204), .landing, .usage, .pipeline, .health] {
            XCTAssertEqual(Page.from(url: p.url), p, "\(p)")
        }
        XCTAssertNil(Page.from(url: URL(string: "https://example.com/task/1")!))
        XCTAssertNil(Page.from(url: URL(string: "mason://task/not-a-number")!))
        XCTAssertNil(Page.from(url: URL(string: "mason://task")!))
        XCTAssertNil(Page.from(url: URL(string: "mason://landing/extra")!))
        let m = model()
        m.open(url: URL(string: "mason://task/7")!)
        XCTAssertEqual(m.page, .task(7))
        m.open(url: URL(string: "mason://nonsense")!)
        XCTAssertEqual(m.page, .task(7), "an unknown link changes nothing")
    }

    // MARK: counts, banners and attention while disconnected

    func testTilesShowADashWhileDisconnectedNeverAStaleCountOrAFalseZero() {
        let m = model(connected: true)
        let c = m.header!.counts[0]
        XCTAssertEqual(m.tileCount(c), "4")
        XCTAssertEqual(m.attention, 1)
        m.disconnected()
        XCTAssertEqual(m.tileCount(c), "–")
        XCTAssertEqual(m.attention, 0, "no attention indicator on a stale count")
        let item = m.visibleItems().first { $0.id == "tile:queued" }!
        XCTAssertEqual(item.texts.first, "–")
    }

    // MARK: sidebar

    func testHistorySectionsStartCollapsedAndChoicesAreRemembered() {
        let prefs = MemoryPreferences()
        let a = MonitorModel(preferences: prefs)
        a.sidebar = model().sidebar
        let failed = a.sidebar!.sections[1]
        let queued = a.sidebar!.sections[0]
        XCTAssertFalse(a.isExpanded(failed), "failed starts collapsed")
        XCTAssertTrue(a.isExpanded(queued))
        a.toggle(failed); a.toggle(queued)
        XCTAssertTrue(a.isExpanded(failed))
        XCTAssertFalse(a.isExpanded(queued))
        // A new session on the same device remembers.
        let b = MonitorModel(preferences: prefs)
        XCTAssertTrue(b.isExpanded(failed))
        XCTAssertFalse(b.isExpanded(queued))
    }

    func testClickingATileExpandsItsSectionAndScrollsToIt() {
        let m = model()
        m.sidebar = model().sidebar
        let failed = m.sidebar!.sections[1]
        XCTAssertFalse(m.isExpanded(failed))
        m.reveal(state: "failed")
        XCTAssertTrue(m.isExpanded(failed))
        XCTAssertEqual(m.scrollTarget, "failed")
    }

    func testSearchMatchesNameTitleAndBodyEachWithItsOwnCheckbox() {
        let m = model()
        let rows = m.sidebar!.sections[0].rows
        m.search.text = "paging"
        XCTAssertEqual(m.filteredRows(m.sidebar!.sections[0]).map(\.id), [2], "title")
        m.search.text = "ios"
        XCTAssertEqual(m.filteredRows(m.sidebar!.sections[0]).map(\.id), [2], "name (the project)")
        m.search.inName = false
        XCTAssertTrue(m.filteredRows(m.sidebar!.sections[0]).isEmpty, "name unchecked")
        m.search.text = "headers"
        XCTAssertTrue(m.filteredRows(m.sidebar!.sections[0]).isEmpty, "body unchecked")
        m.search.inBody = true
        XCTAssertEqual(m.filteredRows(m.sidebar!.sections[0]).map(\.id), [1], "body")
        m.search.text = ""
        XCTAssertEqual(m.filteredRows(m.sidebar!.sections[0]).count, rows.count)
        XCTAssertEqual(m.visibleCount(m.sidebar!.sections[0]), 2)
    }

    // MARK: text scale

    func testTextScaleStaysInRangeAndIsRemembered() {
        let prefs = MemoryPreferences()
        let m = MonitorModel(preferences: prefs)
        XCTAssertEqual(m.textScale, 1.0)
        for _ in 0..<30 { m.zoomIn() }
        XCTAssertEqual(m.textScale, MonitorModel.scaleRange.upperBound, accuracy: 0.001)
        for _ in 0..<60 { m.zoomOut() }
        XCTAssertEqual(m.textScale, MonitorModel.scaleRange.lowerBound, accuracy: 0.001)
        m.zoomReset()
        XCTAssertEqual(m.textScale, 1.0)
        m.zoomIn(); m.zoomIn()
        XCTAssertEqual(MonitorModel(preferences: prefs).textScale, 1.2, accuracy: 0.001)
    }

    // MARK: updates from the daemon

    func testViewsAndSequenceNumbersArriveFromTheDaemon() {
        let m = MonitorModel()
        var w = Mason_V1_ServerMessage()
        w.welcome = .with { $0.protocol = 1; $0.seq = 9; $0.resume = .snapshot }
        m.apply(w)
        XCTAssertTrue(m.connected)
        var snap = Mason_V1_Snapshot()
        snap.seq = 9
        var hv = Mason_V1_View()
        hv.header = .with { $0.status = "All quiet" }
        snap.views = [hv]
        var sm = Mason_V1_ServerMessage()
        sm.snapshot = snap
        m.apply(sm)
        XCTAssertEqual(m.lastSeq, 9)
        XCTAssertEqual(m.header?.status, "All quiet")
        var ev = Mason_V1_ServerMessage()
        ev.events = .with { $0.events = [.with { $0.seq = 12 }, .with { $0.seq = 13 }] }
        m.apply(ev)
        XCTAssertEqual(m.lastSeq, 13, "what a reconnect will say it last saw")
        // An older number never moves it back.
        var old = Mason_V1_ServerMessage()
        old.view = .with { $0.seq = 5; $0.view = hv }
        m.apply(old)
        XCTAssertEqual(m.lastSeq, 13)
    }

    // MARK: commands (the client sends what the daemon allows; it decides nothing)

    func testCommandsAreBuiltForTheDaemon() {
        let m = MonitorModel()
        var sent: [Mason_V1_Command] = []
        m.sender = { sent.append($0) }
        m.identity = "brian"
        m.cancel(4)
        m.answer(task: 2, "1", cardHash: "abc")
        m.setSessionLimit(3)
        m.skipTest(5, step: "test")
        XCTAssertEqual(sent.count, 4)
        XCTAssertTrue(sent.allSatisfy { $0.actor == "brian" }, "every command says who sent it")
        if case .cancel(let r)? = sent[0].body { XCTAssertEqual(r.task, 4) } else { XCTFail() }
        if case .answer(let a)? = sent[1].body {
            XCTAssertEqual([a.task, UInt64(a.answer == "1" ? 1 : 0)], [2, 1])
            XCTAssertEqual(a.cardHash, "abc", "an answer carries the hash of the card it answers")
        } else { XCTFail() }
        if case .setLimit(let l)? = sent[2].body { XCTAssertEqual(l.resource, "llm:claude"); XCTAssertEqual(l.units, 3) } else { XCTFail() }
        if case .skip(let s)? = sent[3].body {
            XCTAssertEqual(s.task, 5)
            if case .step(let n)? = s.scope { XCTAssertEqual(n, "test") } else { XCTFail() }
        } else { XCTFail() }
    }

    // MARK: what the client shows (the conformance items)

    func testVisibleItemsFollowWhatTheViewsSayAndTheCollapseState() {
        let m = model()
        let ids = Set(m.visibleItems().map(\.id))
        XCTAssertTrue(ids.contains("tile:queued"))
        XCTAssertTrue(ids.contains("row:queued:1") && ids.contains("row:queued:2"))
        XCTAssertFalse(ids.contains("row:failed:3"), "a collapsed section shows no rows")
        m.reveal(state: "failed")
        XCTAssertTrue(m.visibleItems().contains { $0.id == "row:failed:3" && $0.actions == ["cancel", "retry"] })
        let section = m.visibleItems().first { $0.id == "section:failed" }!
        XCTAssertEqual(section.texts.last, "expanded")
    }

    func testDurationsAreAlwaysShown() {
        XCTAssertEqual(Format.duration(ms: 0), "0 s")
        XCTAssertEqual(Format.duration(ms: 42_000), "42 s")
        XCTAssertEqual(Format.duration(ms: 9 * 60_000), "9 min")
        XCTAssertEqual(Format.duration(ms: 3 * 3_600_000 + 10 * 60_000), "3h10")
        XCTAssertEqual(Format.duration(ms: 2 * 3_600_000), "2h")
    }

    func testTheHealthPageReadsTheDaemonsResourceReport() {
        let h = HealthSnapshot(json: ##"{"resources":[{"name":"compile","kind":"machine","used":1,"limit":2}],"waiting":[{"label":"#4","explain":"Waiting for compile slot"}],"holders":[{"label":"#3"}]}"##)
        XCTAssertEqual(h.resources, [.init(name: "compile", kind: "machine", used: 1, limit: 2)])
        XCTAssertEqual(h.waiting, ["#4: Waiting for compile slot"])
        XCTAssertEqual(HealthSnapshot(json: "not json").resources, [])
    }

    // ---- versions (Part 15) ----

    private func welcome(version: String = "dev", minProtocol: UInt32 = 1) -> Mason_V1_ServerMessage {
        var w = Mason_V1_Welcome()
        w.protocol = 1
        w.resume = .upToDate
        w.version = version
        w.minProtocol = minProtocol
        var m = Mason_V1_ServerMessage()
        m.welcome = w
        return m
    }

    func testAMonitorOffersToUpdateOnlyWhenBothBuildsAreRealAndDiffer() {
        let m = MonitorModel()
        m.ownVersion = "aaa111"
        m.apply(welcome(version: "bbb222"))
        XCTAssertTrue(m.updateAvailable)
        m.apply(welcome(version: "aaa111"))
        XCTAssertFalse(m.updateAvailable, "the same build")
        m.apply(welcome(version: "dev"))
        XCTAssertFalse(m.updateAvailable, "a daemon run from a build directory has no version to compare")
        m.ownVersion = "dev"
        m.apply(welcome(version: "bbb222"))
        XCTAssertFalse(m.updateAvailable, "a development monitor does not nag")
    }

    func testAMonitorTooOldForTheProtocolIsReadOnlyAndSaysWhy() {
        let m = MonitorModel()
        var sent = 0
        m.sender = { _ in sent += 1 }
        m.apply(welcome(minProtocol: 1))
        m.cancel(3)
        XCTAssertEqual(sent, 1)
        XCTAssertNil(m.readOnlyReason)
        m.apply(welcome(minProtocol: 2))
        XCTAssertNotNil(m.readOnlyReason)
        XCTAssertTrue(m.readOnlyReason!.contains("protocol 1") && m.readOnlyReason!.contains("2 or newer"))
        m.cancel(3)
        m.answer(task: 1, "yes", cardHash: "x")
        XCTAssertEqual(sent, 1, "nothing is sent while read-only")
        XCTAssertTrue(m.lastResult?.hasPrefix("Read-only:") ?? false)
        // A daemon that serves it again lifts the restriction.
        m.apply(welcome(minProtocol: 1))
        XCTAssertNil(m.readOnlyReason)
        m.cancel(3)
        XCTAssertEqual(sent, 2)
    }

    func testTheUpdateButtonKnowsHowItWillGetANewerMonitor() {
        XCTAssertEqual(ClientUpdate.plan(env: ["MASON_SOURCE_DIR": "/src"], exists: { $0 == "/src/scripts/bundle-monitor.sh" }), .buildFromSource("/src"))
        if case .unavailable(let why) = ClientUpdate.plan(env: ["MASON_SOURCE_DIR": "/nope"], exists: { _ in false }) {
            XCTAssertTrue(why.contains("/nope"))
        } else { XCTFail("a checkout without the script is not a source") }
        XCTAssertEqual(ClientUpdate.plan(env: ["MASON_INSTALL_URL": "https://example.org/mason"]), .openPage("https://example.org/mason"))
        if case .unavailable = ClientUpdate.plan(env: ["MASON_INSTALL_URL": "http://insecure"]) {} else { XCTFail("only https pages") }
        if case .unavailable(let why) = ClientUpdate.plan(env: [:]) { XCTAssertTrue(why.contains("MASON_SOURCE_DIR")) } else { XCTFail() }
    }
}
