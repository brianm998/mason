// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import mason.v1.*
import kotlin.test.*

/** s270: paused banners collapse to icons (kept per device), prune when the pause is gone, and Resume lifts exactly one pause. */
class PauseCollapseTest {
    private fun banner(kind: String, project: String = "", mode: String = "") = Banner(
        kind = kind, project = project, mode = mode,
        text = if (kind == "paused") "Paused ($mode) for ${project.ifEmpty { "everything" }} by brian" else kind,
    )

    private fun header(vararg b: Banner) = HeaderView(banners = b.toList())

    @Test
    fun collapsingRecordsTheKeyAndExpandingClearsIt() {
        val prefs = MemoryPreferences()
        val m = MonitorModel(prefs)
        m.header = header(banner("paused", "star", "claims"))
        assertFalse(m.isPauseCollapsed("star", "claims"))
        m.setPauseCollapsed("star", "claims", true)
        assertEquals(true, prefs.bool("paused:star:claims"))
        assertTrue(MonitorModel(prefs).isPauseCollapsed("star", "claims"), "kept across launches")
        m.setPauseCollapsed("star", "claims", false)
        assertEquals(false, prefs.bool("paused:star:claims"))
        assertFalse(m.isPauseCollapsed("star", "claims"))
    }

    @Test
    fun aKeyWhosePauseIsGoneIsPruned() {
        val prefs = MemoryPreferences()
        val m = MonitorModel(prefs)
        m.header = header(banner("paused", "star", "claims"), banner("paused", "", "all"))
        m.setPauseCollapsed("star", "claims", true)
        m.setPauseCollapsed("", "all", true)
        m.header = header(banner("paused", "", "all"))
        assertFalse(m.isPauseCollapsed("star", "claims"))
        assertTrue(m.isPauseCollapsed("", "all"))
        assertFalse(MonitorModel(prefs).isPauseCollapsed("star", "claims"))
        m.header = header(banner("paused", "star", "claims"), banner("paused", "", "all"))
        assertFalse(m.isPauseCollapsed("star", "claims"), "a later pause starts expanded")
    }

    @Test
    fun resumePauseSendsBothProjectAndMode() {
        val m = MonitorModel()
        val sent = mutableListOf<Command>()
        m.sender = { sent += it }
        m.resumePause("star", "claims")
        m.resumePause("", "all")
        assertEquals(ResumeWork(project = "star", mode = "claims"), sent[0].resume)
        assertEquals(ResumeWork(project = "", mode = "all"), sent[1].resume, "an everything pause sends an empty project and its mode")
    }

    @Test
    fun noResumeActionWithoutCanAct() {
        val m = MonitorModel()
        assertFalse(m.canAct, "a monitor that is not the owner's cannot act: the view shows no Resume")
        m.isOwner = true
        assertTrue(m.canAct)
        m.readOnlyReason = "read-only"
        assertFalse(m.canAct)
    }

    @Test
    fun rowsHoldCollapsedIconsBesideTheFirstExpandedPause() {
        val m = MonitorModel()
        m.header = header(banner("paused", "a", "claims"), banner("connection"), banner("paused", "b", "all"), banner("paused", "c", "all"))
        m.setPauseCollapsed("a", "claims", true)
        assertEquals(listOf(MonitorModel.PauseRow(listOf(0), 2), MonitorModel.PauseRow(emptyList(), 3)), m.pauseRows())
        m.setPauseCollapsed("b", "all", true)
        m.setPauseCollapsed("c", "all", true)
        assertEquals(listOf(MonitorModel.PauseRow(listOf(0, 2, 3), null)), m.pauseRows(), "one icon-only row")
        assertEquals(4, m.visibleItems().count { it.id.startsWith("banner:") }, "the item view still lists every banner")
    }
}
