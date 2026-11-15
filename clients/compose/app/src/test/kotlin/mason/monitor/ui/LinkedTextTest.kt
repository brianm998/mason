// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor.ui

import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.style.TextDecoration
import mason.monitor.Linkify
import mason.v1.CiLink
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

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

    @Test fun everyLinkIsInTheLinkColourAndUnderlined() {
        val theme = Theme(dark = false, scale = 1.0)
        val text = "waiting for #12; PR #7: checks failed: lint; see #99"
        val a = linkedString(AnnotatedString(text), Linkify.segments(text, { it == 12L }, links), theme.link, none)
        val ranges = a.getLinkAnnotations(0, a.length)
        assertEquals(3, ranges.size)
        for (r in ranges) {
            val st = (r.item as androidx.compose.ui.text.LinkAnnotation.Clickable).styles?.style
            assertEquals(theme.link, st?.color)
            assertEquals(TextDecoration.Underline, st?.textDecoration)
        }
        assertTrue(theme.link != theme.accent, "a link is not the accent colour")
        val unknown = text.indexOf("#99")
        assertTrue(ranges.none { it.start <= unknown && unknown < it.end }, "an unknown #N stays plain")
    }

    /** Grep-style guard: the hand-made accent text with a click handler is gone from the ui package. */
    @Test fun noNavigationSiteDrawsItsOwnAccentLink() {
        val dir = listOf("src/main/kotlin/mason/monitor/ui", "app/src/main/kotlin/mason/monitor/ui")
            .map { java.io.File(it) }.first { it.isDirectory }
        // Not navigation: the CPU "more" toggle and the plan's copy button keep a plain button look.
        val allowed = listOf("cpu-more", "Copy plan")
        val bad = mutableListOf<String>()
        for (f in dir.listFiles()!!.filter { it.name.endsWith(".kt") }) {
            val lines = f.readLines()
            lines.forEachIndexed { i, line ->
                if (line.contains("color = theme.accent")) {
                    val window = lines.subList(maxOf(0, i - 3), minOf(lines.size, i + 4))
                    if (window.any { it.contains("clickable") } && window.none { w -> allowed.any { w.contains(it) } }) {
                        bad += "${f.name}:${i + 1}: $line"
                    }
                }
            }
        }
        assertTrue(bad.isEmpty(), "use Link (theme.link, underline, hand cursor) instead:\n" + bad.joinToString("\n"))
    }
}
