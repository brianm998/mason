// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor.ui

import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.test.ExperimentalTestApi
import androidx.compose.ui.test.assertTextEquals
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.runComposeUiTest
import kotlin.test.Test
import mason.monitor.LatenessWindow
import mason.monitor.MonitorModel

@OptIn(ExperimentalTestApi::class)
class HealthLineTest {
    @Test
    fun healthPageShowsTheInjectedNumbers() = runComposeUiTest {
        val model = MonitorModel()
        setContent {
            CompositionLocalProvider(LocalTheme provides Theme(dark = false, scale = 1.0)) {
                HealthPage(
                    model,
                    lateness = { LatenessWindow(pings = 2400, late = 31, worstMs = 640.0, totalLateMs = 9000.0) },
                    backgroundLateness = { LatenessWindow(pings = 0, late = 0, worstMs = 0.0, totalLateMs = 0.0) },
                )
            }
        }
        onNodeWithTag("health:ui-thread").assertTextEquals("UI thread: worst 640 ms late in the last 10 minutes, 31 late pings")
    }
}
