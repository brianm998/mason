// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import mason.v1.*
import kotlin.test.*

class MonitorCoreTest {
    @Test
    fun timeInStateCountsFromStateSinceAndBodySearchTogglesRowBodies() {
        val r = TaskRow(in_state_ms = 120_000, state_since_ms = 5_000_000)
        val m = MonitorModel()
        m.anchor(5_090_000, local = 1_000_000)
        assertEquals(90_000, r.inState(m.daemonNow(1_000_000)))
        assertEquals(0, r.inState(4_000_000))
        assertEquals(120_000, r.copy(state_since_ms = 0).inState(9_000_000))
        var subscribed = 0
        m.subscribe = { subscribed += 1 }
        m.search = m.search.copy(inBody = true)
        assertEquals(0, subscribed)
        m.search = m.search.copy(text = "x")
        assertEquals(1, subscribed)
        m.search = m.search.copy(text = "xy")
        assertEquals(1, subscribed)
        m.search = m.search.copy(inBody = false)
        assertEquals(2, subscribed)
        assertFalse(m.search.wantsRowBodies)
    }

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

    // ---- mascot choice ----

    @Test
    fun homeMascotFollowsTheDaemonsState() {
        val m = model(connected = false)
        assertEquals("not_connected", m.homeMascot.id)
        m.connected = true
        assertEquals("needs_attention", m.homeMascot.id)
        m.header = m.header!!.copy(attention = 0)
        m.home = HomeView()
        assertEquals(MascotChoice("no_tasks", true), m.homeMascot)
        m.home = HomeView(stages = listOf(section("queued", rows = listOf(row(1, "queued")))))
        assertEquals(MascotChoice("all_clear", false), m.homeMascot)
        m.home = HomeView(active = listOf(ActiveModel(task = 1)))
        assertEquals("developing", m.homeMascot.id)
    }

    @Test
    fun searchWithNoMatchesSaysSo() {
        val m = model()
        assertEquals(false, m.searchMatchesNothing)
        m.search = m.search.copy(text = "rate")
        assertEquals(false, m.searchMatchesNothing)
        m.search = m.search.copy(text = "zzzz")
        assertEquals(true, m.searchMatchesNothing)
        assertEquals("Nothing matched.", SEARCH_NO_MATCH_TEXT)
    }

    // ---- the attention sound ----

    private fun sidebarOf(vararg s: SidebarSection) = View(sidebar = SidebarView(sections = s.toList()))

    @Test fun aStaleViewArrivingAfterANewerOneIsDropped() {
        val m = MonitorModel()
        val newer = section("needs_attention", rows = listOf(row(1, "developing")))
        val older = section("needs_attention", rows = listOf(row(1, "developing"), row(2, "testing")))
        m.apply(ServerMessage(view = ViewUpdate(seq = 8, view = sidebarOf(newer))))
        m.apply(ServerMessage(view = ViewUpdate(seq = 7, view = sidebarOf(older))))
        assertEquals(1, m.sidebar?.sections?.single()?.rows?.size, "the older view did not replace the newer")
        assertEquals(8L, m.lastSeq)
    }

    @Test fun somethingNewNeedingYouChimesOnceAndTheFirstUpdateIsSilent() {
        val m = MonitorModel()
        var chimes = 0
        m.onAttention = { chimes++ }
        val waiting = section("needs_attention", rows = listOf(row(1, "developing")))
        m.apply(sidebarOf(waiting))
        assertEquals(0, chimes, "what was already waiting when the monitor opened is silent")
        m.apply(sidebarOf(waiting))
        assertEquals(0, chimes, "the same task again is not new")
        m.apply(sidebarOf(section("needs_attention", rows = listOf(row(1, "developing"), row(2, "testing")))))
        assertEquals(1, chimes, "a second task needs you")
        m.apply(sidebarOf(section("failed", rows = listOf(row(3, "failed")))))
        assertEquals(2, chimes, "a failure needs you")
        m.apply(sidebarOf(section("queued", rows = listOf(row(4, "queued")))))
        assertEquals(2, chimes, "a queued task does not")
    }

    @Test fun mutedIsSilentAndChangingProjectDoesNotChime() {
        val m = MonitorModel()
        var chimes = 0
        m.onAttention = { chimes++ }
        m.apply(sidebarOf(section("needs_attention")))
        m.toggleMute()
        m.apply(sidebarOf(section("needs_attention", rows = listOf(row(1, "developing")))))
        assertEquals(0, chimes, "muted")
        m.toggleMute()
        m.apply(sidebarOf(section("needs_attention", rows = listOf(row(1, "developing")))))
        assertEquals(0, chimes, "it was seen while muted, so it is not new when unmuted")
        m.project = "ios"
        m.apply(sidebarOf(section("needs_attention", rows = listOf(row(9, "developing")))))
        assertEquals(0, chimes, "a different project's list is a fresh start")
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

    @Test fun backAndForwardRestoreEachEntrysScroll() {
        var h = NavHistory().go(Page.Task(5), 300)
        assertEquals(0, h.restore, "a new selection starts at the top")
        h = h.goBack(40)
        assertEquals(Page.Home, h.current)
        assertEquals(300, h.restore)
        h = h.goForward(300)
        assertEquals(Page.Task(5), h.current)
        assertEquals(40, h.restore)
    }

    @Test fun theSamePageTwiceKeepsTwoPositions() {
        var h = NavHistory().go(Page.Task(5), 100).go(Page.Home, 10).go(Page.Task(9), 700)
        h = h.goBack(1)
        assertEquals(700, h.restore)
        h = h.goBack(2)
        assertEquals(Page.Task(5), h.current)
        assertEquals(10, h.restore)
        h = h.goBack(3)
        assertEquals(Page.Home, h.current)
        assertEquals(100, h.restore)
    }

    @Test fun goHomeFromHomeKeepsItsPositionAndSelectingClearsForward() {
        var h = NavHistory().go(Page.Landing, 5).goBack(9)
        assertEquals(5, h.restore)
        assertEquals(h, h.goHome(77), "going where you already are changes nothing")
        h = h.go(Page.Usage, 77)
        assertFalse(h.canGoForward)
        assertTrue(h.forwardScroll.isEmpty())
        assertEquals(77, h.goBack().restore)
    }

    @Test fun modelSavesTheLiveScrollWhenLeaving() {
        val m = model()
        m.reportScroll(250)
        m.go(Page.Task(3))
        m.goBack()
        assertEquals(250, m.liveScroll)
        assertEquals(250, m.history.restore)
    }

    @Test fun deepLinksRoundTripAndRefuseOtherThings() {
        for (p in listOf(Page.Home, Page.Task(204), Page.Landing, Page.Usage, Page.Pipeline, Page.Health, Page.Settings, Page.Section("cpu"), Page.Section("testing"))) {
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

    @Test fun issuesTheTrackerDidNotImportAreATileAndAListWithTheirReasons() {
        val m = model()
        assertNull(m.visibleItems().firstOrNull { it.id == "tile:not_imported" }, "nothing to say, nothing shown")
        val p = PendingIssue(
            project = "mason", number = 12, title = "Add paging",
            reason = "10 self-tasks are open and the limit is 10", url = "https://github.com/o/r/issues/12",
        )
        m.header = m.header!!.copy(pending_issues = listOf(p))
        val items = m.visibleItems()
        assertEquals(listOf("1", "not imported"), items.first { it.id == "tile:not_imported" }.texts)
        assertEquals(
            listOf("mason", "#12", "Add paging", "10 self-tasks are open and the limit is 10", "https://github.com/o/r/issues/12"),
            items.first { it.id == "pending:mason:12" }.texts,
        )
    }

    @Test fun aProposedImportShowsItsIssueAndOffersApprove() {
        val m = model()
        val r = row(9, "proposed", "Add paging", actions = listOf("approve", "reject", "edit"))
            .copy(issue = 12, issue_url = "https://github.com/o/r/issues/12")
        m.sidebar = m.sidebar!!.copy(sections = m.sidebar!!.sections + section("proposed_imports", rows = listOf(r)))
        val item = m.visibleItems().first { it.id == "row:proposed_imports:9" }
        assertTrue("issue #12" in item.texts)
        assertTrue("https://github.com/o/r/issues/12" in item.texts)
        assertEquals(listOf("approve", "edit", "reject"), item.actions.sorted())
    }

    @Test fun tileLabelIsTheDaemonsLabelOrTheUnderscoreFallback() {
        val m = model()
        val c = StateCount(state = "needs_attention")
        assertEquals("needs attention", m.tileLabel(c))
        assertEquals("stuck", m.tileLabel(c.copy(label = "stuck")))
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

    @Test fun doubleClickOnAStateWithOneTaskExpandsScrollsAndOpensIt() {
        val m = model()
        m.tileClicked("failed", 1)
        assertEquals(Page.Home, m.page)
        m.tileClicked("failed", 2)
        assertTrue(m.isExpanded(m.sidebar!!.sections[1]))
        assertEquals("failed", m.scrollTarget)
        assertEquals(Page.Task(3), m.page)
    }

    @Test fun doubleClickOnFailedWithSeveralOrNoTasks() {
        val m = model()
        m.apply(sidebarOf(section("failed", rows = listOf(row(3, "failed"), row(4, "failed")))))
        m.tileClicked("failed", 2)
        assertEquals("failed", m.scrollTarget)
        assertEquals(Page.Section("failed"), m.page)
        assertEquals("Failed", SectionPages.title("failed"))

        val z = model()
        z.apply(sidebarOf(section("failed", rows = emptyList())))
        z.tileClicked("failed", 2)
        assertEquals(Page.Home, z.page)

        val d = model(connected = false)
        d.tileDoubleClicked("failed")
        assertEquals(Page.Home, d.page)
    }

    @Test fun doubleClickWithSeveralTasksExpandsAndScrollsAndOpensTheStagePage() {
        val m = model()
        m.tileClicked("queued", 1)
        assertEquals(Page.Home, m.page)
        m.tileClicked("queued", 2)
        assertTrue(m.isExpanded(m.sidebar!!.sections[0]))
        assertEquals("queued", m.scrollTarget)
        assertEquals(Page.Section("queued"), m.page)
    }

    @Test fun doubleClickOnAStageWithThreeTasksOpensItsPageAndOneTaskOpensTheTask() {
        val m = model()
        m.sidebar = m.sidebar!!.copy(sections = m.sidebar!!.sections + listOf(
            section("testing", rows = listOf(row(10, "testing"), row(11, "testing"), row(12, "testing"))),
            section("review", rows = listOf(row(20, "review"))),
            section("landing", rows = listOf(row(30, "landing"), row(31, "landing"))),
        ))
        m.tileClicked("testing", 2)
        assertEquals(Page.Section("testing"), m.page)
        m.tileClicked("review", 2)
        assertEquals(Page.Task(20), m.page)
        m.tileClicked("landing", 2)
        assertEquals(Page.Landing, m.page)
    }

    @Test fun doubleClickOnStuckOrProposedWithSeveralTasksOpensTheirPage() {
        for (state in listOf("needs_attention", "proposed")) {
            val m = model()
            m.sidebar = m.sidebar!!.copy(sections = m.sidebar!!.sections + section(state, rows = listOf(row(40, state), row(41, state))))
            m.tileClicked(state, 2)
            assertEquals(state, m.scrollTarget)
            assertEquals(Page.Section(state), m.page)
            assertEquals(listOf(40L, 41L), m.sectionRows(state).map { it.id })
            val one = model()
            one.sidebar = one.sidebar!!.copy(sections = one.sidebar!!.sections + section(state, rows = listOf(row(42, state))))
            one.tileClicked(state, 2)
            assertEquals(Page.Task(42), one.page)
        }
        assertEquals("Needs you", SectionPages.title("needs_attention"))
        assertEquals("Proposed", SectionPages.title("proposed"))
    }

    @Test fun sectionUrlsRoundTripToTheirPages() {
        for (name in Page.sectionNames) {
            val page = Page.sectionPage(name)
            assertNotNull(page, name)
            assertEquals(page, Page.from("mason://section/$name"), name)
            assertEquals(page, Page.from(page.url), name)
        }
        assertEquals(Page.Landing, Page.sectionPage("landing"))
        assertEquals("mason://section/testing", Page.Section("testing").url)
        assertNull(Page.from("mason://section/nonsense"))
        assertNull(Page.from("mason://section"))
        var h = NavHistory().go(Page.Section("cpu")).goBack()
        assertEquals(Page.Home, h.current)
        h = h.goForward()
        assertEquals(Page.Section("cpu"), h.current)
    }

    @Test fun testingPageRowsKeepTheSidebarOrderAndSkipSendsTheSidebarCommand() {
        val m = model()
        m.isOwner = true
        val a = row(10, "testing", actions = listOf("skip_test")).copy(
            step = "test",
            progress = listOf(ProgressCell(state = CellState.PART, run = 1, done = 3, total = 8, unit = "suites", total_source = ProgressSource.SOURCE_REPORTED)),
        )
        val b = row(11, "testing", actions = listOf("cancel")).copy(step = "test")
        val c = row(12, "testing", actions = listOf("skip_test")).copy(step = "test")
        m.sidebar = m.sidebar!!.copy(sections = m.sidebar!!.sections + section("testing", rows = listOf(b, a, c)))
        assertEquals(listOf(11L, 10L, 12L), m.sectionRows("testing").map { it.id })
        assertEquals(listOf(10L, 12L), m.skippableRows(m.sectionRows("testing")).map { it.id })
        val sent = mutableListOf<Command>()
        m.sender = { sent += it }
        m.skipAllTests(m.sectionRows("testing"))
        assertEquals(listOf(10L, 12L), sent.map { it.skip?.task })
        // masond answers the first two (task #522: a second click while one is pending sends nothing).
        sent.toList().forEach { m.apply(ServerMessage(result = CommandResult(id = it.id, ok = false, error = "no"))) }
        m.skipTest(10, "test")
        assertEquals(10L, sent.last().skip?.task)
        assertEquals("3 of 8 suites", SectionPages.suiteProgress(a))
        assertEquals("", SectionPages.suiteProgress(b))
    }

    @Test fun skipAllOverRowsWithNoStepSendsOnePerTaskAndSaysSo() {
        val m = model()
        m.isOwner = true
        m.sidebar = m.sidebar!!.copy(sections = m.sidebar!!.sections + section("testing", rows = listOf(
            row(20, "testing", actions = listOf("skip_test")), row(21, "testing", actions = listOf("skip_test")))))
        val sent = mutableListOf<Command>()
        m.sender = { sent += it }
        m.skipAllTests(m.sectionRows("testing"))
        assertEquals(listOf(20L, 21L), sent.map { it.skip?.task })
        assertEquals(listOf("", ""), sent.map { it.skip?.step }, "the daemon reads an empty step as the test step")
        assertEquals("Skipped testing for 2 tasks", m.skipNotice)
    }

    @Test fun skipAllSaysWhyWhenItCannotAct() {
        val m = model()
        m.sidebar = m.sidebar!!.copy(sections = m.sidebar!!.sections + section("testing", rows = listOf(row(20, "testing", actions = listOf("skip_test")))))
        val sent = mutableListOf<Command>()
        m.sender = { sent += it }
        m.isOwner = false
        assertNotNull(m.skipDisabledReason)
        m.skipAllTests(m.sectionRows("testing"))
        assertTrue(sent.isEmpty())
        assertEquals(m.skipDisabledReason, m.skipNotice)
        m.isOwner = true
        m.sidebar = m.sidebar!!.copy(sections = m.sidebar!!.sections.filter { it.state != "testing" } + section("testing", rows = listOf(row(21, "testing", actions = listOf("cancel")))))
        m.skipAllTests(m.sectionRows("testing"))
        assertEquals("Nothing to skip: no task here allows it", m.skipNotice)
    }

    @Test fun aRefusedCommandResultIsAMessage() {
        val m = model()
        m.apply(ServerMessage(result = CommandResult(id = "m1", ok = false, error = "there is no step `x`")))
        assertEquals("there is no step `x`", m.lastResult)
    }

    @Test fun doubleClickOnAnEmptyStateChangesNothingBeyondTheReveal() {
        val m = model()
        m.sidebar = m.sidebar!!.copy(sections = m.sidebar!!.sections + section("canceled", collapsed = true))
        m.tileClicked("canceled", 2)
        assertEquals(Page.Home, m.page)
    }

    @Test fun doubleClickStillOpensTheLoneTaskWhenSearchHidesIt() {
        val m = model()
        m.search = m.search.copy(text = "zzz-no-match")
        m.tileClicked("failed", 2)
        assertEquals(Page.Task(3), m.page)
        assertEquals("zzz-no-match", m.search.text)
    }

    @Test fun doubleClickWhileDisconnectedDoesNothing() {
        val m = model(connected = false)
        m.revealAndOpenIfOne("failed")
        assertEquals(Page.Home, m.page)
        assertEquals(null, m.scrollTarget)
        assertFalse(m.isExpanded(m.sidebar!!.sections[1]))
    }

    @Test fun tooltipMentionsDoubleClickOnlyForACountOfOne() {
        val m = model()
        assertEquals("tip\nDouble-click to open it", m.tileTooltip(StateCount(state = "failed", count = 1, tooltip = "tip")))
        assertEquals("tip", m.tileTooltip(StateCount(state = "failed", count = 2, tooltip = "tip")))
        m.connected = false
        assertEquals("tip", m.tileTooltip(StateCount(state = "failed", count = 1, tooltip = "tip")))
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
        assertEquals("2", m.countText(sec))
    }

    @Test
    fun aCappedSectionSaysSoInItsHeaderAndSearchSaysWhereItLooked() {
        val m = MonitorModel()
        val rows = listOf(TaskRow(id = 9, title = "newest"), TaskRow(id = 8, title = "older"))
        val sec = SidebarSection(state = "landed", title = "Landed", count = 2000, rows = rows)
        assertTrue(m.isCapped(sec))
        assertEquals("2 of 2000", m.countText(sec))
        m.search = SearchState(text = "newest")
        assertEquals("1 in newest 2", m.countText(sec))
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

    @Test fun onlyViewsMoveTheViewSequence() {
        val m = MonitorModel()
        val v = View(header_ = HeaderView(status = "All quiet"))
        m.apply(ServerMessage(events = Events(events = listOf(EventRecord(seq = 12)))))
        assertEquals(12, m.lastSeq)
        assertEquals(0, m.lastViewSeq)
        m.apply(ServerMessage(view = ViewUpdate(seq = 10, view = v)))
        assertEquals(10, m.lastViewSeq)
        m.apply(ServerMessage(view = ViewUpdate(seq = 4, view = v)))
        assertEquals(10, m.lastViewSeq, "a stale view never moves it back")
    }

    @Test fun theLandingViewCarriesOneTrackerLinePerProject() {
        val m = MonitorModel()
        val lines = listOf(
            "mason: tracker off; issues are not imported, add [tracker] to /c/projects/mason.toml",
            "web: tracker stopped: HTTP 404 (/c/projects/web.toml)",
        )
        m.apply(View(landing = LandingView(trackers = lines)))
        assertEquals(lines, m.landing?.trackers)
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
        m.setCompileLimit(5)
        m.setCompileLimit(0)
        assertEquals("compile", sent[4].set_limit?.resource)
        assertEquals(5, sent[4].set_limit?.units)
        assertEquals(1, sent[5].set_limit?.units, "the stepper never asks for zero compiles")
    }

    @Test fun theCpuGateStepperSendsASetGateAndStaysWithinTenToAHundred() {
        val m = MonitorModel()
        val sent = mutableListOf<Command>()
        m.sender = { sent += it }
        m.setCompileCpuGate(70 + 5)
        m.setCompileCpuGate(0)
        m.setCompileCpuGate(500)
        m.setCompileSettle(-5)
        assertEquals("compile_cpu_max", sent[0].set_gate?.name)
        assertEquals(75L, sent[0].set_gate?.value_)
        assertEquals(false, sent[0].set_gate?.reset)
        assertEquals(10L, sent[1].set_gate?.value_, "never below 10")
        assertEquals(100L, sent[2].set_gate?.value_, "never above 100")
        assertEquals("compile_settle_s", sent[3].set_gate?.name)
        assertEquals(0L, sent[3].set_gate?.value_)
    }

    @Test fun aRefusedGateShowsInLastResult() {
        val (m, sent) = ownerModel()
        m.setCompileCpuGate(75)
        assertEquals(75L, sent[0].set_gate!!.value_)
        m.apply(ServerMessage(result = CommandResult(id = "m1", ok = false, status = 403, error = "only the owner can change a compile gate")))
        assertEquals("only the owner can change a compile gate", m.lastResult)
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

    @Test fun theSyspolicydLineShowsTheReadingAndTheLimit() {
        assertNull(Compilation().policyLine, "no gate")
        assertEquals("syspolicyd 640% — holding compiles over 300%", Compilation(syspolicyd_pct = 640, syspolicyd_max_pct = 300).policyLine)
        assertEquals("syspolicyd 20% — compiles held over 300%", Compilation(syspolicyd_pct = 20, syspolicyd_max_pct = 300).policyLine)
        val h = HealthSnapshot.parse("""{"cpu":{"busy_pct":10,"compile_max_pct":50,"syspolicyd_pct":640,"syspolicyd_max_pct":300}}""")
        assertEquals("syspolicyd 640% — holding compiles over 300%", h.policyLine)
        assertNull(HealthSnapshot.parse("""{"cpu":{"busy_pct":10,"compile_max_pct":50}}""").policyLine)
    }

    @Test fun theHealthPageReadsTheDaemonsResourceReport() {
        val h = HealthSnapshot.parse("""{"resources":[{"name":"compile","kind":"machine","used":1,"limit":2}],"waiting":[{"label":"#4","explain":"Waiting for compile slot"}],"holders":[{"label":"#3"}]}""")
        assertEquals(listOf(HealthSnapshot.Resource("compile", "machine", 1, 2)), h.resources)
        assertEquals(listOf("#4: Waiting for compile slot"), h.waiting)
        assertEquals(emptyList(), HealthSnapshot.parse("not json").resources)
        assertEquals("CPU 71% busy; a compile starts below 50%.", HealthSnapshot.parse("""{"cpu":{"busy_pct":71,"compile_max_pct":50}}""").cpuLine)
        assertEquals("CPU not readable; a compile starts below 80%.", HealthSnapshot.parse("""{"cpu":{"busy_pct":null,"compile_max_pct":80}}""").cpuLine)
        assertEquals("CPU 20% busy; a compile starts below 50%.", HealthSnapshot.parse("""{"cpu":{"busy_pct":20,"compile_max_pct":50,"source":"mach","failed_reads":0}}""").cpuLine)
        assertEquals("CPU 100% busy (load average; the CPU reading is missing); a compile starts below 50%.", HealthSnapshot.parse("""{"cpu":{"busy_pct":100,"compile_max_pct":50,"source":"loadavg","failed_reads":4}}""").cpuLine)
        assertEquals("CPU assumed 100% busy (nothing is readable, so new compiles wait); a compile starts below 50%.", HealthSnapshot.parse("""{"cpu":{"busy_pct":100,"compile_max_pct":50,"source":"assumed-busy","failed_reads":4}}""").cpuLine)
    }

    // ---- versions (Part 15) ----

    private fun welcome(version: String = "dev", minProtocol: Int = 1) =
        ServerMessage(welcome = Welcome(protocol = 1, resume = ResumeKind.UP_TO_DATE, version = version, min_protocol = minProtocol))

    private fun owner(role: String = "", owner: Boolean = true) =
        ServerMessage(welcome = Welcome(protocol = 1, resume = ResumeKind.UP_TO_DATE, min_protocol = 1, owner = owner, role = role))

    private fun update(latest: String, phase: String = "", path: String = "", error: String = "", daemon: String = "", canUpdate: Boolean = false, button: String = "", confirm: String = "") =
        ServerMessage(
            view = ViewUpdate(
                view = View(
                    header_ = HeaderView(
                        update = UpdateInfo(
                            daemon_text = daemon,
                            daemon_can_update = canUpdate,
                            daemon_button = button,
                            daemon_confirm = confirm,
                            monitors = listOf(
                                MonitorBuild(kind = "swift", latest = "other"),
                                MonitorBuild(kind = "compose", latest = latest, phase = phase, path = path, error = error),
                            ),
                        ),
                    ),
                ),
            ),
        )

    private fun result(id: String, ok: Boolean, error: String = "") =
        ServerMessage(result = CommandResult(id = id, ok = ok, error = error))

    @Test fun aMonitorOffersToUpdateOnlyWhenItsOwnSourceMoved() {
        val m = MonitorModel()
        m.onInstall = {}
        m.ownVersion = "aaa111"
        m.apply(owner())
        m.apply(update("bbb222"))
        assertTrue(m.updateAvailable)
        m.apply(update("aaa111"))
        assertFalse(m.updateAvailable)
        m.apply(update(""))
        assertFalse(m.updateAvailable)
        m.ownVersion = "dev"
        m.apply(update("bbb222"))
        assertFalse(m.updateAvailable)
    }

    @Test fun onlyTheOwnersDesktopMonitorOnTheSameMachineIsOffered() {
        val m = MonitorModel()
        m.ownVersion = "aaa111"
        m.apply(owner())
        m.apply(update("bbb222"))
        assertFalse(m.updateAvailable)
        m.onInstall = {}
        m.apply(owner(role = "owner"))
        assertFalse(m.updateAvailable)
        m.apply(owner(owner = false))
        assertFalse(m.updateAvailable)
    }

    @Test fun homeShowsTheDaemonsShortCommit() {
        val m = MonitorModel()
        m.ownVersion = "dev"
        assertEquals("", m.versionLine)
        m.apply(ServerMessage(welcome = Welcome(protocol = 1, resume = ResumeKind.UP_TO_DATE, min_protocol = 1, version = "64b3068a5c0ffee1234")))
        assertTrue(m.versionLine.contains("masond 64b3068a"))
        assertFalse(m.versionLine.contains("64b3068a5"))
        assertTrue(m.versionTip.contains("64b3068a5c0ffee1234"))
        assertTrue(m.versionLine.contains("monitor (unversioned build)"))
        m.ownVersion = "abcdef0123456"
        assertTrue(m.versionLine.contains("monitor abcdef01"))
        m.apply(update("x", daemon = "masond is 3 commits behind develop"))
        assertTrue(m.versionLine.contains("3 commits behind develop"))
        m.apply(ServerMessage(welcome = Welcome(protocol = 1, resume = ResumeKind.UP_TO_DATE, min_protocol = 1, version = "dev")))
        assertTrue(m.versionLine.contains("masond (unversioned build)"))
    }

    @Test fun theDaemonButtonIsTheOwnersAndSendsTheUpdate() {
        val m = MonitorModel()
        val sent = mutableListOf<Command>()
        m.sender = { sent.add(it) }
        m.apply(update("x", daemon = "masond is 3 commits behind develop", canUpdate = true))
        assertEquals("masond is 3 commits behind develop", m.daemonUpdateText)
        assertFalse(m.canUpdateDaemon)
        m.apply(owner())
        assertTrue(m.canUpdateDaemon)
        m.updateDaemon()
        assertEquals("POST", sent.last().raw?.method)
        assertEquals("/update", sent.last().raw?.path)
    }

    @Test fun theDaemonButtonFollowsTheDaemonsWordsAndAsksBeforeUpdatingNow() {
        val m = MonitorModel()
        val sent = mutableListOf<Command>()
        m.sender = { sent.add(it) }
        m.apply(owner())
        m.apply(update("x", daemon = "behind", canUpdate = true))
        assertEquals("Update masond", m.daemonUpdateButton)
        m.apply(update("x", daemon = "waiting", canUpdate = true, button = "Update now", confirm = "Abandons star #142."))
        assertEquals("Update now", m.daemonUpdateButton)
        assertEquals("Abandons star #142.", m.daemonUpdateConfirm)
        val before = sent.size
        m.pressUpdateDaemon()
        assertTrue(m.confirmingDaemonUpdate)
        assertEquals(before, sent.size)
        m.cancelDaemonUpdate()
        assertFalse(m.confirmingDaemonUpdate)
        assertEquals(before, sent.size)
        m.pressUpdateDaemon()
        m.confirmDaemonUpdate()
        assertEquals("/update/now", sent.last().raw?.path)
        assertEquals("update-daemon", sent.last().id)
        m.apply(result("update-daemon", false, "no update is waiting"))
        assertEquals("no update is waiting", m.updateError)
    }

    @Test fun theMonitorButtonFollowsTheBuildAndInstallsItOnce() {
        val m = MonitorModel()
        val installed = mutableListOf<String>()
        val sent = mutableListOf<Command>()
        m.sender = { sent.add(it) }
        m.onInstall = { installed.add(it) }
        m.ownVersion = "aaa111"
        m.apply(owner())
        m.apply(update("bbb222", phase = "failed", error = "an earlier try"))
        m.updateMonitor()
        assertEquals("/update/monitor", sent.last().raw?.path)
        assertEquals("{\"kind\":\"compose\"}", sent.last().raw?.json)
        m.apply(update("bbb222", phase = "failed", error = "an earlier try"))
        assertTrue(m.updating != null)
        assertNull(m.updateError)
        m.apply(result("update-monitor", true))
        m.apply(update("bbb222", phase = "building"))
        assertEquals(emptyList(), installed)
        m.apply(update("bbb222", phase = "ready", path = "/x/bin/mason-monitor"))
        assertEquals(listOf("/x/bin/mason-monitor"), installed)
        m.apply(update("bbb222", phase = "ready", path = "/x/bin/mason-monitor"))
        assertEquals(listOf("/x/bin/mason-monitor"), installed)
    }

    @Test fun aMonitorBuildThatStopsSaysWhyAndCanBeTriedAgain() {
        val m = MonitorModel()
        m.sender = {}
        m.onInstall = {}
        m.ownVersion = "aaa111"
        m.apply(owner())
        m.apply(update("bbb222"))
        m.updateMonitor()
        m.apply(result("update-monitor", true))
        m.apply(update("bbb222", phase = "failed", error = "gradle failed"))
        assertNull(m.updating)
        assertTrue(m.updateError!!.contains("gradle failed"))
        assertTrue(m.updateAvailable)
        m.updateMonitor()
        m.apply(result("update-monitor", true))
        m.apply(update("bbb222"))
        assertNull(m.updating)
        assertTrue(m.updateError!!.contains("restarted"))
        m.updateMonitor()
        assertTrue(m.updating != null)
        m.apply(owner())
        assertNull(m.updating)
        m.updateMonitor()
        m.apply(result("update-monitor", false, "already being built"))
        assertEquals("already being built", m.updateError)
    }

    @Test fun aMonitorTooOldForTheProtocolIsReadOnlyAndSaysWhy() {
        val m = MonitorModel()
        var sent = 0
        m.sender = { sent += 1 }
        m.apply(welcome(minProtocol = 1))
        m.cancel(3)
        assertEquals(1, sent)
        assertNull(m.readOnlyReason)
        m.apply(welcome(minProtocol = 3))
        assertTrue(m.readOnlyReason!!.contains("protocol 2") && m.readOnlyReason!!.contains("3 or newer"))
        m.cancel(3)
        m.answer(1, "yes", "x")
        assertEquals(1, sent)
        assertTrue(m.lastResult!!.startsWith("Read-only:"))
        m.apply(welcome(minProtocol = 1))
        assertNull(m.readOnlyReason)
        m.cancel(3)
        assertEquals(2, sent)
    }

    // ---- the pipeline editor (Part 18) ----

    private fun editorView() = EditorView(
        project = "web",
        lanes = listOf(EditorLane("developing", "Developing"), EditorLane("testing", "Testing")),
        nodes = listOf(
            EditorNode(name = "develop", kind = "agent", category = "developing", is_start = true, summary = "deep"),
            EditorNode(name = "test", kind = "test", category = "testing", summary = "unit", problems = listOf("needs a bound")),
        ),
        edges = listOf(EditorEdge(from = "develop", to = "test", kind = "next"), EditorEdge(from = "test", to = "develop", kind = "on_red", bound = 2)),
        grid = TestGridView(columns = listOf("development", "test_step"), rows = listOf(GridRowView(suite = "unit", level = "unit", cells = listOf(false, true)))),
        problems = listOf("suite.x: odd"),
        dirty = true,
        can_undo = true,
        changes = listOf("adds step `lint`"),
    )

    @Test fun theEditorsItemsAreWhatTheViewSays() {
        val items = editorView().items().associateBy { it.id }
        assertEquals(listOf("develop", "agent", "developing", "deep", "start"), items["node:develop"]?.texts)
        assertEquals(listOf("test", "test", "testing", "unit", "needs a bound"), items["node:test"]?.texts)
        assertEquals(listOf("test", "develop", "on red", "at most 2 times"), items["edge:test:on_red:develop"]?.texts)
        assertEquals(listOf("develop", "test", "next"), items["edge:develop:next:test"]?.texts)
        assertEquals(listOf("unit", "unit", "test step"), items["grid:unit"]?.texts)
        assertEquals(listOf("suite.x: odd"), items["problem:0"]?.texts)
        assertEquals(listOf("Testing"), items["lane:testing"]?.texts)
        assertEquals(listOf("apply", "discard", "simulate", "undo"), items["draft"]?.actions?.sorted())
        assertEquals(listOf("adds step `lint`"), items["change:0"]?.texts)
        val clean = editorView().copy(dirty = false, can_undo = false, can_redo = true).items().first { it.id == "draft" }
        assertEquals(listOf("redo", "simulate"), clean.actions.sorted())
    }

    @Test fun editsAreSentAsTheOperationsTheDaemonTakes() {
        val m = MonitorModel()
        val sent = mutableListOf<Command>()
        m.sender = { sent += it }
        m.project = "web"
        m.editorOp(mapOf("op" to "move", "step" to "develop", "x" to 10L, "y" to 20L))
        m.editorCall("undo")
        assertEquals(listOf("POST", "POST"), sent.map { it.raw?.method })
        assertEquals(listOf("/projects/web/editor/op", "/projects/web/editor/undo"), sent.map { it.raw?.path })
        val body = org.json.JSONObject(sent[0].raw!!.json)
        assertEquals("move", body.getString("op"))
        assertEquals(10, body.getInt("x"))
        var subscribed = 0
        m.subscribe = { subscribed += 1 }
        m.selectStep("test")
        assertEquals("test", m.editorStep)
        assertEquals(1, subscribed)
    }

    @Test fun aFormIsBuiltFromTheSchemaAloneInTheOrderItGives() {
        val schema = """{"properties": {"max_rounds": {"type": "integer", "title": "Rounds"}, "category": {"type": "string", "enum": ["testing"], "title": "Category"}, "suites": {"type": "array", "items": {"type": "string", "enum": ["unit", "e2e"]}, "title": "Suites"}}, "x-order": ["category", "suites", "max_rounds"]}"""
        val f = mason.monitor.EditorJson.fields(schema)
        assertEquals(listOf("category", "suites", "max_rounds"), f.map { it.key })
        assertEquals(listOf("testing"), f[0].choices)
        assertEquals(listOf("unit", "e2e"), f[1].choices)
        assertEquals("array", f[1].type)
        assertEquals("integer", f[2].type)
        assertEquals(emptyList(), mason.monitor.EditorJson.fields("not json"))
    }

    // ---- answers and approvals by clicking (Stage 20) ----

    private fun card(hash: String = "h1") = DecisionCard(
        ask = "One hour or one day?", why = "The ticket is silent.",
        options = listOf(CardOption(label = "One hour", recommended = true), CardOption(label = "One day")),
        evidence = listOf("the ticket says tokens expire"), hash = hash,
    )

    private fun approval(hold: Boolean) = Approval(
        kind = if (hold) "hold" else "approval",
        waiting_for = "Your approval of a change to MASON's own rules",
        lines = listOf("touches crates/testing/src/verdict.rs (verdict logic)"),
        diff_hash = if (hold) "abc123" else "",
        branch = "mason/scratch/t3",
        diff_command = "git diff main...mason/scratch/t3",
    )

    private fun ownerModel(owner: Boolean = true): Pair<MonitorModel, MutableList<Command>> {
        val m = MonitorModel()
        val sent = mutableListOf<Command>()
        m.sender = { sent += it }
        m.identity = "brian"
        m.apply(ServerMessage(welcome = Welcome(protocol = 1, owner = owner)))
        return m to sent
    }

    @Test fun answeringByOptionSendsTheNumberAndTheCardsHash() {
        val (m, sent) = ownerModel()
        m.answer(2, "1", card().hash)
        val a = sent[0].answer!!
        assertEquals(listOf("1", "h1"), listOf(a.answer, a.card_hash))
        assertEquals("brian", sent[0].actor, "the owner's identity goes with it")
    }

    @Test fun otherTextIsSentAsFreeTextTrimmedAndNeverWhenBlank() {
        val (m, sent) = ownerModel()
        assertNull(otherAnswer("  \n "))
        m.answer(2, otherAnswer("  use two hours\nplease \n")!!, "h1")
        val a = sent[0].answer!!
        assertEquals("use two hours\nplease", a.answer)
        assertEquals("h1", a.card_hash)
    }

    @Test fun aStaleCardHashIsSentAsItWasAndTheRefusalIsShown() {
        val (m, sent) = ownerModel()
        m.answer(2, "1", card("old").hash)
        assertEquals("old", sent[0].answer!!.card_hash)
        m.apply(ServerMessage(result = CommandResult(id = "m1", ok = false, status = 409, error = "stale: that card has changed")))
        assertEquals("stale: that card has changed", m.lastResult, "the card shows the refusal")
    }

    @Test fun arrowsMoveTheSelectionAndReturnPicksIt() {
        var s = OptionSelection.starting(3, 1)
        assertEquals("2", s.answer, "it starts on the recommended option")
        s = s.down(); assertEquals("3", s.answer)
        s = s.down(); assertEquals("3", s.answer, "it stops at the end")
        s = s.up().up().up(); assertEquals("1", s.answer)
        val none = OptionSelection.starting(2, null)
        assertNull(none.answer, "nothing is picked until a key is pressed")
        assertEquals("2", none.up().answer)
        assertEquals(OtherKey.SEND, otherBoxKey(shift = false))
        assertEquals(OtherKey.NEWLINE, otherBoxKey(shift = true))
    }

    @Test fun spaceTogglesAndReturnSendsTheCheckedSet() {
        var s = MultiSelection.starting(4, 1)
        assertNull(s.answer, "nothing is checked until Space")
        s = s.toggle(); assertEquals("2", s.answer, "Space checks the highlighted option")
        s = s.down().down().toggle(); assertEquals("2,4", s.answer)
        s = s.toggle(0); assertEquals("1,2,4", s.answer, "a click checks that option, in card order")
        s = s.toggle(); assertEquals("2,4", s.answer, "Space on a checked option unchecks it")
        assertEquals(s, s.toggle(9), "a box that is not there changes nothing")
        assertNull(MultiSelection.starting(2, null).toggle().answer, "no highlight, nothing to toggle")
        assertNull(otherAnswer("  \n"), "a blank Other box sends nothing")
    }

    @Test fun aMultiSelectCardOffersSendToTheOwnerOnly() {
        val c = card().copy(multi_select = true)
        assertEquals(listOf("option:1", "option:2", "other", "send"), c.item(7).actions)
        assertEquals(emptyList(), c.item(7, owner = false).actions)
        assertEquals(listOf("option:1", "option:2", "other"), card().item(7).actions)
    }

    @Test fun approveSendsTheDiffHashShownForAHoldAndAPlainApproveOtherwise() {
        val (m, sent) = ownerModel()
        m.approve(3, approval(hold = true))
        m.approve(4, approval(hold = false))
        assertEquals(3L, sent[0].approve_hold!!.task)
        assertEquals("abc123", sent[0].approve_hold!!.diff_hash)
        assertEquals(4L, sent[1].approve!!.task)
        assertNull(sent[1].approve_hold)
    }

    @Test fun rejectSendsTheReason() {
        val (m, sent) = ownerModel()
        m.reject(3, "too broad")
        assertEquals(3L, sent[0].reject!!.task)
        assertEquals("too broad", sent[0].reject!!.reason)
    }

    private fun homeWith(c: DecisionCard?, a: Approval?) = ServerMessage(
        view = ViewUpdate(seq = 1, view = View(home = HomeView(attention = listOf(AttentionItem(task = row(3, "needs_attention"), card = c, approval = a))))),
    )

    @Test fun theOwnerSeesTheButtonsAndANonOwnerSeesTheCardAndNone() {
        for (owner in listOf(true, false)) {
            val (m, _) = ownerModel(owner)
            m.apply(homeWith(card(), approval(hold = true)))
            val items = m.visibleItems()
            val c = items.first { it.id == "card:3" }
            val a = items.first { it.id == "approval:3" }
            assertTrue("1. One hour (recommended)" in c.texts && "the ticket says tokens expire" in c.texts)
            assertTrue("diff abc123" in a.texts && "git diff main...mason/scratch/t3" in a.texts)
            assertEquals(if (owner) listOf("option:1", "option:2", "other") else emptyList(), c.actions)
            assertEquals(if (owner) listOf("approve", "reject") else emptyList(), a.actions)
            assertEquals(owner, m.canAct)
        }
    }

    @Test fun clickingTheAttentionCountOpensTheFirstTaskThatNeedsYou() {
        val (m, _) = ownerModel()
        m.openFirstNeedingYou()
        assertEquals(Page.Home, m.page, "nothing to open: stay on home")
        m.apply(ServerMessage(view = ViewUpdate(seq = 2, view = View(header_ = HeaderView(attention = 1)))))
        m.apply(homeWith(card(), null))
        m.openFirstNeedingYou()
        assertEquals(Page.Task(3), m.page)
    }

    // ---- graph layout ----

    private val masonBack = listOf(GraphLayout.BackEdge(440.0, 240.0, true), GraphLayout.BackEdge(640.0, 240.0, true))

    @Test fun graphNothingClippedLeft() {
        val h = GraphLayout.horizontal(listOf(40.0, 240.0, 440.0, 640.0), masonBack, 0.0)
        assertTrue(h.left + h.dx >= 0.0)
        assertTrue(h.right + h.dx <= h.width)
    }

    @Test fun graphCenteredWhenViewportWider() {
        val h = GraphLayout.horizontal(listOf(40.0, 240.0, 440.0, 640.0), masonBack, 2000.0)
        assertEquals(2000.0, h.width)
        assertEquals(h.left + h.dx, h.width - (h.right + h.dx), 0.5)
    }

    @Test fun graphWiderThanViewportKeepsPad() {
        val h = GraphLayout.horizontal(listOf(0.0, 2000.0), listOf(GraphLayout.BackEdge(2000.0, 0.0, true)), 500.0)
        assertEquals(GraphLayout.PAD, h.left + h.dx)
    }

    @Test fun graphNegativeLayoutX() {
        val h = GraphLayout.horizontal(listOf(-300.0, 100.0), emptyList(), 0.0)
        assertTrue(h.left + h.dx >= 0.0)
    }

    // ---- reconnecting after a daemon restart ----

    @Test fun reconnectPauseSchedule() {
        val p = ReconnectPolicy()
        assertEquals(listOf(1.0, 2.0, 4.0, 8.0, 10.0, 10.0, 10.0), (0 until 7).map { p.nextPauseSeconds() })
        p.reset()
        assertEquals(1.0, p.nextPauseSeconds())
    }

    @Test fun daemonInfoReplacesOnPortPidOrStartTime() {
        val a = DaemonInfo(7423, 5, 100)
        assertFalse(DaemonInfo(7423, 5, 100).replaces(a))
        assertTrue(DaemonInfo(7424, 5, 100).replaces(a))
        assertTrue(DaemonInfo(7423, 6, 100).replaces(a))
        assertTrue(DaemonInfo(7423, 5, 101).replaces(a))
    }

    @Test fun discoveryReadsPidAndStartTime() {
        val f = java.io.File.createTempFile("daemon", ".json")
        f.writeText("""{"pid": 41, "port": 7423, "started_ms": 99}""")
        assertEquals(DaemonInfo(7423, 41, 99), Discovery.info(f))
        assertEquals(DaemonInfo(9), Discovery.info(listOf("--port", "9"), emptyMap()))
    }

    private class FakeSocket(val port: Int, val events: SocketEvents) : MonitorSocket {
        val sent = java.util.concurrent.CopyOnWriteArrayList<String>()
        @Volatile var closed = false
        override fun send(text: String): Boolean { if (!closed) sent += text; testSignal.signal(); return !closed }
        override fun close() { if (!closed) { closed = true; events.onClosed() }; testSignal.signal() }
    }

    private class Rig(val model: MonitorModel, stallAfter: Double = 15.0, var info: DaemonInfo? = DaemonInfo(7423, 1, 10)) {
        val sockets = java.util.concurrent.CopyOnWriteArrayList<FakeSocket>()
        /** How many times the connection read daemon.json: once a session, then once a tick. */
        val discovered = java.util.concurrent.atomic.AtomicInteger()
        val conn = MonitorConnection(
            model, { discovered.incrementAndGet(); testSignal.signal(); info },
            ConnectionTiming(pingEverySeconds = 0.05, stallAfterSeconds = stallAfter, watchEverySeconds = 0.02, pauseScale = 0.01),
            { port, ev -> FakeSocket(port, ev).also { sockets += it; testSignal.signal() } },
        )
    }

    private val signal = testSignal
    private fun waitFor(@Suppress("UNUSED_PARAMETER") what: String, f: () -> Boolean) = signal.waitFor(f)

    private val welcome = """{"welcome":{"version":"test"}}"""

    @Test fun aDropThenAChangedDaemonJsonReconnectsWithoutARelaunch() {
        val r = Rig(MonitorModel(MemoryPreferences()))
        r.conn.start()
        waitFor("a socket") { r.sockets.size == 1 }
        r.sockets[0].events.onOpen()
        r.sockets[0].events.onMessage(welcome)
        waitFor("connected") { r.model.connected }
        r.model.lastSeq = 7
        r.info = DaemonInfo(9000, 2, 20)
        r.sockets[0].close()
        waitFor("a second socket") { r.sockets.size == 2 }
        assertEquals(9000, r.sockets[1].port)
        waitFor("not connected") { !r.model.connected }
        waitFor("reconnecting") { r.model.reconnecting }
        assertTrue(r.model.reconnecting, "shown as reconnecting, not an error")
        r.sockets[1].events.onOpen()
        assertTrue(Regex("[lL]ast_?[sS]eq\"?:\"?7\\b").containsMatchIn(r.sockets[1].sent[0]), r.sockets[1].sent[0])
        r.sockets[1].events.onMessage(welcome)
        waitFor("connected again") { r.model.connected }
        assertFalse(r.model.reconnecting)
        r.conn.stop()
    }

    @Test fun nothingIsSentBeforeTheSocketOpensAndTheHelloIsFirst() {
        // s220 failed one run in five: a ping went out on the new socket before the hello.
        val r = Rig(MonitorModel(MemoryPreferences()))
        r.conn.start()
        waitFor("a socket") { r.sockets.size == 1 }
        val ticks = r.discovered.get()
        // Ten ticks of 0.02 s are more than one ping interval (0.05 s): a ping would be due.
        waitFor("ten ticks") { r.discovered.get() >= ticks + 10 }
        assertTrue(r.sockets[0].sent.isEmpty(), "sent before the open: ${r.sockets[0].sent}")
        r.sockets[0].events.onOpen()
        assertTrue(r.sockets[0].sent[0].contains("hello"), r.sockets[0].sent[0])
        r.conn.stop()
    }

    @Test fun aChangedPidWhileConnectedReconnectsFromTheLastSeq() {
        val r = Rig(MonitorModel(MemoryPreferences()))
        r.conn.start()
        waitFor("a socket") { r.sockets.size == 1 }
        r.sockets[0].events.onMessage(welcome)
        waitFor("connected") { r.model.connected }
        r.model.lastSeq = 12
        r.info = DaemonInfo(7423, 2, 30)  // an update on the same port
        waitFor("a second socket") { r.sockets.size == 2 }
        assertTrue(r.sockets[0].closed)
        r.sockets[1].events.onOpen()
        assertTrue(Regex("[lL]ast_?[sS]eq\"?:\"?12\\b").containsMatchIn(r.sockets[1].sent[0]), r.sockets[1].sent[0])
        r.conn.stop()
    }

    @Test fun aStalledDaemonShowsReconnectingNotAnError() {
        val r = Rig(MonitorModel(MemoryPreferences()), stallAfter = 1.0)
        r.conn.start()
        waitFor("a socket") { r.sockets.size == 1 }
        r.sockets[0].events.onOpen()
        r.sockets[0].events.onMessage(welcome)
        waitFor("connected") { r.model.connected }
        waitFor("a ping") { r.sockets[0].sent.any { it.contains("ping") } }
        waitFor("a second socket") { r.sockets.size == 2 }
        assertTrue(r.sockets[0].closed)
        assertTrue(r.sockets[0].sent.any { it.contains("ping") }, "it asked before giving up")
        waitFor("reconnecting") { r.model.reconnecting }
        assertTrue(r.model.reconnecting)
        assertNull(r.model.lastResult, "no error is shown")
        r.conn.stop()
    }

    @Test fun noDaemonJsonYetKeepsRetrying() {
        val r = Rig(MonitorModel(MemoryPreferences()), info = null)
        r.conn.start()
        waitFor("reconnecting") { r.model.reconnecting }
        assertTrue(r.sockets.isEmpty())
        r.info = DaemonInfo(7423, 1, 1)
        waitFor("a socket") { r.sockets.size == 1 }
        r.conn.stop()
    }

    // ---- readable plan ----

    private fun planView(numbers: List<String> = listOf("0", "1")) = PlanView(
        summary_paragraphs = listOf("First.", "Second `x`."),
        steps_are_list = true,
        steps = listOf("do a\nmore a", "do b"),
        step_numbers = numbers,
        files = listOf("a.rs", "b.rs"),
        criteria = listOf("it works"),
        other = listOf(PlanExtra(label = "Risks", value_ = "- one")),
        raw = "{\"summary\":\"First.\"}",
    )

    @Test
    fun planCopyAsTextKeepsTheAgentsNumbers() {
        val t = PlanText.copyAsText(planView())
        assertEquals(
            "## Summary\n\nFirst.\n\nSecond `x`.\n\n## Steps\n\n0. do a\n   more a\n1. do b\n\n" +
                "## Files (2)\n\n- `a.rs`\n- `b.rs`\n\n## Acceptance criteria (1)\n\n- [ ] it works\n\n" +
                "## Other\n\n### Risks\n\n- one\n",
            t,
        )
        assertFalse(t.contains("{"))
    }

    @Test
    fun planCopyAsTextBulletsAndUnreadable() {
        val v = planView(numbers = emptyList())
        assertTrue(PlanText.copyAsText(v).contains("- do a\n  more a\n- do b"))
        assertNull(PlanText.marker(PlanView(), 0))
        assertEquals("•", PlanText.marker(v, 0))
        assertEquals("not json", PlanText.copyAsText(PlanView(raw = "not json", note = "n")))
    }

    @Test
    fun planExpandRule() {
        assertTrue(PlanText.expandedByDefault(0))
        assertTrue(PlanText.expandedByDefault(9))
        assertFalse(PlanText.expandedByDefault(10))
        assertFalse(PlanText.expandedByDefault(25))
    }

    @Test
    fun planInlineMarkdown() {
        assertEquals(
            listOf(PlanText.Span("a "), PlanText.Span("b", code = true), PlanText.Span(" "), PlanText.Span("c", bold = true), PlanText.Span(" `open")),
            PlanText.inline("a `b` **c** `open"),
        )
    }

    // ---- live progress (s49) ----

    private fun cell(
        unit: String = "tests", done: Int, total: Int = 0,
        doneSrc: ProgressSource = ProgressSource.SOURCE_REPORTED, totalSrc: ProgressSource = ProgressSource.SOURCE_NONE, low: Int = 0,
    ) = ProgressCell(step = "develop", state = CellState.PART, run = 1, unit = unit, done = done, total = total,
        done_source = doneSrc, total_source = totalSrc, total_low = low, stale_after_ms = 300_000)

    @Test fun countTextFollowsTheRuleForNumbers() {
        assertEquals("212 of 340 tests, 62%", cell(done = 212, total = 340, totalSrc = ProgressSource.SOURCE_REPORTED).countText)
        assertEquals("5 of 7 lines, 71%", cell(unit = "lines", done = 5, total = 7, doneSrc = ProgressSource.SOURCE_COUNTED, totalSrc = ProgressSource.SOURCE_COUNTED).countText)
        assertEquals("400 of 340 tests, 100%", cell(done = 400, total = 340, totalSrc = ProgressSource.SOURCE_REPORTED).countText)
        val h = cell(unit = "lines", done = 1240, total = 2400, doneSrc = ProgressSource.SOURCE_COUNTED, totalSrc = ProgressSource.SOURCE_HISTORY, low = 1800)
        assertEquals("1,240 lines, usually 1,800 to 2,400", h.countText)
        assertNull(h.percent)
        assertEquals("37 tool calls", cell(unit = "tool calls", done = 37).countText)
        assertEquals("3 tests", cell(done = 3, total = 0, totalSrc = ProgressSource.SOURCE_REPORTED).countText)
        assertEquals("", Format.countText("", 0, ProgressSource.SOURCE_NONE, 0, ProgressSource.SOURCE_NONE, 0))
    }

    @Test fun cellFill() {
        assertEquals(CellFill.Proportional(3f / 7f), cell(done = 3, total = 7, totalSrc = ProgressSource.SOURCE_REPORTED).fill)
        assertEquals(CellFill.Typical(0.25f), cell(done = 10, total = 40, totalSrc = ProgressSource.SOURCE_HISTORY).fill)
        assertEquals(CellFill.Typical(1f), cell(done = 50, total = 40, totalSrc = ProgressSource.SOURCE_HISTORY).fill)
        assertEquals(CellFill.Outline, cell(done = 10).fill)
    }

    @Test fun staleRuleAndTimeText() {
        assertEquals("updated 4 s ago", Format.timeText(10_000, 6_000, 300_000))
        assertEquals("no output for 6 min", Format.timeText(400_000, 40_000, 300_000))
        assertEquals("updated 4 min ago", Format.timeText(300_000, 40_000, 300_000))
        assertFalse(Format.isStale(300_000, 0, 1))
        assertFalse(Format.isStale(1_300_000, 1_000_000, 300_000))
        assertTrue(Format.isStale(1_300_001, 1_000_000, 300_000))
        assertEquals("1,234,567", Format.group(1_234_567))
    }

    @Test fun spokenLabelHasCountAndPercentButNoClock() {
        val c = cell(unit = "plan steps", done = 3, total = 7, totalSrc = ProgressSource.SOURCE_REPORTED).copy(updated_at_ms = 1_000)
        assertEquals("develop, running, 3 of 7 plan steps, 42 percent", c.spokenLabel)
        assertEquals("develop, running, 37 tool calls", cell(unit = "tool calls", done = 37).spokenLabel)
        assertEquals("develop, finished", c.copy(state = CellState.FILLED).spokenLabel)
    }

    @Test fun stepProgressUpdatesTheRowAndAnchorsTheClock() {
        val m = model()
        val c = cell(done = 1, total = 4, totalSrc = ProgressSource.SOURCE_REPORTED)
        m.taskDetail = TaskDetailView(row = row(1, "developing").copy(progress = listOf(c)))
        m.sidebar = m.sidebar!!.copy(sections = m.sidebar!!.sections.mapIndexed { i, sec ->
            if (i == 0) sec.copy(rows = sec.rows.mapIndexed { j, r -> if (j == 0) r.copy(progress = listOf(c)) else r }) else sec
        })
        val p = StepProgress(task = 1, progress = c.copy(done = 3), now_ms = 5_000_000, log = listOf("a", "b"))
        m.apply(p, local = 1_000_000)
        assertEquals(3, m.taskDetail!!.row!!.progress.first().done)
        assertEquals(3, m.sidebar!!.sections[0].rows[0].progress.first().done)
        assertEquals(listOf("a", "b"), m.liveLog)
        assertEquals(5_000_000, m.daemonNow(1_000_000))
        assertEquals(5_007_000, m.daemonNow(1_007_000))
        m.apply(p.copy(log = (0 until 60).map { "l$it" }), local = 1_000_000)
        assertEquals(40, m.liveLog.size)
        assertEquals("l59", m.liveLog.last())
    }

    @Test fun framesAreDecodedOffTheUiThreadAndAppliedOnIt() {
        val uiThread = java.util.concurrent.atomic.AtomicReference<Thread?>(null)
        val ui = java.util.concurrent.Executors.newSingleThreadExecutor { r -> Thread(r, "fake-ui").also { uiThread.set(it) } }
        val decodedOn = java.util.concurrent.CopyOnWriteArrayList<Thread>()
        val appliedOn = java.util.concurrent.CopyOnWriteArrayList<Thread>()
        val seen = java.util.concurrent.CopyOnWriteArrayList<String>()
        val m = MonitorModel(MemoryPreferences())
        m.onResult = { appliedOn += Thread.currentThread(); seen += it.id; testSignal.signal() }
        val sockets = java.util.concurrent.CopyOnWriteArrayList<FakeSocket>()
        val conn = MonitorConnection(
            m, { testSignal.signal(); DaemonInfo(7423, 1, 10) },
            ConnectionTiming(pingEverySeconds = 0.05, watchEverySeconds = 0.02, pauseScale = 0.01),
            { port, ev -> FakeSocket(port, ev).also { sockets += it; testSignal.signal() } },
            decode = { decodedOn += Thread.currentThread(); Codec.decode(it) },
            ui = ui,
        )
        conn.start()
        waitFor("a socket") { sockets.size == 1 }
        val socketThread = Thread { for (i in 1..20) sockets[0].events.onMessage("""{"result":{"id":"r$i","ok":true}}""") }
        socketThread.start()
        socketThread.join()
        waitFor("all results") { seen.size == 20 }
        conn.stop()
        ui.shutdown()
        assertEquals((1..20).map { "r$it" }, seen, "same messages, same order")
        assertEquals(20, decodedOn.size)
        assertTrue(decodedOn.none { it === uiThread.get() }, "decoding stays off the UI thread")
        assertTrue(appliedOn.all { it === uiThread.get() }, "apply runs on the UI thread")
    }

    @Test fun aHundredProgressUpdatesWhileBusyPublishTheSidebarOnce() {
        val m = model()
        val c = cell(done = 1, total = 400, totalSrc = ProgressSource.SOURCE_REPORTED)
        m.sidebar = m.sidebar!!.copy(sections = m.sidebar!!.sections.mapIndexed { i, sec ->
            if (i == 0) sec.copy(rows = sec.rows.mapIndexed { j, r -> if (j == 0) r.copy(progress = listOf(c)) else r }) else sec
        })
        val scheduled = ArrayList<() -> Unit>()
        m.scheduleProgressFlush = { scheduled += it }
        val before = m.sidebar
        for (i in 1..100) m.apply(ServerMessage(step_progress = StepProgress(task = 1, progress = c.copy(done = i), log = listOf("l$i"))))
        assertSame(before, m.sidebar, "nothing is published while the frame is not due")
        assertEquals(1, scheduled.size, "one flush is asked for, not one per message")
        scheduled[0]()
        assertNotSame(before, m.sidebar)
        assertEquals(100, m.sidebar!!.sections[0].rows[0].progress.first().done, "the last value wins")
        val after = m.sidebar
        m.flushProgress()
        assertSame(after, m.sidebar, "a second flush has nothing to publish")
    }

    @Test fun aViewArrivingAfterHeldProgressSeesTheProgressFirst() {
        val m = model()
        val c = cell(done = 1, total = 4, totalSrc = ProgressSource.SOURCE_REPORTED)
        m.sidebar = m.sidebar!!.copy(sections = m.sidebar!!.sections.mapIndexed { i, sec ->
            if (i == 0) sec.copy(rows = sec.rows.mapIndexed { j, r -> if (j == 0) r.copy(progress = listOf(c)) else r }) else sec
        })
        m.scheduleProgressFlush = { }
        m.apply(ServerMessage(step_progress = StepProgress(task = 1, progress = c.copy(done = 3))))
        assertEquals(1, m.sidebar!!.sections[0].rows[0].progress.first().done, "held")
        m.apply(ServerMessage(welcome = Welcome(version = "x")))
        assertEquals(3, m.sidebar!!.sections[0].rows[0].progress.first().done, "flushed before the next message")
    }

    @Test fun progressItemsHaveNoClockText() {
        val c = cell(done = 212, total = 340, totalSrc = ProgressSource.SOURCE_REPORTED).copy(message = "running the api suite", updated_at_ms = 1_000_000)
        val t = c.progressTexts(1_004_000)
        assertEquals("212 of 340 tests, 62%", t.first())
        assertTrue(t.none { it.contains(" ago") })
        assertFalse("silent" in t)
        assertTrue("silent" in c.progressTexts(1_400_000))
    }

    // ---- live tokens (s76) ----

    @Test fun tokensShowCompactlyAndNothingWhenNoneWereReported() {
        val c = cell(done = 5)
        assertEquals("", c.tokensCompact)
        assertTrue(c.progressTexts(1_000_000).none { it.startsWith("tokens") || it.startsWith("input") })
        val t = TokenLine(has_input = true, input = 4, has_output = true, output = 988,
            has_cache_read = true, cache_read = 7664, has_cache_write = true, cache_write = 4503)
        val with = c.copy(tokens = t)
        assertEquals("12.2k in, 988 out", with.tokensCompact)
        val texts = with.progressTexts(1_000_000)
        assertTrue("tokens 12.2k in, 988 out" in texts)
        assertTrue("cache read 7,664" in texts)
        assertTrue(texts.none { it.startsWith("cost") }, "cost only when reported")
        assertTrue("cost $0.0123" in c.copy(tokens = t.copy(has_cost = true, cost_usd = 0.0123)).progressTexts(1_000_000))
        assertEquals("999.9k", Format.compactCount(999_949))
        assertEquals("1.2M", Format.compactCount(1_234_567))
        assertEquals("5 out", Format.tokensCompact(TokenLine(has_output = true, output = 5)))
    }

    @Test fun sidebarWidthDefaultsToTheToken() = assertEquals(300.0, MonitorModel().sidebarBase)

    @Test fun sidebarWidthIsClampedWhenSet() {
        val m = MonitorModel()
        m.setSidebarWidth(10.0); assertEquals(220.0, m.sidebarBase)
        m.setSidebarWidth(5000.0); assertEquals(640.0, m.sidebarBase)
        m.setSidebarWidth(410.0); assertEquals(410.0, m.sidebarBase)
    }

    @Test fun aStoredWidthIsRestoredAndClamped() {
        val prefs = MemoryPreferences()
        MonitorModel(prefs).setSidebarWidth(450.0)
        assertEquals(450.0, MonitorModel(prefs).sidebarBase)
        prefs.setDouble(MonitorModel.KEY_SIDEBAR, 9000.0)
        assertEquals(640.0, MonitorModel(prefs).sidebarBase)
        prefs.setDouble(MonitorModel.KEY_SIDEBAR, 1.0)
        assertEquals(220.0, MonitorModel(prefs).sidebarBase)
    }

    @Test fun textZoomScalesTheStoredWidthUpToTheCapAndNotTheStoredValue() {
        val m = MonitorModel()
        m.setSidebarWidth(400.0)
        assertEquals(400.0, m.panelWidth(1.0), 0.001)
        assertEquals(480.0, m.panelWidth(1.2), 0.001)
        assertEquals(600.0, m.panelWidth(2.4), 0.001)
        assertEquals(400.0, m.sidebarBase)
    }

    @Test fun thePanelGivesWayBeforeTheMainView() {
        assertEquals(500.0, MonitorModel.fit(500.0, 1200.0, 1.0))
        assertEquals(800.0 - 360 - 8, MonitorModel.fit(500.0, 800.0, 1.0))
        assertEquals(220.0, MonitorModel.fit(500.0, 400.0, 1.0))
        assertEquals(330.0, MonitorModel.fit(300.0, 400.0, 2.0))
    }

    @Test fun sidebarEntryKeysAreUniqueAndNotPositional() {
        val keys = model().sidebarEntries().map { it.key }
        assertEquals(keys.size, keys.toSet().size)
        assertTrue(keys.any { it.startsWith("r:") })
    }

    @Test fun homeAndSearchAreNotInTheScrollingEntries() {
        val m = model()
        assertEquals(listOf("sidebar:home", "search"), SIDEBAR_PINNED_IDS)
        val es = m.sidebarEntries()
        assertTrue(es.first() is SidebarEntry.Header)
        assertEquals(listOf("Landing", "LLM usage", "Pipeline", "Health", "Settings"), es.filterIsInstance<SidebarEntry.PageLink>().map { it.title })
    }

    @Test fun aTileTargetResolvesToASectionInTheScrollingEntries() {
        val m = model()
        m.reveal("failed")
        val i = m.scrollIndex(m.scrollTarget!!)
        assertTrue(i >= 0)
        assertEquals("failed", (m.sidebarEntries()[i] as SidebarEntry.Header).section.state)
        assertEquals(-1, m.scrollIndex("nope"))
    }

    // ---- releasing and adding blockers (s75) ----

    private fun heldRow() = row(11, "queued", actions = listOf("cancel", "block", "unblock")).copy(
        blockers = listOf(8L, 9L),
        blocker_links = listOf(
            BlockerLink(task = 8, state = "canceled", origin = "from GitHub", actions = listOf("release", "revive")),
            BlockerLink(task = 9, state = "failed", origin = "set by you", actions = listOf("release", "retry")),
        ),
    )

    @Test fun aHeldTaskShowsEachBlockerWithItsOriginAndTheOwnersActions() {
        for (owner in listOf(true, false)) {
            val m = MonitorModel()
            m.apply(ServerMessage(welcome = Welcome(protocol = 1, owner = owner)))
            m.taskDetail = TaskDetailView(row = heldRow())
            m.go(Page.Task(11))
            val items = m.visibleItems()
            val b8 = items.first { it.id == "blocker:11:8" }
            val b9 = items.first { it.id == "blocker:11:9" }
            assertEquals(listOf("#8", "canceled", "from GitHub"), b8.texts)
            assertEquals(listOf("#9", "failed", "set by you"), b9.texts)
            assertEquals(if (owner) listOf("release", "revive") else emptyList(), b8.actions)
            assertEquals(if (owner) listOf("release", "retry") else emptyList(), b9.actions)
            assertEquals(if (owner) listOf("add_blocker") else emptyList(), items.first { it.id == "add-blocker:11" }.actions)
        }
    }

    @Test fun releaseAddBlockerAndReviveSendTheOwnersCommands() {
        val m = MonitorModel()
        m.identity = "brian"
        val sent = mutableListOf<Command>()
        m.sender = { sent += it }
        // masond answers each before the next click (a task with a command pending takes no second one).
        fun answered() = m.apply(ServerMessage(result = CommandResult(id = sent.last().id, ok = false, error = "no")))
        m.release(11, 8); answered()
        m.addBlocker(11, 5); answered()
        m.revive(8)
        assertEquals(listOf(8L), sent[0].unblock?.from)
        assertEquals(11L, sent[0].unblock?.task)
        assertEquals(listOf(5L), sent[1].block?.after)
        assertEquals(8L, sent[2].revive?.task)
        assertTrue(sent.all { it.actor == "brian" })
    }

    @Test fun theTaskPickerOffersLiveTasksThatAreNotAlreadyBlockers() {
        val m = model()
        m.sidebar = SidebarView(
            sections = listOf(
                section("queued", rows = listOf(heldRow(), row(12, "queued"), row(8, "queued"))),
                section("failed", rows = listOf(row(13, "failed"))),
                section("landed", rows = listOf(row(14, "landed"))),
            ),
        )
        assertEquals(listOf(12L), m.blockerCandidates(heldRow()).map { it.id })
    }

    @Test fun aWorkflowLineNamesEachStateThatHasJobsAndFailedIsKept() {
        val j = JobCounts(queued = 1, running = 1, succeeded = 2, failed = 1, running_names = listOf("build"))
        assertEquals("macOS: 5 jobs · 1 queued · 1 running (build) · 2 passed · 1 failed", Format.jobsText("macOS", j))
        assertEquals("CI: 1 job · 1 passed", Format.jobsText("CI", JobCounts(succeeded = 1)))
    }

    @Test fun tileShowsCurrentTaskForProposedAndStuck() {
        for (state in listOf("proposed", "needs_attention")) {
            val m = model()
            val c = StateCount(state = state, count = 2, wants_you = true)
            m.sidebar = SidebarView(sections = listOf(
                section(state, rows = listOf(row(1, state), row(2, state))),
                section("queued", rows = listOf(row(3, "queued"))),
            ))
            assertEquals(false, m.tileShowsCurrentTask(c), "$state on Home")
            m.go(Page.Task(3))
            assertEquals(false, m.tileShowsCurrentTask(c), "$state: a task in another state")
            m.go(Page.Task(1))
            assertEquals(true, m.tileShowsCurrentTask(c), "$state: first task")
            m.go(Page.Task(2))
            assertEquals(true, m.tileShowsCurrentTask(c), "$state: second task")
            m.goHome()
            assertEquals(false, m.tileShowsCurrentTask(c), "$state back on Home")
            m.go(Page.Task(2))
            m.sidebar = SidebarView(sections = listOf(
                section(state, rows = listOf(row(1, state))),
                section("queued", rows = listOf(row(2, "queued"), row(3, "queued"))),
            ))
            assertEquals(false, m.tileShowsCurrentTask(c), "$state: the shown task left the state")
        }
    }

    // ---- Land now for all ----

    private fun landingModel(owner: Boolean = true): Pair<MonitorModel, MutableList<Command>> {
        val m = model()
        m.isOwner = owner
        m.landing = LandingView(batches = listOf(BatchLine(id = 7, running = true, tasks = listOf(1L, 2L), allowed_actions = listOf("land_now"))))
        val sent = mutableListOf<Command>()
        m.sender = { sent += it }
        return m to sent
    }

    @Test fun landAllNowSendsOneBatchCommandAndOnePerQueuedTask() {
        val (m, sent) = landingModel()
        m.landAllNow(listOf(row(1, "landing", actions = listOf("land_now")), row(2, "landing", actions = listOf("land_now")),
            row(3, "landing", actions = listOf("land_now")), row(4, "landing", actions = listOf("cancel"))))
        assertEquals(listOf(7L), sent.mapNotNull { it.land_now_batch?.batch })
        assertEquals(listOf(3L), sent.mapNotNull { it.land_now?.task })
        assertEquals(2, sent.size)
        assertEquals("Land now sent for 3 tasks (1 batch)", m.landNotice)
    }

    @Test fun landAllNowSendsNothingWhenNothingAllowsItOrTheDeviceCannotAct() {
        val (m, sent) = landingModel()
        m.landAllNow(listOf(row(4, "landing", actions = listOf("cancel"))))
        assertTrue(sent.isEmpty())
        assertEquals("Nothing to land: no task here allows it", m.landNotice)
        m.isOwner = false
        assertNotNull(m.landDisabledReason)
        m.landAllNow(listOf(row(3, "landing", actions = listOf("land_now"))))
        assertTrue(sent.isEmpty())
        assertEquals(m.landDisabledReason, m.landNotice)
    }

    @Test fun landingRowsOfferLandNowOnlyWhenAllowedAndCanAct() {
        val (m, _) = landingModel()
        val rows = listOf(row(1, "landing", actions = listOf("land_now")), row(4, "landing", actions = listOf("cancel")))
        assertEquals(listOf(1L), m.landableRows(rows).map { it.id })
        assertTrue(m.canAct && "land_now" in rows[0].allowed_actions)
        m.isOwner = false
        assertFalse(m.canAct && "land_now" in rows[0].allowed_actions)
    }

    @Test
    fun activeHeaderCountsWorkingAndWaiting() {
        fun agent(task: Long, state: String, place: String = "") = ActiveModel(task = task, state = state, place = place)
        assertNull(ActiveSummary.header(emptyList()))
        val active = listOf(agent(1, "working"), agent(2, "working")) + (3L..7L).map { agent(it, "waiting_check", "github") }
        assertEquals("7 agents: 2 working, 5 waiting on GitHub checks", ActiveSummary.header(active))
        assertEquals("1 agent: 1 working", ActiveSummary.header(listOf(agent(1, ""))))
        assertEquals("waiting on a GitHub check", ActiveSummary.state(agent(3, "waiting_check", "github")))
        val r = RemoteSummary(
            checks = 7, test_steps = 4, debts = 2,
            pools = listOf(PoolUse(name = "gha", used = 13, limit = 16), PoolUse(name = "gha-macos", used = 2, limit = 4)),
        )
        assertEquals("13 of 16 runners: 7 checks, 4 test steps, 2 debts; macOS 2 of 4", ActiveSummary.github(r))
        assertNull(ActiveSummary.github(null))
    }
}
