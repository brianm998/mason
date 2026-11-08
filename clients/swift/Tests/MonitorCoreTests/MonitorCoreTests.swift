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

    // MARK: the attention sound

    private func sidebarUpdate(_ sections: [SidebarSection]) -> Mason_V1_View {
        var sb = SidebarView()
        sb.sections = sections
        var v = Mason_V1_View()
        v.body = .sidebar(sb)
        return v
    }

    func testSomethingNewNeedingYouChimesOnceAndTheFirstUpdateIsSilent() {
        let m = MonitorModel()
        var chimes = 0
        m.onAttention = { chimes += 1 }
        let waiting = section("needs_attention", rows: [row(1, state: "developing")])
        m.apply(sidebarUpdate([waiting]))
        XCTAssertEqual(chimes, 0, "what was already waiting when the monitor opened is silent")
        m.apply(sidebarUpdate([waiting]))
        XCTAssertEqual(chimes, 0, "the same task again is not new")
        m.apply(sidebarUpdate([section("needs_attention", rows: [row(1, state: "developing"), row(2, state: "testing")])]))
        XCTAssertEqual(chimes, 1, "a second task needs you")
        m.apply(sidebarUpdate([section("failed", rows: [row(3, state: "failed")])]))
        XCTAssertEqual(chimes, 2, "a failure needs you")
        m.apply(sidebarUpdate([section("queued", rows: [row(4, state: "queued")])]))
        XCTAssertEqual(chimes, 2, "a queued task does not")
    }

    func testMutedIsSilentAndChangingProjectDoesNotChime() {
        let m = MonitorModel()
        var chimes = 0
        m.onAttention = { chimes += 1 }
        m.apply(sidebarUpdate([section("needs_attention", rows: [])]))
        m.muted = true
        m.apply(sidebarUpdate([section("needs_attention", rows: [row(1, state: "developing")])]))
        XCTAssertEqual(chimes, 0, "muted")
        m.muted = false
        m.apply(sidebarUpdate([section("needs_attention", rows: [row(1, state: "developing")])]))
        XCTAssertEqual(chimes, 0, "it was seen while muted, so it is not new when unmuted")
        m.project = "ios"
        m.apply(sidebarUpdate([section("needs_attention", rows: [row(9, state: "developing")])]))
        XCTAssertEqual(chimes, 0, "a different project's list is a fresh start")
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

    // ---- the pipeline editor (Part 18) ----

    private func editorView() -> Mason_V1_EditorView {
        var e = Mason_V1_EditorView()
        e.project = "web"
        e.lanes = [("developing", "Developing"), ("testing", "Testing")].map { c, t in .with { $0.category = c; $0.title = t } }
        e.nodes = [
            .with { $0.name = "develop"; $0.kind = "agent"; $0.category = "developing"; $0.isStart = true; $0.summary = "deep" },
            .with { $0.name = "test"; $0.kind = "test"; $0.category = "testing"; $0.summary = "unit"; $0.problems = ["needs a bound"] },
        ]
        e.edges = [
            .with { $0.from = "develop"; $0.to = "test"; $0.kind = "next" },
            .with { $0.from = "test"; $0.to = "develop"; $0.kind = "on_red"; $0.bound = 2 },
        ]
        e.grid = .with { $0.columns = ["development", "test_step"]; $0.rows = [.with { $0.suite = "unit"; $0.level = "unit"; $0.cells = [false, true] }] }
        e.problems = ["suite.x: odd"]
        e.dirty = true
        e.canUndo = true
        e.changes = ["adds step `lint`"]
        return e
    }

    func testTheEditorsItemsAreWhatTheViewSays() {
        let items = Dictionary(uniqueKeysWithValues: editorView().items().map { ($0.id, $0) })
        XCTAssertEqual(items["node:develop"]?.texts, ["develop", "agent", "developing", "deep", "start"])
        XCTAssertEqual(items["node:test"]?.texts, ["test", "test", "testing", "unit", "needs a bound"])
        XCTAssertEqual(items["edge:test:on_red:develop"]?.texts, ["test", "develop", "on red", "at most 2 times"])
        XCTAssertEqual(items["edge:develop:next:test"]?.texts, ["develop", "test", "next"])
        XCTAssertEqual(items["grid:unit"]?.texts, ["unit", "unit", "test step"])
        XCTAssertEqual(items["problem:0"]?.texts, ["suite.x: odd"])
        XCTAssertEqual(items["lane:testing"]?.texts, ["Testing"])
        XCTAssertEqual(items["draft"]?.actions, ["apply", "discard", "simulate", "undo"].sorted())
        XCTAssertEqual(items["change:0"]?.texts, ["adds step `lint`"])
        var clean = editorView(); clean.dirty = false; clean.canUndo = false; clean.canRedo = true
        XCTAssertEqual(clean.items().first { $0.id == "draft" }?.actions, ["redo", "simulate"])
    }

    func testEditsAreSentAsTheOperationsTheDaemonTakes() {
        let m = MonitorModel()
        var sent: [Mason_V1_Command] = []
        m.sender = { sent.append($0) }
        m.project = "web"
        m.editorOp(["op": "move", "step": "develop", "x": 10, "y": 20])
        m.editorCall("undo")
        XCTAssertEqual(sent.map { $0.raw.method }, ["POST", "POST"])
        XCTAssertEqual(sent.map { $0.raw.path }, ["/projects/web/editor/op", "/projects/web/editor/undo"])
        let body = try! JSONSerialization.jsonObject(with: Data(sent[0].raw.json.utf8)) as! [String: Any]
        XCTAssertEqual(body["op"] as? String, "move")
        XCTAssertEqual(body["x"] as? Int, 10)
        // Selecting a step re-subscribes (so the inspector arrives).
        var subscribed = 0
        m.subscribe = { subscribed += 1 }
        m.selectStep("test")
        XCTAssertEqual(m.editorStep, "test")
        XCTAssertEqual(subscribed, 1)
    }

    // MARK: answers and approvals by clicking (Stage 20)

    func card(hash: String = "h1") -> Mason_V1_DecisionCard {
        .with {
            $0.ask = "One hour or one day?"
            $0.why = "The ticket is silent."
            $0.options = [.with { $0.label = "One hour"; $0.recommended = true }, .with { $0.label = "One day" }]
            $0.evidence = ["the ticket says tokens expire"]
            $0.hash = hash
        }
    }

    func ownerModel(owner: Bool = true) -> (MonitorModel, () -> [Mason_V1_Command]) {
        let m = MonitorModel()
        var sent: [Mason_V1_Command] = []
        m.sender = { sent.append($0) }
        m.identity = "brian"
        var w = Mason_V1_ServerMessage()
        w.welcome = .with { $0.protocol = 1; $0.owner = owner }
        m.apply(w)
        return (m, { sent })
    }

    func testAnsweringByOptionSendsTheNumberAndTheCardsHash() {
        let (m, sent) = ownerModel()
        m.answer(task: 2, "1", cardHash: card().hash)
        guard case .answer(let a)? = sent()[0].body else { return XCTFail() }
        XCTAssertEqual([a.answer, a.cardHash], ["1", "h1"])
        XCTAssertEqual(sent()[0].actor, "brian", "the owner's identity goes with it")
    }

    func testOtherTextIsSentAsFreeTextTrimmedAndNeverWhenBlank() {
        let (m, sent) = ownerModel()
        XCTAssertNil(otherAnswer("  \n "))
        let text = otherAnswer("  use two hours\nplease \n")!
        m.answer(task: 2, text, cardHash: "h1")
        guard case .answer(let a)? = sent()[0].body else { return XCTFail() }
        XCTAssertEqual(a.answer, "use two hours\nplease")
        XCTAssertEqual(a.cardHash, "h1")
    }

    func testAStaleCardHashIsSentAsItWasAndTheRefusalIsShown() {
        let (m, sent) = ownerModel()
        // The card on screen is the old one; the hash goes as the card said, and the daemon decides.
        m.answer(task: 2, "1", cardHash: card(hash: "old").hash)
        guard case .answer(let a)? = sent()[0].body else { return XCTFail() }
        XCTAssertEqual(a.cardHash, "old")
        var r = Mason_V1_ServerMessage()
        r.result = .with { $0.id = "m1"; $0.ok = false; $0.status = 409; $0.error = "stale: that card has changed" }
        m.apply(r)
        XCTAssertEqual(m.lastResult, "stale: that card has changed", "the card shows the refusal")
    }

    func testArrowsMoveTheSelectionAndReturnPicksIt() {
        var s = OptionSelection(count: 3, recommended: 1)
        XCTAssertEqual(s.answer, "2", "it starts on the recommended option")
        s.down(); XCTAssertEqual(s.answer, "3")
        s.down(); XCTAssertEqual(s.answer, "3", "it stops at the end")
        s.up(); s.up(); s.up(); XCTAssertEqual(s.answer, "1")
        var none = OptionSelection(count: 2)
        XCTAssertNil(none.answer, "nothing is picked until a key is pressed")
        none.up(); XCTAssertEqual(none.answer, "2")
        XCTAssertEqual(otherBoxKey(shift: false), .send)
        XCTAssertEqual(otherBoxKey(shift: true), .newline)
    }

    func approval(hold: Bool) -> Mason_V1_Approval {
        .with {
            $0.kind = hold ? "hold" : "approval"
            $0.waitingFor = "Your approval of a change to MASON's own rules"
            $0.lines = ["touches crates/testing/src/verdict.rs (verdict logic)"]
            $0.diffHash = hold ? "abc123" : ""
            $0.branch = "mason/scratch/t3"
            $0.diffCommand = "git diff main...mason/scratch/t3"
        }
    }

    func testApproveSendsTheDiffHashShownForAHoldAndAPlainApproveOtherwise() {
        let (m, sent) = ownerModel()
        m.approve(3, approval(hold: true))
        m.approve(4, approval(hold: false))
        guard case .approveHold(let h)? = sent()[0].body else { return XCTFail() }
        XCTAssertEqual([h.task, UInt64(h.diffHash == "abc123" ? 1 : 0)], [3, 1])
        guard case .approve(let p)? = sent()[1].body else { return XCTFail() }
        XCTAssertEqual(p.task, 4)
    }

    func testRejectSendsTheReason() {
        let (m, sent) = ownerModel()
        m.reject(3, reason: "too broad")
        guard case .reject(let r)? = sent()[0].body else { return XCTFail() }
        XCTAssertEqual([r.task, UInt64(r.reason == "too broad" ? 1 : 0)], [3, 1])
    }

    func homeWith(card c: Mason_V1_DecisionCard?, approval a: Mason_V1_Approval?) -> Mason_V1_ServerMessage {
        var item = Mason_V1_AttentionItem()
        item.task = row(3, state: "needs_attention")
        if let c { item.card = c }
        if let a { item.approval = a }
        var v = Mason_V1_View()
        v.home = .with { $0.attention = [item] }
        var msg = Mason_V1_ServerMessage()
        msg.view = .with { $0.seq = 1; $0.view = v }
        return msg
    }

    func testTheOwnerSeesTheButtonsAndANonOwnerSeesTheCardAndNone() {
        for owner in [true, false] {
            let (m, _) = ownerModel(owner: owner)
            m.apply(homeWith(card: card(), approval: approval(hold: true)))
            let items = m.visibleItems()
            let c = items.first { $0.id == "card:3" }!
            let a = items.first { $0.id == "approval:3" }!
            XCTAssertTrue(c.texts.contains("1. One hour (recommended)") && c.texts.contains("the ticket says tokens expire"))
            XCTAssertTrue(a.texts.contains("diff abc123") && a.texts.contains("git diff main...mason/scratch/t3"))
            XCTAssertEqual(c.actions, owner ? ["option:1", "option:2", "other"] : [])
            XCTAssertEqual(a.actions, owner ? ["approve", "reject"] : [])
            XCTAssertEqual(m.canAct, owner)
        }
    }

    func testClickingTheAttentionCountOpensTheFirstTaskThatNeedsYou() {
        let (m, _) = ownerModel()
        m.openFirstNeedingYou()
        XCTAssertEqual(m.page, .home, "nothing to open: stay on home")
        var h = Mason_V1_ServerMessage()
        h.view = .with { $0.seq = 2; $0.view = .with { $0.header = .with { $0.attention = 1 } } }
        m.apply(h)
        m.apply(homeWith(card: card(), approval: nil))
        m.openFirstNeedingYou()
        XCTAssertEqual(m.page, .task(3))
    }

    // MARK: graph layout

    private let masonEdges: [GraphLayout.BackEdge] = [
        .init(fromX: 440, toX: 240, labeled: true), .init(fromX: 640, toX: 240, labeled: true)]

    func testGraphNothingClippedLeft() {
        let h = GraphLayout.horizontal(nodeXs: [40, 240, 440, 640], backEdges: masonEdges, viewport: 0)
        XCTAssertGreaterThanOrEqual(h.left + h.dx, 0)
        XCTAssertGreaterThanOrEqual(h.right + h.dx + GraphLayout.pad, 0)
        XCTAssertLessThanOrEqual(h.right + h.dx, h.width)
    }

    func testGraphCenteredWhenViewportWider() {
        let h = GraphLayout.horizontal(nodeXs: [40, 240, 440, 640], backEdges: masonEdges, viewport: 2000)
        XCTAssertEqual(h.width, 2000)
        XCTAssertEqual(h.left + h.dx, h.width - (h.right + h.dx), accuracy: 0.5)
    }

    func testGraphWiderThanViewportKeepsPad() {
        let h = GraphLayout.horizontal(nodeXs: [0, 2000], backEdges: [.init(fromX: 2000, toX: 0, labeled: true)], viewport: 500)
        XCTAssertEqual(h.dx, GraphLayout.pad - h.left)
        XCTAssertEqual(h.left + h.dx, GraphLayout.pad)
    }

    func testGraphNegativeLayoutX() {
        let h = GraphLayout.horizontal(nodeXs: [-300, 100], backEdges: [], viewport: 0)
        XCTAssertGreaterThanOrEqual(h.left + h.dx, 0)
    }

    // MARK: reconnecting after a daemon restart

    func testReconnectPauseSchedule() {
        var p = ReconnectPolicy()
        XCTAssertEqual((0..<7).map { _ in p.nextPause() }, [1, 2, 4, 8, 10, 10, 10])
        p.reset()
        XCTAssertEqual(p.nextPause(), 1)
    }

    func testDaemonInfoReplacesOnPortPidOrStartTime() {
        let a = DaemonInfo(port: 7423, pid: 5, startedMs: 100)
        XCTAssertFalse(DaemonInfo(port: 7423, pid: 5, startedMs: 100).replaces(a))
        XCTAssertTrue(DaemonInfo(port: 7424, pid: 5, startedMs: 100).replaces(a))
        XCTAssertTrue(DaemonInfo(port: 7423, pid: 6, startedMs: 100).replaces(a))
        XCTAssertTrue(DaemonInfo(port: 7423, pid: 5, startedMs: 101).replaces(a))
    }

    func testDiscoveryReadsPidAndStartTime() throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("disc-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        try Data(#"{"pid": 41, "port": 7423, "started_ms": 99}"#.utf8).write(to: dir.appendingPathComponent("daemon.json"))
        let got = Discovery.info(arguments: ["--data-dir", dir.path], environment: [:])
        XCTAssertEqual(got, DaemonInfo(port: 7423, pid: 41, startedMs: 99))
        XCTAssertEqual(Discovery.info(arguments: ["--port", "9"], environment: [:]), DaemonInfo(port: 9))
    }

    @MainActor
    final class FakeSocket: MonitorSocket {
        var port: UInt16 = 0
        var sent: [String] = []
        var inbox: [String] = []
        var waiter: CheckedContinuation<String, Error>?
        var closed = false
        func open(port: UInt16) { self.port = port }
        func send(_ text: String) async throws {
            if closed { throw URLError(.networkConnectionLost) }
            sent.append(text)
        }
        func receive() async throws -> String {
            if closed { throw URLError(.networkConnectionLost) }
            if !inbox.isEmpty { return inbox.removeFirst() }
            return try await withCheckedThrowingContinuation { waiter = $0 }
        }
        func push(_ text: String) {
            if let w = waiter { waiter = nil; w.resume(returning: text) } else { inbox.append(text) }
        }
        func close() {
            closed = true
            waiter?.resume(throwing: URLError(.networkConnectionLost))
            waiter = nil
        }
    }

    func welcomeJSON() throws -> String {
        var w = Mason_V1_Welcome()
        w.version = "test"
        var m = Mason_V1_ServerMessage()
        m.welcome = w
        return try m.jsonString()
    }

    func waitUntil(_ what: String, _ f: @escaping () -> Bool) async {
        for _ in 0..<300 {
            if f() { return }
            try? await Task.sleep(nanoseconds: 10_000_000)
        }
        XCTFail("timed out waiting for \(what)")
    }

    func connection(_ model: MonitorModel, info: @escaping () -> DaemonInfo?, sockets: Box<[FakeSocket]>, stallAfter: TimeInterval = 15) -> MonitorConnection {
        var t = ConnectionTiming()
        t.policy = ReconnectPolicy(scale: 0.01)
        t.watchEvery = 0.02
        t.pingEvery = 0.05
        t.stallAfter = stallAfter
        return MonitorConnection(model: model, discover: info, timing: t, makeSocket: {
            let s = FakeSocket()
            sockets.value.append(s)
            return s
        })
    }

    final class Box<T> { var value: T; init(_ v: T) { value = v } }

    func testDropThenChangedDaemonJsonReconnectsWithoutARelaunch() async throws {
        let model = MonitorModel()
        let sockets = Box<[FakeSocket]>([])
        var current = DaemonInfo(port: 7423, pid: 1, startedMs: 10)
        let c = connection(model, info: { current }, sockets: sockets)
        c.start()
        await waitUntil("the first socket") { sockets.value.count == 1 }
        sockets.value[0].push(try welcomeJSON())
        await waitUntil("connected") { model.connected }
        model.lastSeq = 7
        // The daemon restarts on another port: the file changes, the socket drops.
        current = DaemonInfo(port: 9000, pid: 2, startedMs: 20)
        sockets.value[0].close()
        await waitUntil("a second socket") { sockets.value.count == 2 }
        XCTAssertFalse(model.connected)
        XCTAssertTrue(model.reconnecting, "shown as reconnecting, not an error")
        XCTAssertEqual(sockets.value[1].port, 9000)
        await waitUntil("hello") { !sockets.value[1].sent.isEmpty }
        XCTAssertTrue(sockets.value[1].sent[0].contains("\"lastSeq\":\"7\""), "resumes from the last sequence number: \(sockets.value[1].sent[0])")
        sockets.value[1].push(try welcomeJSON())
        await waitUntil("connected again") { model.connected }
        XCTAssertFalse(model.reconnecting)
        c.stop()
    }

    func testAChangedPidWhileConnectedReconnectsFromTheLastSeq() async throws {
        let model = MonitorModel()
        let sockets = Box<[FakeSocket]>([])
        var current = DaemonInfo(port: 7423, pid: 1, startedMs: 10)
        let c = connection(model, info: { current }, sockets: sockets)
        c.start()
        await waitUntil("the first socket") { sockets.value.count == 1 }
        sockets.value[0].push(try welcomeJSON())
        await waitUntil("connected") { model.connected }
        model.lastSeq = 12
        current = DaemonInfo(port: 7423, pid: 2, startedMs: 30)  // an update on the same port
        await waitUntil("a second socket") { sockets.value.count == 2 }
        XCTAssertTrue(sockets.value[0].closed)
        await waitUntil("hello") { !sockets.value[1].sent.isEmpty }
        XCTAssertTrue(sockets.value[1].sent[0].contains("\"lastSeq\":\"12\""))
        c.stop()
    }

    func testAStalledDaemonShowsReconnectingNotAnError() async throws {
        let model = MonitorModel()
        let sockets = Box<[FakeSocket]>([])
        let c = connection(model, info: { DaemonInfo(port: 7423, pid: 1, startedMs: 10) }, sockets: sockets, stallAfter: 0.15)
        c.start()
        await waitUntil("the first socket") { sockets.value.count == 1 }
        sockets.value[0].push(try welcomeJSON())
        await waitUntil("connected") { model.connected }
        // Silence: not even a Pong.
        await waitUntil("a second socket") { sockets.value.count == 2 }
        XCTAssertTrue(sockets.value[0].closed)
        XCTAssertTrue(sockets.value[0].sent.contains { $0.contains("ping") }, "it asked before giving up")
        XCTAssertTrue(model.reconnecting)
        XCTAssertNil(model.lastResult, "no error is shown")
        c.stop()
    }

    func testNoDaemonJsonYetKeepsRetrying() async throws {
        let model = MonitorModel()
        let sockets = Box<[FakeSocket]>([])
        var current: DaemonInfo? = nil
        let c = connection(model, info: { current }, sockets: sockets)
        c.start()
        await waitUntil("reconnecting") { model.reconnecting }
        XCTAssertTrue(sockets.value.isEmpty)
        current = DaemonInfo(port: 7423, pid: 1, startedMs: 1)
        await waitUntil("a socket") { sockets.value.count == 1 }
        c.stop()
    }
}
