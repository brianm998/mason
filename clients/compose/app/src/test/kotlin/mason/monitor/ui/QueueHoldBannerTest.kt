// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor.ui

import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.test.ExperimentalTestApi
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.runComposeUiTest
import mason.monitor.MonitorModel
import mason.v1.Compilation
import mason.v1.QueueHold
import mason.v1.WaitParty
import kotlin.test.Test
import kotlin.test.assertEquals

/**
 * s527: the queue-held banner, the cause badge and the lease rows, from a fixture of the
 * 2026-11-16 stall. Compose's offscreen test rule: nothing appears on the screen.
 */
@OptIn(ExperimentalTestApi::class)
class QueueHoldBannerTest {
    private val lease = WaitParty(
        label = "starpipe heavy", lease = true, origin = "starpipe in /work/star",
        needs = "exclusive", exclusive = true, since_ms = 1_000_000,
    )
    private val hold = QueueHold(
        kind = "exclusive-request", cause = "behind-exclusive", waiting = 31, party = lease,
        summary = "Queue held: an exclusive `mason run` (starpipe heavy, from starpipe in /work/star, waiting 9 min) is waiting for #510's compile slot; 31 tasks wait behind it.",
    )

    @Test
    fun theBannerLinesAreTheSummaryThenTheLease() {
        assertEquals(listOf(hold.summary, "starpipe heavy, from starpipe in /work/star, wants exclusive, waiting 9 min"),
            queueHoldLines(hold, 1_000_000 + 9 * 60_000))
        assertEquals(listOf("Queue held: syspolicyd at 640% (limit 300%); 8 tasks wait behind it."),
            queueHoldLines(QueueHold(kind = "gate", summary = "Queue held: syspolicyd at 640% (limit 300%); 8 tasks wait behind it."), 0))
    }

    @Test fun theBannerBadgeAndLeaseRowsRender() = runComposeUiTest {
        val model = MonitorModel()
        setContent {
            CompositionLocalProvider(LocalTheme provides Theme(dark = false, scale = 1.0)) {
                androidx.compose.foundation.layout.Column {
                    QueueHoldBanner(model, hold)
                    WaitBadge("behind-exclusive")
                    CompilationSection(model, Compilation(limit = 6, leases = listOf(lease)))
                }
            }
        }
        onNodeWithTag("home:queue-hold").assertIsDisplayed()
        onNodeWithTag("home:queue-hold:lease").assertIsDisplayed()
        onNodeWithTag("wait:badge").assertIsDisplayed()
        onNodeWithTag("compile:lease:0").assertIsDisplayed()
    }
}
