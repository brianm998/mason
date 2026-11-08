// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import mason.v1.PlanView

/** A task's plan as text, and the rule for which parts start open. Pure, so tests cover it. */
object PlanText {
    /** Steps, files and criteria start expanded when the plan has fewer than this many steps. */
    const val EXPAND_LIMIT = 10

    fun expandedByDefault(stepCount: Int): Boolean = stepCount < EXPAND_LIMIT

    /** The label before step [i]: its own number as the agent wrote it ("0."), or a bullet; null for paragraphs. */
    fun marker(v: PlanView, i: Int): String? {
        if (!v.steps_are_list) return null
        return if (i < v.step_numbers.size) "${v.step_numbers[i]}." else "•"
    }

    /** The readable plan as plain Markdown: headings, numbered steps, bullets. An unreadable plan is returned as written. */
    fun copyAsText(v: PlanView): String {
        if (v.note.isNotEmpty()) return v.raw
        val out = mutableListOf<String>()
        if (v.summary_paragraphs.isNotEmpty()) out += "## Summary\n\n" + v.summary_paragraphs.joinToString("\n\n")
        if (v.steps.isNotEmpty() || v.lead.isNotEmpty()) {
            val s = StringBuilder("## Steps\n")
            if (v.lead.isNotEmpty()) s.append("\n").append(v.lead).append("\n")
            if (v.steps_are_list) {
                s.append("\n")
                v.steps.forEachIndexed { i, step ->
                    val m = if (i < v.step_numbers.size) "${v.step_numbers[i]}." else "-"
                    val pad = " ".repeat(m.length + 1)
                    val lines = step.split("\n")
                    s.append("$m ${lines[0]}\n")
                    lines.drop(1).forEach { l -> s.append(if (l.isEmpty()) "\n" else "$pad$l\n") }
                }
            } else {
                s.append("\n").append(v.steps.joinToString("\n\n")).append("\n")
            }
            out += s.toString().trim('\n')
        }
        if (v.files.isNotEmpty()) out += "## Files (${v.files.size})\n\n" + v.files.joinToString("\n") { "- `$it`" }
        if (v.criteria.isNotEmpty()) out += "## Acceptance criteria (${v.criteria.size})\n\n" + v.criteria.joinToString("\n") { "- [ ] $it" }
        if (v.other.isNotEmpty()) out += "## Other\n\n" + v.other.joinToString("\n\n") { "### ${it.label}\n\n${it.value_}" }
        return out.joinToString("\n\n") + "\n"
    }

    /** A piece of inline Markdown: plain text, `code` or **bold**. */
    data class Span(val text: String, val code: Boolean = false, val bold: Boolean = false)

    /** Inline code and bold only. An unclosed marker stays as written. */
    fun inline(s: String): List<Span> {
        val out = mutableListOf<Span>()
        val plain = StringBuilder()
        fun flush() { if (plain.isNotEmpty()) { out += Span(plain.toString()); plain.clear() } }
        var i = 0
        while (i < s.length) {
            if (s[i] == '`') {
                val end = s.indexOf('`', i + 1)
                if (end > i + 1) { flush(); out += Span(s.substring(i + 1, end), code = true); i = end + 1; continue }
            } else if (s.startsWith("**", i)) {
                val end = s.indexOf("**", i + 2)
                if (end > i + 2) { flush(); out += Span(s.substring(i + 2, end), bold = true); i = end + 2; continue }
            }
            plain.append(s[i]); i++
        }
        flush()
        return out
    }
}
