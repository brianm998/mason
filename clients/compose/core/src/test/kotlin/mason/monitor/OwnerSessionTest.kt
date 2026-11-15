// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import mason.v1.ActiveSession
import mason.v1.Banner
import mason.v1.HeaderView

/** s432: the health banner's Open session button and the sidebar's Active sessions panel. */
class OwnerSessionTest {
    @Test
    fun thePanelCountsTaskAndHealthSessionsAndHidesAtZero() {
        assertNull(ActiveSessions.title(emptyList()))
        val task = ActiveSession(id = 1, kind = "task", label = "#4 Fix it", working = true)
        val health = ActiveSession(id = 2, kind = "health", label = "Pipeline health", waiting = true)
        assertEquals("Active sessions (2)", ActiveSessions.title(listOf(task, health)))
        assertEquals("replying", ActiveSessions.marker(task))
        assertEquals("waiting for you", ActiveSessions.marker(health))
        assertEquals("", ActiveSessions.marker(ActiveSession(id = 3)))
    }

    @Test
    fun openSessionOpensTheDialogOnThatSession() {
        val m = MonitorModel()
        m.header = HeaderView(banners = listOf(Banner(kind = "health", session = 7, text = "Pipeline health #7: Cancel it?")))
        assertNull(m.ownerSessionDialog)
        m.openOwnerSession(m.header!!.banners[0].session)
        assertEquals(7L, m.ownerSessionDialog)
        m.closeOwnerSession()
        assertNull(m.ownerSessionDialog)
    }

    @Test
    fun aTranscriptIsReadFromTheDaemonsAnswer() {
        val t = OwnerSessionTranscript.parse("""{"session":{"id":7,"kind":"health","state":"open","working":false,"messages":[{"id":1,"role":"agent","text":"hello"}]}}""")
        assertEquals(7L, t.id)
        assertEquals("health", t.kind)
        assertEquals(listOf("hello"), t.messages.map { it.text })
    }
}
