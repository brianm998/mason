// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor.ui

import androidx.compose.foundation.layout.Column
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.test.ExperimentalTestApi
import androidx.compose.ui.test.assertIsEnabled
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.runComposeUiTest
import mason.monitor.RemoteSettings
import kotlin.test.Test
import kotlin.test.assertEquals

/** #891: the "Where runs go" section in the three source states, drawn offscreen (nothing appears on the screen). */
@OptIn(ExperimentalTestApi::class)
class RemotePlacementTest {
    private val body = """{"mode":"favor-remote","default":"local","problem":"unknown mode `x`, treated as local","changed_by":"owner","changed_at":"2026-11-21T10:00:00Z","projects":[{"project":"web","remote":true,"runs_on":"macos-latest","mode":"remote","source":"project"},{"project":"api","remote":true,"runs_on":"ubuntu-latest","mode":"favor-remote","source":"global"},{"project":"local-only","remote":false,"runs_on":"","mode":"local","source":"no [remote_runs]"}]}"""

    private fun ui(project: String, picked: MutableList<String>, test: androidx.compose.ui.test.ComposeUiTest.() -> Unit) = runComposeUiTest {
        setContent {
            CompositionLocalProvider(LocalTheme provides Theme(dark = false, scale = 1.0)) {
                Column {
                    RemotePlacementBody(RemoteSettings.parse(body), project, { picked += "global:$it" }, { picked += "project:$it" })
                }
            }
        }
        test()
    }

    @Test fun aProjectSetForItselfCanBeChanged() {
        val picked = mutableListOf<String>()
        ui("web", picked) {
            onNodeWithTag("settings:remote:project:local").assertIsEnabled()
            onNodeWithTag("settings:remote:project:local").performClick()
            onNodeWithTag("settings:remote:global:remote").performClick()
        }
        assertEquals(listOf("project:local", "global:remote"), picked)
    }

    @Test fun aProjectFollowingTheGlobalModeCanBeChanged() {
        ui("api", mutableListOf()) { onNodeWithTag("settings:remote:project:local").assertIsEnabled() }
    }

    @Test fun aProjectWithNoRemoteRunsCanBeSentToNodes() {
        ui("local-only", mutableListOf()) {
            onNodeWithTag("settings:remote:project:remote").assertIsEnabled()
            onNodeWithTag("settings:remote:global:remote").assertIsEnabled()
        }
    }
}
