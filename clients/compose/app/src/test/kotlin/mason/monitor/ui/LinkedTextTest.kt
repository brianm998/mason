// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor.ui

import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.AnnotatedString
import mason.monitor.Linkify
import mason.v1.CiLink
import kotlin.test.Test
import kotlin.test.assertEquals

class LinkedTextTest {
    private val none = object : LinkActions {
        override fun openTask(id: Long) {}
        override fun openUrl(url: String) {}
    }

    private val links = listOf(
        CiLink(label = "PR #7", url = "https://github.com/o/r/pull/7", kind = "pull_request"),
        CiLink(label = "lint", url = "https://github.com/o/r/actions/runs/1/job/2", kind = "check"),
    )

    private fun linked(text: String, known: Set<Long>): AnnotatedString =
        linkedString(AnnotatedString(text), Linkify.segments(text, { it in known }, links), Color.Blue, none)

    @Test fun aSampleDetailTextGetsATaskLinkAndCiLinks() {
        val a = linked("waiting for #12; PR #7: checks failed: lint; see #99", setOf(12))
        assertEquals(
            listOf("#12" to "task:12", "PR #7" to "https://github.com/o/r/pull/7", "lint" to "https://github.com/o/r/actions/runs/1/job/2"),
            linkTargets(a),
        )
        assertEquals("waiting for #12; PR #7: checks failed: lint; see #99", a.text, "the text itself is unchanged")
    }

    @Test fun styledTextKeepsItsStylingAndGainsLinks() {
        val base = inlineMarkdown("run **#12** now")
        val a = linkedString(base, Linkify.segments(base.text, { it == 12L }, links), Color.Blue, none)
        assertEquals("run #12 now", base.text)
        assertEquals(listOf("#12" to "task:12"), linkTargets(a))
        assertEquals(base.spanStyles.size, a.spanStyles.size)
    }
}
