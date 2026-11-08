// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor.ui

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import mason.monitor.MonitorModel

/**
 * The top bar, on every page: back, forward, home; the attention indicator (only while
 * something needs a look); the project filter; LLM usage; one count tile per state; add
 * task; mute.
 */
@Composable
fun TopBar(model: MonitorModel) {
    val theme = LocalTheme.current
    var adding by remember { mutableStateOf(false) }
    var improving by remember { mutableStateOf(false) }
    Row(
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(theme.spacing),
        modifier = Modifier.fillMaxWidth().background(theme.surface).padding(theme.spacing),
    ) {
        NavButton("←", "nav:back", model.canGoBack) { model.goBack() }
        NavButton("→", "nav:forward", model.canGoForward) { model.goForward() }
        NavButton("⌂ Home", "nav:home", true) { model.goHome() }

        if (model.attention > 0) {
            // Clicking it opens the first task that needs the owner.
            Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.clickable { model.openFirstNeedingYou() }.testTag("attention")) {
                Box(Modifier.size(theme.cell * 0.7f).background(theme.attention, CircleShape))
                Spacer(Modifier.width(theme.spacingSmall))
                Text(
                    "${model.attention} need${if (model.attention == 1) "s" else ""} you",
                    style = theme.body(FontWeight.SemiBold, theme.attention), maxLines = 1, softWrap = false,
                )
            }
        }

        ProjectFilter(model)

        Column(Modifier.weight(1f, fill = true)) {
            model.usage?.providers?.forEach { p ->
                Text("${p.name}: ${p.sessions_used} of ${p.sessions_limit} sessions", style = theme.small(), maxLines = 1)
            }
        }

        // Tiles are fixed size: a narrow window squeezes the text above, not these.
        Row(horizontalArrangement = Arrangement.spacedBy(theme.spacingSmall)) {
            model.header?.counts?.forEach { CountTile(model, it) }
            model.header?.pending_issues?.size?.takeIf { it > 0 }?.let { PendingTile(it) }
        }

        NavButton("+", "add-task", true) { adding = true }
        if (model.selfProject.isNotEmpty()) NavButton("✨", "improve-pipeline", true) { improving = true }
        NavButton(if (model.muted) "🔇" else "🔊", "mute", true) { model.toggleMute() }
    }
    if (adding) AddTaskDialog(model) { adding = false }
    if (improving) ImproveDialog(model) { improving = false }
}

@Composable
private fun NavButton(text: String, tag: String, enabled: Boolean, onClick: () -> Unit) {
    val theme = LocalTheme.current
    val shape = RoundedCornerShape(theme.radius)
    Text(
        text,
        style = theme.body(color = if (enabled) theme.text else theme.muted.copy(alpha = 0.5f)),
        modifier = Modifier
            .background(theme.background, shape)
            .border(1.dp, theme.border, shape)
            .clickable(enabled = enabled, onClick = onClick)
            .padding(horizontal = theme.spacing, vertical = theme.spacingSmall)
            .testTag(tag),
        maxLines = 1,
        softWrap = false,
    )
}

@Composable
private fun ProjectFilter(model: MonitorModel) {
    val theme = LocalTheme.current
    var open by remember { mutableStateOf(false) }
    val shape = RoundedCornerShape(theme.radius)
    Box {
        Text(
            (model.project.ifEmpty { "All projects" }) + "  ▾",
            style = theme.body(),
            modifier = Modifier.background(theme.background, shape).border(1.dp, theme.border, shape)
                .clickable { open = true }.padding(horizontal = theme.spacing, vertical = theme.spacingSmall)
                .testTag("project-filter"),
            maxLines = 1, softWrap = false,
        )
        DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
            DropdownMenuItem(text = { Text("All projects") }, onClick = { model.project = ""; model.subscribe?.invoke(); open = false })
            model.header?.projects?.forEach { p ->
                DropdownMenuItem(text = { Text(p) }, onClick = { model.project = p; model.subscribe?.invoke(); open = false })
            }
        }
    }
}

@Composable
fun BannersView(model: MonitorModel) {
    val theme = LocalTheme.current
    Column {
        if (!model.connected) banner(if (model.reconnecting) "Daemon restarting, reconnecting…" else "Connection lost. Reconnecting…", "connection", theme)
        model.readOnlyReason?.let { banner("Read-only: $it", "read-only", theme) }
        if (model.updateAvailable || model.updating != null) {
            Row(
                modifier = Modifier.fillMaxWidth().background(theme.slow.copy(alpha = 0.18f))
                    .padding(horizontal = theme.spacingLarge, vertical = theme.spacing).testTag("monitor-update-banner"),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(
                    model.updating ?: "A newer monitor is available (${model.daemonVersion.take(8)}).",
                    style = theme.body(FontWeight.Medium),
                    modifier = Modifier.weight(1f),
                )
                if (model.updating == null) {
                    Button(onClick = { model.onUpdate?.invoke() }, modifier = Modifier.testTag("monitor-update")) { Text("Update") }
                }
            }
        }
        model.header?.banners?.forEachIndexed { i, b -> banner(b.text, "banner:$i", theme) }
        model.header?.pending_issues?.forEach { p ->
            val uri = androidx.compose.ui.platform.LocalUriHandler.current
            Row(
                horizontalArrangement = Arrangement.spacedBy(theme.spacing),
                modifier = Modifier.fillMaxWidth().background(theme.slow.copy(alpha = 0.12f))
                    .padding(horizontal = theme.spacingLarge, vertical = theme.spacingSmall).testTag("pending:${p.project}:${p.number}"),
            ) {
                Text(
                    "#${p.number}", style = theme.small(color = if (p.url.isNotEmpty()) theme.accent else theme.text),
                    modifier = Modifier.clickable(enabled = p.url.isNotEmpty()) { runCatching { uri.openUri(p.url) } },
                )
                Text(p.title, style = theme.small(), maxLines = 1)
                Text(p.reason, style = theme.small(color = theme.muted))
            }
        }
    }
}

/** How many open issues the tracker has not imported; the list is below the top bar. */
@Composable
private fun PendingTile(count: Int) {
    val theme = LocalTheme.current
    val shape = RoundedCornerShape(theme.radius)
    Column(
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.Center,
        modifier = Modifier
            .height(theme.tileHeight)
            .widthIn(min = theme.tileMinWidth)
            .background(theme.attentionTint, shape)
            .border(BorderStroke(1.dp, theme.attention), shape)
            .testTag("tile:not_imported")
            .semantics { contentDescription = "$count not imported" }
            .padding(horizontal = theme.spacing),
    ) {
        Text(count.toString(), style = theme.count(theme.attention), maxLines = 1)
        Text("not imported", style = theme.small(), maxLines = 1, softWrap = false)
    }
}

@Composable
private fun banner(text: String, tag: String, theme: Theme) {
    Text(
        text,
        style = theme.body(FontWeight.Medium),
        modifier = Modifier.fillMaxWidth().background(theme.slow.copy(alpha = 0.18f))
            .padding(horizontal = theme.spacingLarge, vertical = theme.spacing).testTag(tag),
    )
}

@Composable
private fun AddTaskDialog(model: MonitorModel, onClose: () -> Unit) {
    val theme = LocalTheme.current
    var project by remember { mutableStateOf(model.header?.projects?.firstOrNull() ?: "") }
    var title by remember { mutableStateOf("") }
    var text by remember { mutableStateOf("") }
    AlertDialog(
        onDismissRequest = onClose,
        title = { Text("Add task", style = theme.title()) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(theme.spacing)) {
                Row(horizontalArrangement = Arrangement.spacedBy(theme.spacing)) {
                    model.header?.projects?.forEach { p ->
                        Pill(p, prominent = p == project) { project = p }
                    }
                }
                OutlinedTextField(title, { title = it }, label = { Text("Title") }, singleLine = true)
                OutlinedTextField(text, { text = it }, label = { Text("Details") }, minLines = 4)
            }
        },
        confirmButton = {
            TextButton(enabled = title.isNotBlank(), onClick = { model.addTask(project, title, text); onClose() }) { Text("Add") }
        },
        dismissButton = { TextButton(onClick = onClose) { Text("Cancel") } },
    )
}

/** "Improve the pipeline": type what MASON should change about itself. */
@Composable
private fun ImproveDialog(model: MonitorModel, onClose: () -> Unit) {
    val theme = LocalTheme.current
    var text by remember { mutableStateOf("") }
    AlertDialog(
        onDismissRequest = onClose,
        title = { Text("Improve the pipeline", style = theme.title()) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(theme.spacing)) {
                Text(
                    "MASON files this as a task on its own repository. Without write access the change goes to your fork; " +
                        "nothing goes upstream until you have read the redacted version and said so.",
                    style = theme.small(),
                )
                OutlinedTextField(text, { text = it }, label = { Text("What should change?" ) }, minLines = 5, modifier = Modifier.testTag("improve:text"))
            }
        },
        confirmButton = {
            TextButton(
                enabled = text.isNotBlank() && model.selfProject.isNotEmpty(),
                onClick = { model.improve(model.selfProject, text); onClose() },
                modifier = Modifier.testTag("improve:send"),
            ) { Text("Improve") }
        },
        dismissButton = { TextButton(onClick = onClose) { Text("Cancel") } },
    )
}
