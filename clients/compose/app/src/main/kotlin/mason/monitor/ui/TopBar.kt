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
    var voices by remember { mutableStateOf(false) }
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
                // The most constrained window ("claude 5-hour 82%, resets 1 h 10 m"), else the sessions.
                val line = model.usage?.top_bar?.firstOrNull { it.startsWith("${p.name} ") }
                    ?: "${p.name}: ${p.sessions_used} of ${p.sessions_limit} sessions"
                Text(line, style = theme.small(), maxLines = 1, modifier = Modifier.testTag("topbar-usage:${p.name}"))
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
        NavButton("〰", "voice-settings", true) { voices = true }
    }
    if (voices) SpeechSettingsDialog(model) { voices = false }
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
private fun updateRow(text: String, button: String?, id: String, theme: Theme, action: () -> Unit) {
    Row(
        modifier = Modifier.fillMaxWidth().background(theme.slow.copy(alpha = 0.18f))
            .padding(horizontal = theme.spacingLarge, vertical = theme.spacing).testTag("$id-banner"),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(text, style = theme.body(FontWeight.Medium), modifier = Modifier.weight(1f))
        if (button != null) Button(onClick = action, modifier = Modifier.testTag(id)) { Text(button) }
    }
}

@Composable
fun BannersView(model: MonitorModel) {
    val theme = LocalTheme.current
    Column {
        if (model.announcer.daemonDown) banner("Daemon down: MASON has lost its daemon. Check that masond is running.", "daemon-down", theme)
        if (!model.connected) banner(if (model.reconnecting) "Daemon restarting, reconnecting…" else "Connection lost. Reconnecting…", "connection", theme)
        model.readOnlyReason?.let { banner("Read-only: $it", "read-only", theme) }
        if (model.daemonUpdateText.isNotEmpty()) {
            updateRow(model.daemonUpdateText, if (model.canUpdateDaemon) "Update masond" else null, "daemon-update", theme) { model.updateDaemon() }
        }
        val monitorText = model.updating
            ?: model.updateError
            ?: if (model.updateAvailable) "A newer monitor is available (${model.latestMonitor.take(8)})." else null
        if (monitorText != null) {
            updateRow(monitorText, if (model.updateAvailable && model.updating == null) "Update monitor" else null, "monitor-update", theme) { model.updateMonitor() }
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

private val KIND_LABELS = listOf(
    "needs_you" to "Needs a decision", "failed" to "Task failed", "landing" to "Landing frozen",
    "daemon_down" to "Daemon down", "credentials" to "Credentials failing",
    "provider_outage" to "Provider outage", "landed" to "Landed",
    "disk" to "Disk nearly full", "environment" to "Environment problem", "blocked" to "Task blocked", "host_lost" to "Host lost", "runtime" to "Runtime update failed", "definition" to "Definition refused", "tracker" to "Tracker problem", "provider_limit" to "Provider limit", "provider_overage" to "Provider overage", "provider_budget" to "Provider budget",
)
private val SOUND_NAMES = listOf("Glass", "Basso", "Tink", "Ping", "Pop", "Submarine", "Sosumi", "Funk")

/** Which conditions make noise, which sound each family uses, and which OS voice speaks (Part 19). */
@Composable
private fun SpeechSettingsDialog(model: MonitorModel, onClose: () -> Unit) {
    val theme = LocalTheme.current
    val a = model.announcer
    val s = a.settings
    AlertDialog(
        onDismissRequest = onClose,
        title = { Text("Sound and speech", style = theme.title()) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(theme.spacingSmall)) {
                KIND_LABELS.forEach { (kind, label) ->
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Checkbox(
                            checked = kind in s.kinds,
                            onCheckedChange = { on -> a.update(s.copy(kinds = if (on) s.kinds + kind else s.kinds - kind)) },
                            modifier = Modifier.testTag("speech-kind:$kind"),
                        )
                        Text(label, style = theme.body())
                    }
                }
                listOf("decision" to "Wants a decision", "broke" to "Broke", "info" to "Other").forEach { (family, label) ->
                    Picker(label, s.sounds[family] ?: "Tink", SOUND_NAMES, "speech-sound:$family") {
                        a.update(s.copy(sounds = s.sounds + (family to it)))
                    }
                }
                Picker("Voice", s.voice.ifEmpty { "System default" }, listOf("System default") + a.availableVoices(), "speech-voice") {
                    a.update(s.copy(voice = if (it == "System default") "" else it))
                }
                a.noChannelLine?.let { Text(it, style = theme.small()) }
                Text(
                    if (a.isSpeaker) "This monitor is the one that speaks." else "Another monitor speaks, or none is needed.",
                    style = theme.small(),
                )
            }
        },
        confirmButton = { TextButton(onClick = onClose) { Text("Done") } },
    )
}

@Composable
private fun Picker(label: String, current: String, options: List<String>, tag: String, onPick: (String) -> Unit) {
    val theme = LocalTheme.current
    var open by remember { mutableStateOf(false) }
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(theme.spacing)) {
        Text(label, style = theme.body())
        Box {
            TextButton(onClick = { open = true }, modifier = Modifier.testTag(tag)) { Text(current) }
            DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
                options.forEach { o -> DropdownMenuItem(text = { Text(o) }, onClick = { onPick(o); open = false }) }
            }
        }
    }
}
