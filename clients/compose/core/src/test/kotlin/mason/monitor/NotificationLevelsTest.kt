// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import mason.v1.*
import kotlin.test.*

/** s929: info banners and a progressing masond update leave the bar; the status line counts them. */
class NotificationLevelsTest {
    private fun banner(kind: String, level: String, text: String) = Banner(kind = kind, level = level, text = text)

    @Test
    fun infoIsSummarisedAndActionAndWarningAreNot() {
        val m = MonitorModel(MemoryPreferences())
        m.header = HeaderView(
            banners = listOf(
                banner("push_auth", "action", "star: push refused"),
                banner("ci_watch", "info", "mason: 2 red CI jobs being fixed: #935, #836"),
                banner("landing_frozen", "warning", "Landing is frozen for mason"),
            ),
            update = UpdateInfo(daemon_text = "Updating masond: building 4259e0bc…", daemon_level = "info"),
        )
        assertEquals("masond updating · 2 CI jobs being fixed", m.infoSummary)
        assertEquals(2, m.infoNotices.size)
        assertTrue(m.daemonUpdateIsInfo)
    }

    @Test
    fun noLevelReadsAsAWarning() {
        val m = MonitorModel(MemoryPreferences())
        m.header = HeaderView(banners = listOf(banner("x", "", "old daemon")))
        assertEquals("", m.infoSummary)
    }
}
