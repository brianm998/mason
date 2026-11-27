// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import mason.v1.TaskRow
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertTrue

/** #1046: the answer request from (option, context) and (Other, context), the choice a failed task's page makes, and the row badge. */
class DiagnosisCardTest {
    @Test
    fun anAnswerRequestFromAnOptionAndContext() {
        assertEquals(AnswerRequest("2", "the flaky one"), AnswerRequest.option(1, "  the flaky one \n"))
    }

    @Test
    fun anEmptyContextIsOmitted() {
        assertEquals("", AnswerRequest.option(0, "   ").context)
        assertNull(contextText(" \n"))
        assertEquals("", AnswerRequest.multi("1,3", "").context)
    }

    @Test
    fun otherSendsFreeTextAndTheContext() {
        assertEquals(AnswerRequest("use X", "because Y"), AnswerRequest.other(" use X ", "because Y"))
        assertEquals("", AnswerRequest.other("use X", "")?.context)
        assertNull(AnswerRequest.other("  ", "because Y"))
    }

    @Test
    fun aFailedPageChoosesCardDiagnosingOrButtons() {
        val acts = listOf("retry", "cancel", "open_session", "diagnose")
        assertEquals(FailedPage.Card, FailedPage.choose(true, false, listOf("answer") + acts))
        assertEquals(FailedPage.Diagnosing, FailedPage.choose(false, true, listOf("retry")))
        assertEquals(FailedPage.Buttons(true), FailedPage.choose(false, false, acts))
        assertEquals(FailedPage.Buttons(false), FailedPage.choose(false, false, listOf("retry")))
    }

    @Test
    fun otherActionsIsCollapsedWithACardAndOpenSessionStaysVisible() {
        assertTrue(FailedPage.Card.otherActionsCollapsed)
        assertTrue(FailedPage.Diagnosing.otherActionsCollapsed)
        assertFalse(FailedPage.Buttons(true).otherActionsCollapsed)
        for (a in listOf("retry", "cancel", "skip_all_tests")) assertTrue(FailedPage.Card.hidesUnderOther(a))
        assertFalse(FailedPage.Card.hidesUnderOther("open_session"))
        assertFalse(FailedPage.Buttons(true).hidesUnderOther("retry"))
    }

    @Test
    fun theDiagnosingLineAndTheRowBadge() {
        assertEquals("Diagnosing on Opus medium… started just now", diagnosingLine(0, "Opus medium", 0))
        assertTrue(diagnosingLine(1_000, "", 181_000).endsWith("ago"))
        assertEquals("diagnosis ready", diagnosisBadge("ready"))
        assertEquals("diagnosing", diagnosisBadge("running"))
        assertNull(diagnosisBadge(""))
        assertEquals("idle · diagnosis ready", QueueWait.line(TaskRow(activity = "idle", diagnosis = "ready")))
    }
}
