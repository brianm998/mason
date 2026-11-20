// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor.ui

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNotEquals
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import androidx.compose.ui.unit.dp
import mason.monitor.MonitorModel
import mason.monitor.busiestState
import mason.monitor.topBarMascot
import mason.monitor.topBarMascotTooltip
import mason.v1.HeaderView
import mason.v1.StateCount
import mason.v1.ProviderStatus
import mason.v1.UsageView

/** s203: the stacked top bar shows every usage line, and a zero count is gray. */
class TopBarTest {
    @Test
    fun zeroCountIsMutedAndNonZeroIsNot() {
        val theme = Theme(dark = false, scale = 1.0)
        assertEquals(theme.muted, tileCountColor(theme, 0, wants = false, connected = true))
        assertEquals(theme.text, tileCountColor(theme, 5, wants = false, connected = true))
        assertEquals(theme.attention, tileCountColor(theme, 0, wants = true, connected = true))
        assertEquals(theme.text, tileCountColor(theme, 0, wants = false, connected = false))
        assertNotEquals(theme.muted, theme.text)
    }

    @Test
    fun usageLinesAreAllTheAccountsLinesInOrder() {
        val u = UsageView(top_bar = listOf("claude 5-hour 82%", "claude-second Weekly 9%", "claude Weekly 40%"))
        assertEquals(listOf("claude 5-hour 82%", "claude Weekly 40%"), topBarUsageLines(u, ProviderStatus(name = "claude")))
        assertEquals(
            listOf("codex: 1 of 4 sessions"),
            topBarUsageLines(u, ProviderStatus(name = "codex", sessions_used = 1, sessions_limit = 4)),
        )
    }

    private fun model(vararg counts: Pair<String, Int>) = MonitorModel().also { m ->
        m.connected = true
        m.header = HeaderView(counts = counts.map { StateCount(state = it.first, label = it.first, count = it.second) })
    }

    @Test
    fun busiestActiveStateWinsAndTiesGoLate() {
        assertEquals("testing", model("testing" to 7, "developing" to 3, "landed" to 40).busiestState?.first)
        assertEquals("testing", model("developing" to 3, "testing" to 3).busiestState?.first)
        assertNull(model("testing" to 0, "landed" to 9, "canceled" to 4).busiestState)
        assertEquals("all_clear", model("testing" to 0).topBarMascot?.id)
        assertEquals("Most tasks: testing (7)", model("testing" to 7).topBarMascotTooltip)
    }

    @Test
    fun topBarDrawsTheBusiestStatesIllustration() {
        val m = model("testing" to 7, "developing" to 3, "landed" to 40)
        assertEquals("testing", m.topBarMascot?.id)
        assertNotNull(Mascot.illustration("testing"))
        assertEquals(85f, topBarMascotSide(100.dp, 1000.dp).value, 0.01f)
        assertEquals(50f, topBarMascotSide(96.dp, 50.dp).value, 0.01f)
    }

    @Test
    fun onlyProposedAndStuckFillWhenShown() {
        assertEquals(true, tileFillsOnlyWhenShown("proposed"))
        assertEquals(true, tileFillsOnlyWhenShown("needs_attention"))
        assertEquals(false, tileFillsOnlyWhenShown("failed"))
        assertEquals(false, tileFillsOnlyWhenShown("queued"))
    }
}
