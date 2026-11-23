// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor.ui

import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.test.ExperimentalTestApi
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.runComposeUiTest
import mason.monitor.MonitorModel
import mason.v1.Banner
import mason.v1.HeaderView
import kotlin.test.Test
import kotlin.test.assertEquals

/** #951: a row with detail has a "?" button that opens the dialog; a short row has none. Offscreen. */
@OptIn(ExperimentalTestApi::class)
class NoticeDetailsTest {
    @Test fun theQuestionMarkOpensTheDialogWithAnOpenTaskAction() = runComposeUiTest {
        val model = MonitorModel()
        model.connected = true
        model.header = HeaderView(banners = listOf(
            Banner(kind = "ci_watch", level = "warning", text = "mason: red on the base branch",
                detail = "mason: red on the base branch: smoke. #42 is fixing it."),
            Banner(kind = "warn", level = "warning", text = "Just a note"),
        ))
        setContent {
            CompositionLocalProvider(LocalTheme provides Theme(dark = false, scale = 1.0)) { BannersView(model) }
        }
        onNodeWithTag("details:banner:0").assertIsDisplayed()
        onNodeWithTag("details:banner:1").assertDoesNotExist()
        onNodeWithTag("details:banner:0").performClick()
        onNodeWithTag("notification-detail-summary").assertIsDisplayed()
        onNodeWithTag("notification-detail-text").assertIsDisplayed()
        onNodeWithTag("open-task:42").assertIsDisplayed()
        onNodeWithTag("open-task:42").performClick()
        assertEquals(mason.monitor.Page.from("mason://task/42"), model.page)
    }
}
