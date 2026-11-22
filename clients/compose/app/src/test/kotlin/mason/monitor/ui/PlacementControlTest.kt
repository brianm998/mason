// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor.ui

import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.test.ExperimentalTestApi
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.assertTextContains
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.runComposeUiTest
import mason.monitor.MonitorModel
import mason.monitor.PlacementScope
import mason.v1.Command
import mason.v1.HeaderView
import mason.v1.Placement
import mason.v1.ProjectPlacement
import kotlin.test.Test
import kotlin.test.assertEquals

/** #834: the Runs on control, drawn offscreen by Compose's test rule (nothing appears on the screen). */
@OptIn(ExperimentalTestApi::class)
class PlacementControlTest {
    @Test fun theProjectControlShowsTheSourceAndSendsAfterConfirm() = runComposeUiTest {
        val model = MonitorModel()
        model.connected = true
        model.isOwner = true
        model.header = HeaderView(project_placements = listOf(
            ProjectPlacement(project = "scratch", placement = Placement(mode = "favor-remote", source = "project", own = true, settable = true)),
        ))
        val sent = mutableListOf<Command>()
        model.sender = { sent += it }
        setContent {
            CompositionLocalProvider(LocalTheme provides Theme(dark = false, scale = 1.0)) {
                PlacementControl(model, PlacementScope.Project("scratch"))
            }
        }
        onNodeWithTag("placement:project:scratch:text").assertTextContains("favor-remote, from the project")
        onNodeWithTag("placement:project:scratch:default").assertIsDisplayed()
        onNodeWithTag("placement:project:scratch:local").performClick()
        onNodeWithTag("placement-confirm").performClick()
        assertEquals(listOf("local"), sent.map { it.set_placement!!.mode })
    }
}
