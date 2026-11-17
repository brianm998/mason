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
        "compilation" -> "Compilation"
        "exploring" -> "Exploring"
        "developing" -> "Developing"
        "testing" -> "Testing"
        "review" -> "Review"
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
    if (!canAct) return
    skippableRows(rows).forEach { skipTest(it.id, it.step) }
}
