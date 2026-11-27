// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor.ui

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import mason.monitor.AddProjectStep
import mason.monitor.AddProjectWizard
import mason.monitor.MonitorModel
import mason.monitor.landChoices

/** Add Project: a git repository on the daemon's machine, taken into MASON in five steps. */
@Composable
internal fun AddProjectDialog(model: MonitorModel, wizard: AddProjectWizard) {
    val theme = LocalTheme.current
    AlertDialog(
        onDismissRequest = { model.closeAddProject() },
        modifier = Modifier.testTag("add-project-dialog").widthIn(min = 640.dp),
        properties = androidx.compose.ui.window.DialogProperties(usePlatformDefaultWidth = false),
        title = {
            Column {
                Text("Add Project", style = theme.title())
                Text(
                    AddProjectStep.entries.joinToString("  ›  ") { if (it == wizard.step) "[${it.title}]" else it.title },
                    style = theme.small(color = theme.muted), modifier = Modifier.testTag("add-project-steps"),
                )
            }
        },
        text = { AddProjectStepView(model, wizard) },
        confirmButton = { AddProjectButtons(model, wizard) },
    )
}

/** The page of the current step. Separate from the dialog so a snapshot can draw it alone. */
@Composable
internal fun AddProjectStepView(model: MonitorModel, w: AddProjectWizard) {
    val theme = LocalTheme.current
    Column(
        Modifier.heightIn(max = 460.dp).verticalScroll(rememberScrollState()).testTag("add-project-step:${w.step.name.lowercase()}"),
        verticalArrangement = Arrangement.spacedBy(theme.spacing),
    ) {
        when (w.step) {
            AddProjectStep.REPOSITORY -> RepositoryStep(model, w)
            AddProjectStep.DETECTED -> {
                Text("What MASON found", style = theme.body(FontWeight.SemiBold))
                w.plan?.detected?.forEach { (k, v) ->
                    Row(horizontalArrangement = Arrangement.spacedBy(theme.spacing)) {
                        Text(k, style = theme.body(color = theme.muted), modifier = Modifier.width(110.dp))
                        Text(v, style = theme.body())
                    }
                }
                if (w.plan?.kind == "adopt") Text("This repository already has a .mason directory; it will be adopted.", style = theme.small(color = theme.muted))
            }
            AddProjectStep.QUESTIONS -> {
                val q = w.plan?.next
                if (q == null) Text("No more questions.", style = theme.body())
                else {
                    Text(q.text, style = theme.body(FontWeight.SemiBold), modifier = Modifier.testTag("add-project-question"))
                    if (q.options.isEmpty()) {
                        var text by remember(q.id) { mutableStateOf(q.default) }
                        OutlinedTextField(text, { text = it }, singleLine = true, modifier = Modifier.fillMaxWidth().testTag("add-project-answer-text"))
                        Pill("Answer", prominent = true, enabled = !w.busy) { model.addProjectAnswer(q, text) }
                    } else {
                        q.options.forEach { o ->
                            Pill(if (o == q.default) "$o (recommended)" else o, prominent = o == q.default, enabled = !w.busy,
                                modifier = Modifier.testTag("add-project-option:$o")) { model.addProjectAnswer(q, o) }
                        }
                    }
                    Pill("Use recommended for the rest", enabled = !w.busy, modifier = Modifier.testTag("add-project-recommended")) { model.addProjectUseRecommended() }
                }
            }
            AddProjectStep.REVIEW -> ReviewStep(w)
            AddProjectStep.APPLY -> {
                w.progress.forEach { Text(it, style = theme.small(), fontFamily = FontFamily.Monospace) }
                when {
                    w.busy -> Text("Working…", style = theme.body(color = theme.muted), modifier = Modifier.testTag("add-project-working"))
                    w.appliedId != null -> Text("Added ${w.appliedId}.", style = theme.body(FontWeight.SemiBold), modifier = Modifier.testTag("add-project-done"))
                }
                (w.applyWarnings + w.applyNotes).forEach { Text(it, style = theme.small(color = theme.attention), modifier = Modifier.testTag("add-project-warning")) }
            }
        }
        w.error?.let { Text(it, style = theme.body(color = theme.attention), modifier = Modifier.testTag("add-project-error")) }
    }
}

@Composable
private fun RepositoryStep(model: MonitorModel, w: AddProjectWizard) {
    val theme = LocalTheme.current
    Text("Which repository?", style = theme.body(FontWeight.SemiBold))
    if (w.candidates.isEmpty()) {
        Text("No candidate repositories found under the daemon's roots. Type a path on the daemon's machine.", style = theme.small(color = theme.muted))
    }
    w.candidates.forEach { c ->
        Pill(c.name + if (c.hasMason) "  (has .mason)" else "", prominent = w.path == c.path, modifier = Modifier.testTag("add-project-candidate:${c.name}")) { w.path = c.path }
        Text(listOfNotNull(c.path, c.remote).joinToString("  ·  "), style = theme.small(color = theme.muted))
    }
    w.candidateErrors.forEach { Text(it, style = theme.small(color = theme.attention)) }
    Row(horizontalArrangement = Arrangement.spacedBy(theme.spacing), verticalAlignment = androidx.compose.ui.Alignment.CenterVertically) {
        OutlinedTextField(w.path, { w.path = it }, label = { Text("Path") }, singleLine = true, modifier = Modifier.weight(1f).testTag("add-project-path"))
        val choose = model.chooseDirectory
        if (choose != null) Pill("Choose…", modifier = Modifier.testTag("add-project-choose")) { choose()?.let { w.path = it } }
    }
}

@Composable
private fun ReviewStep(w: AddProjectWizard) {
    val theme = LocalTheme.current
    Text("Files MASON will write", style = theme.body(FontWeight.SemiBold))
    w.plan?.files?.forEach { f ->
        Text(f.path, style = theme.small(FontWeight.SemiBold), modifier = Modifier.testTag("add-project-file:${f.path}"))
        Text(f.contents, style = theme.small(), fontFamily = FontFamily.Monospace)
    }
    w.plan?.warnings?.forEach { Text(it, style = theme.small(color = theme.attention), modifier = Modifier.testTag("add-project-warning")) }
    Text("Land the setup branch?", style = theme.body(FontWeight.SemiBold))
    Row(horizontalArrangement = Arrangement.spacedBy(theme.spacingSmall)) {
        landChoices.forEach { c -> Pill(c, prominent = w.land == c, modifier = Modifier.testTag("add-project-land:$c")) { w.land = c } }
    }
    Row(verticalAlignment = androidx.compose.ui.Alignment.CenterVertically) {
        Checkbox(w.prove, { w.prove = it }, modifier = Modifier.testTag("add-project-prove"))
        Text("File a proof task to show the pipeline works", style = theme.body())
    }
}

@Composable
private fun AddProjectButtons(model: MonitorModel, w: AddProjectWizard) {
    val theme = LocalTheme.current
    Row(horizontalArrangement = Arrangement.spacedBy(theme.spacing)) {
        val done = w.step == AddProjectStep.APPLY && !w.busy
        if (w.step != AddProjectStep.REPOSITORY && w.step != AddProjectStep.APPLY) TextButton(onClick = { model.addProjectBack() }, modifier = Modifier.testTag("add-project-back")) { Text("Back") }
        TextButton(onClick = { model.closeAddProject() }, modifier = Modifier.testTag("add-project-cancel")) { Text(if (done && w.appliedId != null) "Close" else "Cancel") }
        when (w.step) {
            AddProjectStep.REPOSITORY -> TextButton(enabled = w.path.isNotBlank() && !w.busy, onClick = { model.addProjectPlan() }, modifier = Modifier.testTag("add-project-next")) { Text("Next") }
            AddProjectStep.DETECTED -> TextButton(onClick = { w.continueFromDetected() }, modifier = Modifier.testTag("add-project-next")) { Text("Next") }
            AddProjectStep.QUESTIONS -> {}
            AddProjectStep.REVIEW -> TextButton(enabled = w.plan?.errors?.isEmpty() != false, onClick = { model.addProjectApply() }, modifier = Modifier.testTag("add-project-apply")) { Text("Add project") }
            AddProjectStep.APPLY -> if (done && w.appliedId == null) TextButton(onClick = { w.reviewAgain() }, modifier = Modifier.testTag("add-project-retry")) { Text("Back to review") }
        }
    }
}
