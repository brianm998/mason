// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import mason.v1.*
import kotlin.test.*

class MonitorCoreTest {
    // ---- fixtures ----

    private fun row(id: Long, state: String, title: String = "t", project: String = "web", body: String = "", actions: List<String> = emptyList()) =
        TaskRow(id = id, key = "#$id", title = title, project = project, state = state, body = body, allowed_actions = actions)

    private fun section(state: String, collapsed: Boolean = false, rows: List<TaskRow> = emptyList()) =
        SidebarSection(state = state, title = state.replaceFirstChar { it.uppercase() }, count = rows.size, collapsed = collapsed, rows = rows)

    private fun model(connected: Boolean = true, prefs: Preferences = MemoryPreferences()): MonitorModel {
        val m = MonitorModel(prefs)
        m.header = HeaderView(status = "1 needs you", attention = 1, counts = listOf(StateCount(state = "queued", count = 4)))
        m.sidebar = SidebarView(
            sections = listOf(
                section("queued", rows = listOf(row(1, "queued", "Rate limits", body = "add headers"), row(2, "queued", "Paging", project = "ios"))),
                section("failed", collapsed = true, rows = listOf(row(3, "failed", actions = listOf("retry", "cancel")))),
            ),
        )
        m.connected = connected
        return m
    }

    // ---- navigation ----

    @Test fun historyIsBrowserStyle() {
        var h = NavHistory()
        assertFalse(h.canGoBack || h.canGoForward)
        h = h.go(Page.Task(1)).go(Page.Landing)
        assertEquals(Page.Landing, h.current)
        h = h.goBack()
        assertEquals(Page.Task(1), h.current)
        assertTrue(h.canGoForward)
        h = h.goForward()
        assertEquals(Page.Landing, h.current)
        // Selecting something new clears what was gone forward to.
        h = h.goBack().go(Page.Usage)
        assertFalse(h.canGoForward)
        // Home clears the selection and is itself a step.
        h = h.goHome()
        assertEquals(Page.Home, h.current)
        assertEquals(Page.Usage, h.goBack().current)
        // Going to where you already are changes nothing.
        assertEquals(h, h.go(h.current))
    }

    @Test fun deepLinksRoundTripAndRefuseOtherThings() {
        for (p in listOf(Page.Home, Page.Task(204), Page.Landing, Page.Usage, Page.Pipeline, Page.Health)) {
            assertEquals(p, Page.from(p.url), "$p")
        }
        assertNull(Page.from("https://example.com/task/1"))
        assertNull(Page.from("mason://task/not-a-number"))
        assertNull(Page.from("mason://task"))
        assertNull(Page.from("mason://landing/extra"))
        assertNull(Page.from("not a url at all"))
        val m = model()
        m.open("mason://task/7")
        assertEquals(Page.Task(7), m.page)
        m.open("mason://nonsense")
        assertEquals(Page.Task(7), m.page, "an unknown link changes nothing")
    }

    // ---- counts while disconnected ----

    @Test fun tilesShowADashWhileDisconnectedNeverAStaleCountOrAFalseZero() {
        val m = model()
        val c = m.header!!.counts[0]
        assertEquals("4", m.tileCount(c))
        assertEquals(1, m.attention)
        m.disconnected()
        assertEquals("–", m.tileCount(c))
        assertEquals(0, m.attention, "no attention indicator on a stale count")
        assertEquals("–", m.visibleItems().first { it.id == "tile:queued" }.texts.first())
    }

    // ---- sidebar ----

    @Test fun historySectionsStartCollapsedAndChoicesAreRemembered() {
        val prefs = MemoryPreferences()
        val a = model(prefs = prefs)
        val queued = a.sidebar!!.sections[0]
        val failed = a.sidebar!!.sections[1]
        assertFalse(a.isExpanded(failed), "failed starts collapsed")
        assertTrue(a.isExpanded(queued))
        a.toggle(failed); a.toggle(queued)
        assertTrue(a.isExpanded(failed))
        assertFalse(a.isExpanded(queued))
        val b = MonitorModel(prefs)
        assertTrue(b.isExpanded(failed))
        assertFalse(b.isExpanded(queued))
    }

    @Test fun clickingATileExpandsItsSectionAndScrollsToIt() {
        val m = model()
        assertFalse(m.isExpanded(m.sidebar!!.sections[1]))
        m.reveal("failed")
        assertTrue(m.isExpanded(m.sidebar!!.sections[1]))
        assertEquals("failed", m.scrollTarget)
    }

    @Test fun searchMatchesNameTitleAndBodyEachWithItsOwnCheckbox() {
        val m = model()
        val sec = m.sidebar!!.sections[0]
        m.search = SearchState(text = "paging")
        assertEquals(listOf(2L), m.filteredRows(sec).map { it.id }, "title")
        m.search = SearchState(text = "ios")
        assertEquals(listOf(2L), m.filteredRows(sec).map { it.id }, "name (the project)")
        m.search = m.search.copy(inName = false)
        assertTrue(m.filteredRows(sec).isEmpty(), "name unchecked")
        m.search = m.search.copy(text = "headers")
        assertTrue(m.filteredRows(sec).isEmpty(), "body unchecked")
        m.search = m.search.copy(inBody = true)
        assertEquals(listOf(1L), m.filteredRows(sec).map { it.id }, "body")
        m.search = SearchState()
        assertEquals(2, m.filteredRows(sec).size)
        assertEquals(2, m.visibleCount(sec))
    }

    // ---- text scale ----

    @Test fun textScaleStaysInRangeAndIsRemembered() {
        val prefs = MemoryPreferences()
        val m = MonitorModel(prefs)
        assertEquals(1.0, m.textScale)
        repeat(30) { m.zoomIn() }
        assertEquals(MonitorModel.SCALE_MAX, m.textScale, 0.001)
        repeat(60) { m.zoomOut() }
        assertEquals(MonitorModel.SCALE_MIN, m.textScale, 0.001)
        m.zoomReset()
        assertEquals(1.0, m.textScale)
        m.zoomIn(); m.zoomIn()
        assertEquals(1.2, MonitorModel(prefs).textScale, 0.001)
    }

    // ---- updates from the daemon ----

    @Test fun viewsAndSequenceNumbersArriveFromTheDaemon() {
        val m = MonitorModel()
        m.apply(ServerMessage(welcome = Welcome(protocol = 1, seq = 9, resume = ResumeKind.SNAPSHOT)))
        assertTrue(m.connected)
        val v = View(header_ = HeaderView(status = "All quiet"))
        m.apply(ServerMessage(snapshot = Snapshot(seq = 9, views = listOf(v))))
        assertEquals(9, m.lastSeq)
        assertEquals("All quiet", m.header?.status)
        m.apply(ServerMessage(events = Events(events = listOf(EventRecord(seq = 12), EventRecord(seq = 13)))))
        assertEquals(13, m.lastSeq, "what a reconnect will say it last saw")
        m.apply(ServerMessage(view = ViewUpdate(seq = 5, view = v)))
        assertEquals(13, m.lastSeq, "an older number never moves it back")
    }

    @Test fun theProtocolsJsonMappingDecodesWhatTheDaemonSends() {
        // proto3 JSON as `masond` writes it: camelCase names, enums by name, 64-bit as strings,
        // defaults omitted, and a field this client has never heard of.
        val text = """{"snapshot":{"seq":"9","views":[{"sidebar":{"sections":[{"state":"queued","title":"Queued","count":1,
            "rows":[{"id":"7","key":"#7","title":"Paging","allowedActions":["cancel"],"whyWaiting":"Waiting for compile slot",
            "progress":[{"step":"test","state":"HATCHED","label":"compile"}],"fieldFromTheFuture":true}]}]}}]}}"""
        val m = Codec.decode(text)
        assertNotNull(m)
        val r = m.snapshot!!.views[0].sidebar!!.sections[0].rows[0]
        assertEquals(7L, r.id)
        assertEquals(listOf("cancel"), r.allowed_actions)
        assertEquals("Waiting for compile slot", r.why_waiting)
        assertEquals(CellState.HATCHED, r.progress[0].state)
        assertEquals(9L, m.snapshot!!.seq)
        assertNull(Codec.decode("not json"))
    }

    @Test fun commandsAreEncodedTheWayTheDaemonReadsThem() {
        val json = Codec.encode(ClientMessage(command = Command(
            id = "m1", answer = Answer(task = 2, answer = "1", card_hash = "abc"), actor = "brian")))
        assertTrue("\"answer\"" in json && "\"cardHash\":\"abc\"" in json || "\"card_hash\":\"abc\"" in json, json)
        assertTrue("\"actor\":\"brian\"" in json, json)
        val hello = Codec.encode(ClientMessage(hello = Hello(protocol = 1, last_seq = 41, client = "x")))
        assertTrue("\"lastSeq\":\"41\"" in hello || "\"last_seq\":\"41\"" in hello || "\"lastSeq\":41" in hello, hello)
    }

    // ---- commands (the client sends what the daemon allows; it decides nothing) ----

    @Test fun commandsAreBuiltForTheDaemonWithTheOwnersIdentity() {
        val m = MonitorModel()
        m.identity = "brian"
        val sent = mutableListOf<Command>()
        m.sender = { sent += it }
        m.cancel(4)
        m.answer(2, "1", "abc")
        m.setSessionLimit(3)
        m.skipTest(5, "test")
        assertEquals(4, sent.size)
        assertTrue(sent.all { it.actor == "brian" }, "every command says who sent it")
        assertEquals(4L, sent[0].cancel?.task)
        assertEquals("abc", sent[1].answer?.card_hash, "an answer carries the hash of the card it answers")
        assertEquals("llm:claude", sent[2].set_limit?.resource)
        assertEquals(3, sent[2].set_limit?.units)
        assertEquals("test", sent[3].skip?.step)
    }

    // ---- what the client shows (the conformance items) ----

    @Test fun visibleItemsFollowWhatTheViewsSayAndTheCollapseState() {
        val m = model()
        val ids = m.visibleItems().map { it.id }.toSet()
        assertTrue("tile:queued" in ids && "row:queued:1" in ids && "row:queued:2" in ids)
        assertFalse("row:failed:3" in ids, "a collapsed section shows no rows")
        m.reveal("failed")
        val item = m.visibleItems().first { it.id == "row:failed:3" }
        assertEquals(listOf("retry", "cancel"), item.actions.let { listOf("retry", "cancel").filter { a -> a in it } })
        assertEquals("expanded", m.visibleItems().first { it.id == "section:failed" }.texts.last())
    }

    @Test fun durationsAreAlwaysShown() {
        assertEquals("0 s", Format.duration(0))
        assertEquals("42 s", Format.duration(42_000))
        assertEquals("9 min", Format.duration(9 * 60_000))
        assertEquals("3h10", Format.duration(3 * 3_600_000 + 10 * 60_000))
        assertEquals("2h", Format.duration(2 * 3_600_000))
    }

    @Test fun theHealthPageReadsTheDaemonsResourceReport() {
        val h = HealthSnapshot.parse("""{"resources":[{"name":"compile","kind":"machine","used":1,"limit":2}],"waiting":[{"label":"#4","explain":"Waiting for compile slot"}],"holders":[{"label":"#3"}]}""")
        assertEquals(listOf(HealthSnapshot.Resource("compile", "machine", 1, 2)), h.resources)
        assertEquals(listOf("#4: Waiting for compile slot"), h.waiting)
        assertEquals(emptyList(), HealthSnapshot.parse("not json").resources)
    }
}
