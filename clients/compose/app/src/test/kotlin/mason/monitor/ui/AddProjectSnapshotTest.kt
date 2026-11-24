// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor.ui

import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.test.ExperimentalTestApi
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.assertTextContains
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.runComposeUiTest
import mason.monitor.AddProjectStep
import mason.monitor.MonitorModel
import mason.v1.Command
import mason.v1.HeaderView
import kotlin.test.Test
import kotlin.test.assertEquals

/** Each Add Project step, drawn offscreen by Compose's test rule (nothing appears on the screen). */
@OptIn(ExperimentalTestApi::class)
class AddProjectSnapshotTest {
    private fun owner() = MonitorModel().also {
        it.connected = true
        it.isOwner = true
        it.header = HeaderView(projects = listOf("mason"))
    }

    private fun q(id: String) = """{"id":"$id","text":"Which $id?","options":["a","b"],"default":"a","flag":"--$id"}"""
    private fun plan(next: String?, files: Boolean = false) =
        """{"detected":{"root":"/r/web","id":"web","branch":"main","languages":["rust"],"ci":[],"existing_mason":false},"kind":"init","next":${next ?: "null"},"files":${if (files) """[{"path":".mason/mason.toml","contents":"x = 1"}]""" else "[]"},"errors":[]}"""

    @Test fun repositoryStepListsCandidatesAndAPathField() = runComposeUiTest {
        val m = owner()
        m.openAddProject()
        val w = m.addProject!!
        w.candidatesArrived("""{"candidates":[{"path":"/r/web","name":"web","has_mason":false,"remote":null}],"errors":[]}""")
        setContent { CompositionLocalProvider(LocalTheme provides Theme(false, 1.0)) { AddProjectStepView(m, w) } }
        onNodeWithTag("add-project-step:repository").assertIsDisplayed()
        onNodeWithTag("add-project-candidate:web").performClick()
        assertEquals("/r/web", w.path)
        onNodeWithTag("add-project-path").assertIsDisplayed()
    }

    @Test fun theFolderPickerIsOnlyOfferedWhenTheMonitorCanBrowse() = runComposeUiTest {
        val m = owner()
        m.openAddProject()
        val w = m.addProject!!
        setContent { CompositionLocalProvider(LocalTheme provides Theme(false, 1.0)) { AddProjectStepView(m, w) } }
        onNodeWithTag("add-project-choose").assertDoesNotExist()
        m.chooseDirectory = { "/picked" }
        onNodeWithTag("add-project-choose").performClick()
        assertEquals("/picked", w.path)
    }

    @Test fun detectedStepShowsWhatWasFound() = runComposeUiTest {
        val m = owner()
        m.openAddProject()
        val w = m.addProject!!
        w.planArrived(plan(q("tracker")))
        setContent { CompositionLocalProvider(LocalTheme provides Theme(false, 1.0)) { AddProjectStepView(m, w) } }
        onNodeWithTag("add-project-step:detected").assertIsDisplayed()
        assertEquals(AddProjectStep.DETECTED, w.step)
    }

    @Test fun questionsStepOffersOptionsAndUseRecommended() = runComposeUiTest {
        val m = owner()
        val sent = mutableListOf<Command>()
        m.sender = { sent += it }
        m.openAddProject()
        val w = m.addProject!!
        w.path = "/r/web"
        w.planArrived(plan(q("tracker")))
        w.continueFromDetected()
        setContent { CompositionLocalProvider(LocalTheme provides Theme(false, 1.0)) { AddProjectStepView(m, w) } }
        onNodeWithTag("add-project-question").assertTextContains("Which tracker?")
        onNodeWithTag("add-project-option:b").performClick()
        assertEquals("b", w.answers["tracker"])
        assertEquals("/onboarding/plan", sent.last().raw!!.path)
        onNodeWithTag("add-project-recommended").assertIsDisplayed()
    }

    @Test fun reviewStepShowsFilesLandingAndProof() = runComposeUiTest {
        val m = owner()
        m.openAddProject()
        val w = m.addProject!!
        w.planArrived(plan(null, files = true))
        w.continueFromDetected()
        setContent { CompositionLocalProvider(LocalTheme provides Theme(false, 1.0)) { AddProjectStepView(m, w) } }
        onNodeWithTag("add-project-step:review").assertIsDisplayed()
        onNodeWithTag("add-project-file:.mason/mason.toml").assertIsDisplayed()
        listOf("no", "merge", "push", "pr").forEach { onNodeWithTag("add-project-land:$it").assertIsDisplayed() }
        assertEquals("no", w.land)
        onNodeWithTag("add-project-land:merge").performClick()
        assertEquals("merge", w.land)
        onNodeWithTag("add-project-prove").performClick()
        assertEquals(false, w.prove)
    }

    @Test fun applyStepShowsProgressAndTheResult() = runComposeUiTest {
        val m = owner()
        m.openAddProject()
        val w = m.addProject!!
        w.startApply()
        w.applyArrived(true, """{"id":"web","registered":true,"progress":["wrote files","registered web"]}""", "")
        setContent { CompositionLocalProvider(LocalTheme provides Theme(false, 1.0)) { AddProjectStepView(m, w) } }
        onNodeWithTag("add-project-done").assertTextContains("Added web.")
    }

    @Test fun thePlusIsOnlyForAnOwnerOnDesktop() = runComposeUiTest {
        val m = owner()
        setContent { CompositionLocalProvider(LocalTheme provides Theme(false, 1.0)) { TopBar(m) } }
        onNodeWithTag("add-project").assertIsDisplayed()
        onNodeWithTag("add-project").performClick()
        assertEquals(true, m.addProject != null)
    }

    @Test fun noPlusWithoutOwnerRights() = runComposeUiTest {
        val m = owner().also { it.isOwner = false }
        setContent { CompositionLocalProvider(LocalTheme provides Theme(false, 1.0)) { TopBar(m) } }
        onNodeWithTag("add-project").assertDoesNotExist()
    }
}
