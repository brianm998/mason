// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import mason.v1.*
import kotlin.test.*

/** s321: every project-scoped view is stamped with its project; the model drops one for another filter. */
class ProjectScopeTest {
    private fun header(project: String, attention: Int) = View(header_ = HeaderView(project = project, attention = attention, projects = listOf("a", "b")))

    @Test fun aViewStampedForAnotherProjectIsDropped() {
        val m = MonitorModel()
        m.project = "b"
        m.apply(header("a", 3))
        assertNull(m.header)
        m.apply(header("b", 1))
        assertEquals(1, m.header?.attention)
        m.apply(View(home = HomeView(project = "a")))
        m.apply(View(sidebar = SidebarView(project = "a")))
        m.apply(View(landing = LandingView(project = "a")))
        assertNull(m.home); assertNull(m.sidebar); assertNull(m.landing)
    }

    @Test fun changingTheSelectionClearsCountsUntilTheNewViewArrives() {
        val m = MonitorModel()
        m.apply(View(header_ = HeaderView(project = "", attention = 4, status = "4 need you", projects = listOf("a", "b"), counts = listOf(StateCount(state = "queued", count = 2)))))
        m.apply(View(home = HomeView(project = "")))
        m.project = "b"
        assertEquals(0, m.header?.attention)
        assertTrue(m.header?.counts.orEmpty().isEmpty())
        assertEquals(listOf("a", "b"), m.header?.projects, "the picker keeps its list")
        assertNull(m.home)
        m.apply(header("", 4)) // a rebuild that was in flight for the old filter
        assertEquals(0, m.header?.attention)
        m.apply(header("b", 1))
        assertEquals(1, m.header?.attention)
    }

    @Test fun homeLandingQueueIgnoresAStaleLandingView() {
        val m = MonitorModel()
        m.apply(View(landing = LandingView(project = "", queue = listOf(TaskRow(id = 9)))))
        m.project = "b"
        assertNull(m.landing)
        m.apply(View(landing = LandingView(project = "", queue = listOf(TaskRow(id = 9)))))
        assertNull(m.landing)
    }
}
