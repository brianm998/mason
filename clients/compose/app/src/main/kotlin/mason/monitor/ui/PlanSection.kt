// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.withStyle
import mason.monitor.PlanText
import mason.v1.PlanView
import mason.v1.TaskDetailView

/** One text item with inline code and bold rendered. Block Markdown, links and images are not. */
fun inlineMarkdown(s: String): AnnotatedString = buildAnnotatedString {
    PlanText.inline(s).forEach { sp ->
        val style = SpanStyle(
            fontFamily = if (sp.code) FontFamily.Monospace else null,
            fontWeight = if (sp.bold) FontWeight.Bold else null,
        )
        withStyle(style) { append(sp.text) }
    }
}

/** A task's plan: summary, steps, files, criteria and any extra keys, as a document. */
@Composable
fun PlanSection(d: TaskDetailView) {
    val theme = LocalTheme.current
    val v = d.plan_view
    if (v == null) {
        // An older daemon: only the text.
        SelectionContainer { Text(d.plan, style = theme.body()) }
    } else if (v.note.isNotEmpty()) {
        Text(v.note, style = theme.small())
        SelectionContainer { Text(v.raw, style = theme.mono(), modifier = Modifier.fillMaxWidth().testTag("plan-raw")) }
        CopyButton("Copy JSON", v.raw)
    } else {
        ReadablePlan(v)
    }
}

@Composable
private fun PlanItem(text: String, modifier: Modifier = Modifier) {
    val theme = LocalTheme.current
    SelectionContainer { Text(inlineMarkdown(text), style = theme.body(), modifier = modifier) }
}

@Composable
private fun PlanGroup(title: String, open: Boolean, tag: String, content: @Composable ColumnScope.() -> Unit) {
    val theme = LocalTheme.current
    var expanded by remember { mutableStateOf(open) }
    Column(verticalArrangement = Arrangement.spacedBy(theme.spacingSmall), modifier = Modifier.testTag(tag)) {
        Text(
            (if (expanded) "▾ " else "▸ ") + title,
            style = theme.body(FontWeight.SemiBold),
            modifier = Modifier.clickable { expanded = !expanded },
        )
        if (expanded) Column(
            verticalArrangement = Arrangement.spacedBy(theme.spacingSmall),
            modifier = Modifier.padding(start = theme.spacingLarge),
            content = content,
        )
    }
}

@Composable
private fun ReadablePlan(v: PlanView) {
    val theme = LocalTheme.current
    val open = PlanText.expandedByDefault(v.steps.size)
    Column(verticalArrangement = Arrangement.spacedBy(theme.spacing), modifier = Modifier.fillMaxWidth()) {
        Row(horizontalArrangement = Arrangement.spacedBy(theme.spacingLarge)) {
            Text("⧉ Copy plan", style = theme.body(color = theme.accent),
                modifier = Modifier.testTag("plan-copy-text").clickable { copyToClipboard(PlanText.copyAsText(v)) })
            CopyButton("Copy JSON", v.raw)
        }
        v.summary_paragraphs.forEach { PlanItem(it) }
        if (v.steps.isNotEmpty() || v.lead.isNotEmpty()) {
            PlanGroup(if (v.steps_are_list) "Steps (${v.steps.size})" else "Approach", open, "plan-steps") {
                if (v.lead.isNotEmpty()) PlanItem(v.lead)
                v.steps.forEachIndexed { i, step ->
                    val m = PlanText.marker(v, i)
                    if (m == null) PlanItem(step) else Row(horizontalArrangement = Arrangement.spacedBy(theme.spacingSmall)) {
                        Text(m, style = theme.body(color = theme.muted))
                        PlanItem(step, Modifier.weight(1f))
                    }
                }
            }
        }
        if (v.files.isNotEmpty()) PlanGroup("Files (${v.files.size})", open, "plan-files") {
            v.files.forEach { f ->
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(theme.spacingSmall)) {
                    SelectionContainer { Text(f, style = theme.mono(), modifier = Modifier.weight(1f)) }
                    Text("⧉", style = theme.small(), modifier = Modifier.clickable { copyToClipboard(f) })
                }
            }
        }
        if (v.criteria.isNotEmpty()) PlanGroup("Acceptance criteria (${v.criteria.size})", open, "plan-criteria") {
            v.criteria.forEach { c ->
                Row(horizontalArrangement = Arrangement.spacedBy(theme.spacingSmall)) {
                    Text("☐", style = theme.body(color = theme.muted))
                    PlanItem(c, Modifier.weight(1f))
                }
            }
        }
        if (v.other.isNotEmpty()) PlanGroup("Other", true, "plan-other") {
            v.other.forEach { e ->
                Text(e.label, style = theme.body(FontWeight.SemiBold))
                PlanItem(e.value_)
            }
        }
    }
}
