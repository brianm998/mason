// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor.ui

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNotEquals
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
}
