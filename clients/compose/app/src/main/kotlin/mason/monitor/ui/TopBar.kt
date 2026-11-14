// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor.ui

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import mason.monitor.MonitorModel

/**
 * The top bar, on every page: home over back and forward; the attention indicator (only
 * while something needs a look) over the project filter; LLM usage (5-hour over weekly); one
 * count tile per state; a 2x2 grid of add task, improve, mute, voice.
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
        // Home on top; back and forward in a row under it.
        Column(verticalArrangement = Arrangement.spacedBy(theme.spacingSmall)) {
            NavButton("⌂ Home", "nav:home", true) { model.goHome() }
            Row(horizontalArrangement = Arrangement.spacedBy(theme.spacingSmall)) {
                NavButton("←", "nav:back", model.canGoBack) { model.goBack() }
                NavButton("→", "nav:forward", model.canGoForward) { model.goForward() }
            }
        }

        // The indicator (only while something needs a look) over the project filter; the
        // filter stands alone, centred, when nothing does.
        Column(verticalArrangement = Arrangement.spacedBy(theme.spacingSmall, Alignment.CenterVertically)) {
            if (model.attention > 0) {
                // Clicking it opens the first task that needs the owner.
                Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.clickable { model.openFirstNeedingYou() }.testTag("attention")) {
                    StateIcon("needs_attention")
                    Spacer(Modifier.width(theme.spacingSmall))
                    Text(
                        "${model.attention} need${if (model.attention == 1) "s" else ""} you",
                        style = theme.body(FontWeight.SemiBold, theme.attention), maxLines = 1, softWrap = false,
                    )
                }
            }
            ProjectFilter(model)
        }

        Column(Modifier.weight(1f, fill = true)) {
            model.usage?.providers?.forEach { p ->
                // 5-hour above weekly; the first line keeps the account's tag, the rest add a number.
                topBarUsageLines(model.usage, p).forEachIndexed { i, line ->
                    Text(line, style = theme.small(), maxLines = 1, modifier = Modifier.testTag(if (i == 0) "topbar-usage:${p.name}" else "topbar-usage:${p.name}:$i"))
                }
            }
        }

        // Tiles are fixed size: a narrow window squeezes the text above, not these.
        Row(horizontalArrangement = Arrangement.spacedBy(theme.spacingSmall)) {
            model.header?.counts?.forEach { CountTile(model, it) }
            model.header?.pending_issues?.size?.takeIf { it > 0 }?.let { PendingTile(it) }
        }

        // A 2x2 grid: add task and improve on top, mute and voice under. With no self project
        // the wand's cell stays empty so the others keep their places.
        Column(verticalArrangement = Arrangement.spacedBy(theme.spacingSmall)) {
            Row(horizontalArrangement = Arrangement.spacedBy(theme.spacingSmall)) {
                NavButton("+", "add-task", true) { adding = true }
                if (model.selfProject.isNotEmpty()) NavButton("✨", "improve-pipeline", true) { improving = true }
                else Box(Modifier.alpha(0f).clearAndSetSemantics {}) { NavButton("✨", "improve-pipeline-empty", false) {} }
            }
            Row(horizontalArrangement = Arrangement.spacedBy(theme.spacingSmall)) {
                NavButton(if (model.muted) "🔇" else "🔊", "mute", true) { model.toggleMute() }
                NavButton("〰", "voice-settings", true) { voices = true }
            }
        }
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
private fun updateRow(text: String, button: String?, id: String, theme: Theme, icon: String? = null, action: () -> Unit) {
    Row(
        modifier = Modifier.fillMaxWidth().background(theme.slow.copy(alpha = 0.18f))
            .padding(horizontal = theme.spacingLarge, vertical = theme.spacing).testTag("$id-banner"),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        BannerImage(icon, theme)
        Text(text, style = theme.body(FontWeight.Medium), modifier = Modifier.weight(1f))
        if (button != null) Button(onClick = action, modifier = Modifier.testTag(id)) { Text(button) }
    }
}

@Composable
fun BannersView(model: MonitorModel) {
    val theme = LocalTheme.current
    Column {
        if (model.announcer.daemonDown) banner("Daemon down: MASON has lost its daemon. Check that masond is running.", "daemon-down", theme, "daemon_down")
        if (!model.connected) banner(if (model.reconnecting) "Daemon restarting, reconnecting…" else "Connection lost. Reconnecting…", "connection", theme, "connection_lost")
        model.readOnlyReason?.let { banner("Read-only: $it", "read-only", theme, "read_only") }
        if (model.daemonUpdateText.isNotEmpty()) {
            updateRow(model.daemonUpdateText, if (model.canUpdateDaemon) (if (model.downloadUrl.isEmpty()) "Update masond" else "Download") else null, "daemon-update", theme) {
                if (model.downloadUrl.isEmpty()) model.updateDaemon() else model.openDownloadPage()
            }
        }
        val monitorText = model.updating
            ?: model.updateError
            ?: if (model.updateAvailable) "A newer monitor is available (${model.latestMonitor.take(8)})." else null
        if (monitorText != null) {
            updateRow(monitorText, if (model.updateAvailable && model.updating == null) "Update monitor" else null, "monitor-update", theme, "update") { model.updateMonitor() }
        }
        model.header?.banners?.forEachIndexed { i, b ->
            if (b.kind == "push_auth") {
                // A refused push holds landing; the owner picks the key that fixes it.
                val project = MonitorModel.pushAuthProject(b)
                updateRow(b.text, if (model.canAct) "Choose SSH key…" else null, "push-auth:$project", theme, BannerIcons.forDaemonKind(b.kind)) {
                    model.chooseSshKey(project)
                }
            } else {
                banner(b.text, "banner:$i", theme, BannerIcons.forDaemonKind(b.kind))
            }
        }
        if (model.sshKeyProject != null) SshKeyDialog(model)
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

/** The decorative image at the start of a banner; nothing for an unknown id. */
@Composable
private fun BannerImage(id: String?, theme: Theme) {
    val bmp = remember(id) { id?.let { Mascot.bannerIcon(it) } } ?: return
    val size = theme.mascotBannerIcon
    Image(bmp, contentDescription = null, modifier = Modifier.padding(end = theme.spacing).size(size))
}

@Composable
private fun banner(text: String, tag: String, theme: Theme, icon: String? = null) {
    Row(
        modifier = Modifier.fillMaxWidth().background(theme.slow.copy(alpha = 0.18f))
            .padding(horizontal = theme.spacingLarge, vertical = theme.spacing).testTag(tag),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        BannerImage(icon, theme)
        Text(text, style = theme.body(FontWeight.Medium))
    }
}

@Composable
internal fun AddTaskDialog(model: MonitorModel, onClose: () -> Unit) {
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
    "provider_outage" to "Provider outage", "stalled" to "Pipeline stalled", "landed" to "Landed",
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

/**
 * After a refused push: the keys the daemon could use, with what is known about each. Choosing one
 * writes the repository's ssh command, unlocks the key if it has a passphrase (in a dialog on the
 * daemon's Mac, never here) and tries the remote.
 */
@Composable
internal fun SshKeyDialog(model: MonitorModel) {
    val theme = LocalTheme.current
    val project = model.sshKeyProject ?: return
    AlertDialog(
        onDismissRequest = { model.closeSshKeys() },
        modifier = Modifier.testTag("ssh-key-sheet"),
        title = { Text("Choose the SSH key for $project", style = theme.title()) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(theme.spacing)) {
                val keys = model.sshKeys
                if (keys == null) {
                    if (model.sshKeyResult == null) Text("Looking for keys…", style = theme.body(color = theme.muted))
                } else {
                    if (keys.keys.isEmpty()) Text("No keys were found in ~/.ssh on the daemon's Mac.", style = theme.body(color = theme.muted))
                    keys.keys.forEach { k ->
                        Row(horizontalArrangement = Arrangement.spacedBy(theme.spacing), verticalAlignment = Alignment.CenterVertically) {
                            if (k.encrypted) Text("🔒", modifier = Modifier.semantics { contentDescription = "has a passphrase" })
                            Column(Modifier.weight(1f)) {
                                Text(k.path.ifEmpty { "(only in the agent)" }, style = theme.body(FontWeight.Medium))
                                Text("${k.fingerprint}  ${k.comment}", style = theme.small(color = theme.muted))
                            }
                            if (k.onOwnerAccount) Text("on the repository owner's account", style = theme.small(color = theme.accent))
                            k.seenAs?.let { Text("seen as $it", style = theme.small(color = theme.muted)) }
                            TextButton(
                                enabled = k.usable && !model.sshKeyBusy,
                                onClick = { model.useSshKey(project, k.path) },
                                modifier = Modifier.testTag("ssh-key-use:${k.id}"),
                            ) { Text("Use") }
                        }
                    }
                    if (model.sshKeyBusy) {
                        val host = keys.host.ifEmpty { "the daemon's Mac" }
                        Text("Enter the passphrase in the dialog on $host if it asks.", style = theme.small())
                    }
                }
                model.sshKeyResult?.let { Text(it, style = theme.small(color = theme.attention)) }
            }
        },
        confirmButton = { TextButton(onClick = { model.closeSshKeys() }) { Text("Close") } },
    )
}

/** The top-bar lines for an account, in the order received (5-hour, then weekly); its sessions when it has none. */
fun topBarUsageLines(usage: mason.v1.UsageView?, p: mason.v1.ProviderStatus): List<String> =
    usage?.top_bar.orEmpty().filter { it.startsWith("${p.name} ") }
        .ifEmpty { listOf("${p.name}: ${p.sessions_used} of ${p.sessions_limit} sessions") }
