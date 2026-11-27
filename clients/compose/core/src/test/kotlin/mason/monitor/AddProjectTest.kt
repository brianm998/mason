// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import mason.v1.CommandResult
import mason.v1.ServerMessage
import java.io.File
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue

class AddProjectTest {
    private fun plan(next: String?, files: Boolean = false, errors: String = "") = """{"detected":{"root":"/r/web","id":"web","branch":"main","languages":["rust","python"],"ci":[],"existing_mason":false},"kind":"init",
      "next":${next ?: "null"},"files":${if (files) """[{"path":".mason/mason.toml","contents":"x = 1"}]""" else "[]"},"errors":[$errors]}"""

    private fun q(id: String, default: String, opts: String = """["a","b"]""") =
        """{"id":"$id","text":"Which $id?","options":$opts,"default":"$default","flag":"--$id"}"""

    @Test fun answersBecomeTheFlagMap() {
        val w = AddProjectWizard()
        w.path = " /r/web "
        w.answer(WizardQuestion("tracker", "t", listOf("local"), "local"), "github")
        w.answer(WizardQuestion("ci-workflow", "t", emptyList(), "no"), "yes")
        assertEquals(mapOf<String, Any?>("tracker" to "github", "ci_workflow" to "yes", "land" to "no"), w.flags())
        val body = EditorJson.parse(w.planBody())
        assertEquals("/r/web", body.getString("path"))
        assertEquals("github", body.getJSONObject("answers").getString("tracker"))
    }

    @Test fun landingDefaultsToNoAndProofIsOnByDefault() {
        val w = AddProjectWizard()
        assertEquals("no", w.land)
        assertEquals(listOf("no", "merge", "push", "pr"), landChoices)
        assertFalse(w.flags().containsKey("no_prove"))
        w.prove = false
        w.land = "pr"
        assertEquals(true, w.flags()["no_prove"])
        assertEquals("pr", w.flags()["land"])
    }

    @Test fun stepsFollowThePlans() {
        val w = AddProjectWizard()
        assertEquals(AddProjectStep.REPOSITORY, w.step)
        assertFalse(w.planArrived(plan(q("tracker", "local"))))
        assertEquals(AddProjectStep.DETECTED, w.step)
        assertTrue(w.plan!!.detected.contains("Languages" to "rust, python"))
        w.continueFromDetected()
        assertEquals(AddProjectStep.QUESTIONS, w.step)
        w.planArrived(plan(null, files = true))
        assertEquals(AddProjectStep.REVIEW, w.step)
        assertEquals(1, w.plan!!.files.size)
    }

    @Test fun useRecommendedAnswersTheRestOneAfterAnother() {
        val w = AddProjectWizard()
        w.planArrived(plan(q("tracker", "local")))
        w.continueFromDetected()
        w.useRecommended()
        assertEquals(mapOf("tracker" to "local"), w.answers)
        assertTrue(w.planArrived(plan(q("agent", "claude")))) // asks again
        assertTrue(w.planArrived(plan(q("sessions", "2"))))
        assertFalse(w.planArrived(plan(null, files = true)))
        assertEquals(mapOf("tracker" to "local", "agent" to "claude", "sessions" to "2"), w.answers)
        assertEquals(AddProjectStep.REVIEW, w.step)
        assertFalse(w.usingRecommended)
    }

    @Test fun aPlanWithErrorsStaysPut() {
        val w = AddProjectWizard()
        w.planArrived(plan(null, errors = "\"not a git repository\""))
        assertEquals(AddProjectStep.REPOSITORY, w.step)
        assertEquals("not a git repository", w.error)
    }

    @Test fun planWarningsAndApplyWarningsAndNotesAreReadBack() {
        val w = AddProjectWizard()
        w.planArrived(plan(null, files = true).replace("\"errors\"", "\"warnings\":[\"no claude on PATH\"],\"errors\""))
        assertEquals(listOf("no claude on PATH"), w.plan?.warnings)
        w.startApply()
        w.applyArrived(true, """{"id":"web","registered":true,"progress":["x"],"warnings":["the test step runs true"],"notes":["chose main"]}""", "")
        assertEquals(listOf("the test step runs true"), w.applyWarnings)
        assertEquals(listOf("chose main"), w.applyNotes)
        assertEquals("web", w.appliedId)
    }

    @Test fun backFromReviewForgetsTheLastAnswer() {
        val w = AddProjectWizard()
        w.planArrived(plan(q("tracker", "local")))
        w.continueFromDetected()
        w.answer(WizardQuestion("tracker", "", emptyList(), ""), "local")
        w.answer(WizardQuestion("agent", "", emptyList(), ""), "claude")
        w.planArrived(plan(null))
        assertTrue(w.back())
        assertEquals(mapOf("tracker" to "local"), w.answers)
        assertEquals(AddProjectStep.QUESTIONS, w.step)
    }

    @Test fun candidatesParse() {
        val w = AddProjectWizard()
        w.candidatesArrived("""{"candidates":[{"path":"/r/web","name":"web","has_mason":true,"remote":null},{"path":"/r/api","name":"api","has_mason":false,"remote":"git@github.com:a/api.git"}],"errors":[{"path":"/gone","error":"no such directory"}]}""")
        assertEquals(listOf(Candidate("/r/web", "web", true, null), Candidate("/r/api", "api", false, "git@github.com:a/api.git")), w.candidates)
        assertEquals(listOf("/gone: no such directory"), w.candidateErrors)
    }

    private fun owner() = MonitorModel().also { it.isOwner = true }

    @Test fun applySuccessSelectsTheNewProject() {
        val m = owner()
        m.openAddProject()
        val w = assertNotNull(m.addProject)
        w.path = "/r/web"
        m.addProjectApply()
        assertEquals(AddProjectStep.APPLY, w.step)
        m.apply(ServerMessage(result = CommandResult(id = "onboard-apply", ok = true, json = """{"id":"web","registered":true,"progress":["wrote .mason/mason.toml","registered web"]}""")))
        assertEquals("web", m.project)
        assertEquals("web", w.appliedId)
        assertEquals(listOf("wrote .mason/mason.toml", "registered web"), w.progress)
        assertNull(w.error)
    }

    @Test fun applyFailureKeepsTheProjectAndSaysWhy() {
        val m = owner()
        m.openAddProject()
        val w = assertNotNull(m.addProject)
        m.addProjectApply()
        m.apply(ServerMessage(result = CommandResult(id = "onboard-apply", ok = false, error = "a project named \"web\" is already registered")))
        assertEquals("", m.project)
        assertEquals("a project named \"web\" is already registered", w.error)
        assertNull(w.appliedId)
    }

    @Test fun onlyAnOwnerCanOpenIt() {
        val m = MonitorModel()
        m.openAddProject()
        assertNull(m.addProject)
    }

    @Test fun theAndroidAppHasNoAddProjectControl() {
        // The Android app shares :core only; it must never draw the wizard or its entry points.
        val dir = listOf("../android/src", "../../android/src", "android/src").map(::File).firstOrNull { it.isDirectory } ?: return
        val hits = dir.walkTopDown().filter { it.isFile && it.extension == "kt" }
            .filter { f -> f.readText().let { "add-project" in it || "AddProject" in it || "addProject" in it } }.toList()
        assertTrue(hits.isEmpty(), "Android sources mention Add Project: $hits")
    }
}
