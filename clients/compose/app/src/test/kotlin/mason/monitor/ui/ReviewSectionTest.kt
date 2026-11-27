// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor.ui

import androidx.compose.foundation.layout.Column
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.test.ExperimentalTestApi
import androidx.compose.ui.test.assertIsEnabled
import androidx.compose.ui.test.assertIsNotEnabled
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.runComposeUiTest
import mason.monitor.MonitorModel
import mason.monitor.ReviewSettings
import mason.monitor.SettingsPayload
import kotlin.test.Test
import kotlin.test.assertEquals

/** s1093: the "Pipeline review" section, drawn offscreen (nothing appears on the screen). */
@OptIn(ExperimentalTestApi::class)
class ReviewSectionTest {
    private fun ui(review: ReviewSettings, sent: MutableList<String>, test: androidx.compose.ui.test.ComposeUiTest.() -> Unit) = runComposeUiTest {
        setContent {
            CompositionLocalProvider(LocalTheme provides Theme(dark = false, scale = 1.0)) {
                Column {
                    ReviewSectionBody(MonitorModel(), review, { sent += it }, { sent += "run" })
                }
            }
        }
        test()
    }

    @Test fun eachControlSendsItsChange() {
        val sent = mutableListOf<String>()
        ui(ReviewSettings(at = "03:30", everyDays = 1, project = "mason"), sent) {
            onNodeWithTag("settings:review:enabled").performClick()
            onNodeWithTag("settings:review:hour:more").performClick()
            onNodeWithTag("settings:review:weekly").performClick()
            onNodeWithTag("settings:review:daily").assertIsNotEnabled()
            onNodeWithTag("settings:review:run").performClick()
        }
        assertEquals(
            listOf(
                SettingsPayload.reviewEnabled(false),
                SettingsPayload.reviewAt("04:30"),
                SettingsPayload.reviewEvery(7),
                "run",
            ),
            sent,
        )
    }

    @Test fun runNowIsOffWhileOneRuns() {
        ui(ReviewSettings(project = "mason", runningTask = 9), mutableListOf()) {
            onNodeWithTag("settings:review:run").assertIsNotEnabled()
            onNodeWithTag("settings:review:enabled").assertIsEnabled()
        }
    }
}
