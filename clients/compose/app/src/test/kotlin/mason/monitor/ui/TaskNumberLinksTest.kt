// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor.ui

import mason.monitor.Linkify
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.AnnotatedString
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

/** s662: a task number on screen is a link, and no `Text(` can print one as plain text again. */
class TaskNumberLinksTest {
    private val none = object : LinkActions {
        override fun openTask(id: Long) {}
        override fun openUrl(url: String) {}
    }

    private fun linked(text: String, known: Set<Long>) =
        linkedString(AnnotatedString(text), Linkify.segments(text, { it in known }, emptyList()), Color.Blue, none)

    @Test fun aHeldRowLinksTheTaskItIsBlockedBy() {
        assertEquals(listOf("#613" to "task:613"), linkTargets(linked("held, blocked by #613", setOf(613))))
    }

    @Test fun aBatchLineLinksOnlyItsTasks() {
        val b = mason.v1.BatchLine(id = 390, project = "mason", phase = "verifying", tasks = listOf(579), parked = listOf(567))
        val text = batchText(b)
        assertTrue(text.startsWith("Batch #390 mason verifying: #579"), text)
        assertEquals(listOf("#579" to "task:579"), linkTargets(linked(text.substringBefore(", parked"), setOf(579, 390))))
    }

    /** Not a task: each allowlist entry says why. */
    private val allowed = listOf(
        "#\${x.id} " to "debt id; the debt's task is drawn by LinkedText next to it",
        "#\${f.id} [" to "experiment finding id, not a task",
        "issue #" to "tracker issue number, with its own URL link",
        "PR #" to "pull request number, with its own URL link",
        "Batch #\${b.id} " to "batch id, not a task",
        "Loading task #" to "the page's own task while it loads; nowhere to link to",
        "Text(\"#\${it.id} \"" to "debt id (the debt's task is drawn by LinkedText beside it)",
        "Event log at #" to "event-log sequence number, not a task",
        "#\${p.number}" to "PR number; the pull request has its own URL link",
        "Edit proposal #" to "title of the dialog for the proposal being edited, its own task",
    )

    private val risky = Regex("""Text\(.*(#\$|#\$\{|why_waiting|batchText\(|QueueWait\.lease|leaseDetail|QueueWait\.line|\.chain\()""")

    @Test fun noTextPrintsATaskNumberAsPlainText() {
        val dir = listOf("src/main/kotlin/mason/monitor/ui", "app/src/main/kotlin/mason/monitor/ui")
            .map { java.io.File(it) }.first { it.isDirectory }
        val bad = mutableListOf<String>()
        for (f in dir.listFiles()!!.filter { it.name.endsWith(".kt") }) {
            val lines = f.readLines()
            lines.forEachIndexed { i, line ->
                val t = line.trim()
                if (t.startsWith("LinkedText(") || t.startsWith("//") || t.startsWith("*")) return@forEachIndexed
                if (risky.containsMatchIn(t) && allowed.none { t.contains(it.first) } && !t.contains("LinkedText(")) {
                    bad += "${f.name}:${i + 1}: $t"
                }
            }
        }
        assertTrue(bad.isEmpty(), "show these through LinkedText, or allowlist them with a reason:\n" + bad.joinToString("\n"))
    }
}
