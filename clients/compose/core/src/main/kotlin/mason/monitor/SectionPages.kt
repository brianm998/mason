// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import mason.v1.*

/**
 * What the section detail pages ([Page.Section]) read. They draw the same view-model rows as
 * the sidebar (project-scoped, in the daemon's order: not waiting first, then age).
 */
object SectionPages {
    fun title(name: String): String = when (name) {
        "active_models" -> "Active models"
        "landing" -> "Landing queue"
        "queued" -> "Queued"
        "cpu" -> "CPU"
        "disk" -> "Disk"
        "github" -> "GitHub"
        "compilation" -> "Compilation"
        "exploring" -> "Exploring"
        "developing" -> "Developing"
        "testing" -> "Testing"
        "review" -> "Review"
        "needs_attention" -> "Needs you"
        "proposed" -> "Proposed"
        "failed" -> "Failed"
        else -> name
    }

    /** "12 of 40 suites" for a task whose running step counts, else "". */
    fun suiteProgress(row: TaskRow): String {
        val c = row.progress.running ?: return ""
        if (!c.hasRealTotal) return ""
        return "${c.done} of ${c.total} ${c.unit.ifEmpty { "suites" }}"
    }
}

/** The task rows of a stage's page, in the sidebar's order. */
fun MonitorModel.sectionRows(name: String): List<TaskRow> =
    sidebar?.sections?.firstOrNull { it.state == name }?.rows.orEmpty()

/** The rows Skip testing applies to: those whose actions include it. */
fun MonitorModel.skippableRows(rows: List<TaskRow>): List<TaskRow> = rows.filter { "skip_test" in it.allowed_actions }

/** Skip testing for every row that allows it: the sidebar's command, once per task. */
fun MonitorModel.skipAllTests(rows: List<TaskRow>) {
    skipDisabledReason?.let { skipNotice = it; return }
    val targets = skippableRows(rows)
    if (targets.isEmpty()) {
        skipNotice = "Nothing to skip: no task here allows it"
        return
    }
    // An empty step is sent as it is: the daemon reads it as the task's test step.
    targets.forEach { skipTest(it.id, it.step) }
    skipNotice = "Skipped testing for ${targets.size} task${if (targets.size == 1) "" else "s"}"
}

/** Why Skip testing cannot be used, or null when it can; the button is disabled with this beside it. */
val MonitorModel.skipDisabledReason: String?
    get() = readOnlyReason?.let { "Read-only: $it" } ?: if (!isOwner) "Only the owner can skip testing" else null

/** The count for a capped section page's header ("500 of 2000"), else null. */
fun MonitorModel.sectionCountText(name: String): String? {
    val s = sidebar?.sections?.firstOrNull { it.state == name } ?: return null
    return if (isCapped(s)) countText(s) else null
}
