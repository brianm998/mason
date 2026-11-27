// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor.ui

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.TooltipArea
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
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
import androidx.compose.ui.semantics.onClick
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.layout.ContentScale
import mason.monitor.busiestState
import mason.monitor.topBarMascot
import mason.monitor.topBarMascotTooltip
import mason.monitor.topBarMascotClicked
import mason.monitor.MonitorModel
import mason.monitor.NoticeText
import androidx.compose.ui.text.style.TextOverflow

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
        TopBarMascot(model)
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
            InfoStatus(model)
            ProjectFilter(model)
            // Desktop monitors only; the Android app draws its own bar and never this one.
            if (model.canAct && model.addProjectAvailable) {
                NavButton("+", "add-project", true) { model.openAddProject() }
            }
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

/** 85% of the illustration size, or the bar's height if that is less. */
fun topBarMascotSide(illustration: Dp, bar: Dp): Dp = minOf(illustration * 0.85f, bar)

/** The picture for the state holding the most tasks; click reveals its sidebar section. */
@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun TopBarMascot(model: MonitorModel) {
    val theme = LocalTheme.current
    val choice = model.topBarMascot ?: return
    val bmp = remember(choice.id) { Mascot.illustration(choice.id) } ?: return
    val tip = model.topBarMascotTooltip
    val side = topBarMascotSide(theme.illustration, theme.tileHeight * 2 + theme.spacingSmall)
    TooltipArea(tooltip = {
        Text(tip, style = theme.small(color = theme.text), modifier = Modifier.background(theme.surface).padding(theme.spacing))
    }) {
        Image(
            bmp, contentDescription = null, contentScale = ContentScale.Fit,
            modifier = Modifier.size(side).clickable { model.topBarMascotClicked() }
                .testTag("topbar-mascot:${choice.id}").semantics { contentDescription = tip },
        )
    }
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
private fun updateRow(
    model: MonitorModel, text: String, button: String?, id: String, theme: Theme, icon: String? = null,
    sep: @Composable () -> Unit = {}, detail: String = "", daemonUpdate: Boolean = false, action: () -> Unit,
) {
    sep()
    val one = NoticeText.oneLine(text, detail)
    Row(
        modifier = Modifier.fillMaxWidth().background(theme.slow.copy(alpha = 0.18f))
            .padding(horizontal = theme.spacingLarge, vertical = theme.spacing).testTag("$id-banner"),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        BannerImage(icon, theme)
        Text(one.text, style = theme.body(FontWeight.Medium), maxLines = 1, overflow = TextOverflow.Ellipsis,
            modifier = Modifier.weight(1f).semantics { contentDescription = if (one.detail.isEmpty()) one.text else "${one.text}. ${one.detail}" })
        if (button != null) Button(onClick = action, modifier = Modifier.testTag(id)) { Text(button) }
        DetailsButton(model, one, id, button, daemonUpdate, action)
    }
}

/** The label of the action that opens a task the notification names. */
private fun openTaskLabel(n: Long) = "Open task #$n"

/** The "?" button of a row with more to say, and the dialog it opens: summary, detail and the row's actions. */
@Composable
private fun DetailsButton(
    model: MonitorModel, one: NoticeText.Lines, id: String, button: String?, daemonUpdate: Boolean = false,
    action: (() -> Unit)? = null, extra: @Composable () -> Unit = {},
) {
    if (!one.hasDetails) return
    var open by remember { mutableStateOf(false) }
    IconButton(onClick = { open = true }, modifier = Modifier.testTag("details:$id").semantics { contentDescription = "Details" }) { Text("?") }
    if (open) {
        val close = { open = false }
        AlertDialog(
            onDismissRequest = close,
            modifier = Modifier.testTag("notification-detail:$id"),
            title = { Text(one.text, modifier = Modifier.testTag("notification-detail-summary")) },
            text = {
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text(one.detail, modifier = Modifier.testTag("notification-detail-text"))
                    if (daemonUpdate && model.daemonUpdateConfirm.isNotEmpty()) Text(model.daemonUpdateConfirm)
                    if (button != null && action != null) {
                        Button(onClick = { close(); action() }, modifier = Modifier.testTag("notification-detail-action")) { Text(button) }
                    }
                    extra()
                    NoticeText.taskNumbers(one.text, one.detail).forEach { n ->
                        Button(onClick = { close(); model.open("mason://task/$n") }, modifier = Modifier.testTag("open-task:$n")) { Text(openTaskLabel(n)) }
                    }
                }
            },
            confirmButton = { TextButton(onClick = close, modifier = Modifier.testTag("notification-detail-close")) { Text("Close") } },
        )
    }
}

@Composable
fun BannersView(model: MonitorModel) {
    val theme = LocalTheme.current
    // A 1 dp line between neighbouring rows (none above the first or below the last): the first
    // row to draw takes no line, each later one draws its own before itself.
    var drawn = 0
    val sep: @Composable () -> Unit = {
        val n = drawn++
        if (n > 0) Box(Modifier.fillMaxWidth().height(1.dp).background(theme.border).testTag("row-divider:$n"))
    }
    Column {
        if (model.announcer.daemonDown) banner(model, "Daemon down: MASON has lost its daemon. Check that masond is running.", "daemon-down", theme, "daemon_down", sep = sep)
        if (!model.connected) banner(model, if (model.reconnecting) "Daemon restarting, reconnecting…" else "Connection lost. Reconnecting…", "connection", theme, "connection_lost", sep = sep)
        model.readOnlyReason?.let { banner(model, "Read-only: $it", "read-only", theme, "read_only", sep = sep) }
        if (model.daemonUpdateText.isNotEmpty() && !model.daemonUpdateIsInfo) {
            updateRow(model, model.daemonUpdateText, if (model.canUpdateDaemon) (if (model.downloadUrl.isEmpty()) model.daemonUpdateButton else "Download") else null, "daemon-update", theme, sep = sep, daemonUpdate = model.canUpdateDaemon && model.downloadUrl.isEmpty()) { if (model.downloadUrl.isEmpty()) model.pressUpdateDaemon() else model.openDownloadPage() }
        }
        if (model.confirmingDaemonUpdate) {
            AlertDialog(
                onDismissRequest = { model.cancelDaemonUpdate() },
                title = { Text("Update masond now?") },
                text = { Text(model.daemonUpdateConfirm, modifier = Modifier.testTag("daemon-update-confirm-text")) },
                confirmButton = { Button(onClick = { model.confirmDaemonUpdate() }, modifier = Modifier.testTag("daemon-update-confirm")) { Text("Update now") } },
                dismissButton = { Button(onClick = { model.cancelDaemonUpdate() }, modifier = Modifier.testTag("daemon-update-keep")) { Text("Keep waiting") } },
            )
        }
        val monitorText = model.updating
            ?: model.updateError
            ?: if (model.updateAvailable) "A newer monitor is available (${model.latestMonitor.take(8)})." else null
        if (monitorText != null) {
            updateRow(model, monitorText, if (model.updateAvailable && model.updating == null) "Update monitor" else null, "monitor-update", theme, "update", sep = sep) { model.updateMonitor() }
        }
        model.header?.banners?.forEachIndexed { i, b ->
            // Info is progress or already being handled: the top bar's status line, not a row.
            if (b.level == "info") return@forEachIndexed
            if (b.kind == "push_auth") {
                // A refused push holds landing; the owner picks the key that fixes it.
                val project = MonitorModel.pushAuthProject(b)
                updateRow(model, b.text, if (model.canAct) "Choose SSH key…" else null, "push-auth:$project", theme, BannerIcons.forDaemonKind(b.kind), sep = sep, detail = b.detail) {
                    model.chooseSshKey(project)
                }
            } else if (b.kind == "health") {
                // The pipeline's health session needs the owner: the button opens its dialog.
                updateRow(model, b.text, "Open session", "health-session:${b.session}", theme, BannerIcons.forDaemonKind(b.kind), sep = sep, detail = b.detail) {
                    model.openOwnerSession(b.session)
                }
            } else if (b.kind == "paused") {
                // The whole pause group is drawn once, where the first pause is.
                if (i == model.header?.banners?.indexOfFirst { it.kind == "paused" }) PauseGroup(model, theme, sep)
            } else {
                banner(model, b.text, "banner:$i", theme, BannerIcons.forDaemonKind(b.kind), sep = sep, detail = b.detail)
            }
        }
        if (model.sshKeyProject != null) SshKeyDialog(model)
        if (model.ownerSessionDialog != null) OwnerSessionDialog(model)
        model.header?.pending_issues?.forEach { p ->
            val uri = androidx.compose.ui.platform.LocalUriHandler.current
            sep()
            Row(
                horizontalArrangement = Arrangement.spacedBy(theme.spacing),
                modifier = Modifier.fillMaxWidth().background(theme.slow.copy(alpha = 0.12f))
                    .padding(horizontal = theme.spacingLarge, vertical = theme.spacingSmall).testTag("pending:${p.project}:${p.number}"),
            ) {
                if (p.url.isNotEmpty()) Link("#${p.number}", theme.small()) { runCatching { uri.openUri(p.url) } }
                else Text("#${p.number}", style = theme.small(color = theme.text))
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

/**
 * The pauses: an expanded one is a full banner with Resume and a collapse button; collapsed ones are
 * icons, in the first expanded pause's row or alone in one compact row.
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun PauseGroup(model: MonitorModel, theme: Theme, sep: @Composable () -> Unit = {}) {
    val banners = model.header?.banners ?: return
    model.pauseRows().forEach { row ->
        sep()
        Row(
            modifier = Modifier.fillMaxWidth().background(theme.slow.copy(alpha = 0.18f))
                .padding(horizontal = theme.spacingLarge, vertical = theme.spacing)
                .testTag(if (row.expanded == null) "paused-collapsed-row" else "paused-row:${row.expanded}"),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            row.collapsed.forEach { i ->
                val b = banners[i]
                // A double click (double tap on touch) expands; one click does nothing, so the layout stays put.
                TooltipArea(tooltip = {
                    Text(b.text, style = theme.small(color = theme.text), modifier = Modifier.background(theme.surface).padding(theme.spacing))
                }) {
                    Box(
                        modifier = Modifier.testTag("paused-icon:$i")
                            .semantics { contentDescription = b.text; onClick(label = "Expand") { model.setPauseCollapsed(b.project, b.mode, false); true } }
                            .combinedClickable(onClick = {}, onDoubleClick = { model.setPauseCollapsed(b.project, b.mode, false) }),
                    ) {
                        BannerImage(BannerIcons.forDaemonKind(b.kind), theme)
                    }
                }
            }
            val e = row.expanded
            if (e != null) {
                val b = banners[e]
                Row(modifier = Modifier.weight(1f).testTag("banner:$e"), verticalAlignment = Alignment.CenterVertically) {
                    BannerImage(BannerIcons.forDaemonKind(b.kind), theme)
                    val one = NoticeText.oneLine(b.text, b.detail)
                    Text(one.text, style = theme.body(FontWeight.Medium), maxLines = 1, overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.weight(1f).semantics { contentDescription = if (one.detail.isEmpty()) one.text else "${one.text}. ${one.detail}" })
                    DetailsButton(model, one, "banner:$e", null, extra = {
                        if (model.canAct) Button(onClick = { model.resumePause(b.project, b.mode) }, modifier = Modifier.testTag("notification-detail-resume")) { Text("Resume") }
                    })
                    if (model.canAct) {
                        Button(onClick = { model.resumePause(b.project, b.mode) }, modifier = Modifier.testTag("resume-pause:$e")) { Text("Resume") }
                    }
                    TooltipArea(tooltip = {
                        Text("Collapse", style = theme.small(color = theme.text), modifier = Modifier.background(theme.surface).padding(theme.spacing))
                    }) {
                        IconButton(
                            onClick = { model.setPauseCollapsed(b.project, b.mode, true) },
                            modifier = Modifier.testTag("collapse-pause:$e").semantics { contentDescription = "Collapse" },
                        ) { Text("✕") }
                    }
                }
            }
        }
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
private fun banner(model: MonitorModel, text: String, tag: String, theme: Theme, icon: String? = null, sep: @Composable () -> Unit = {}, detail: String = "") {
    sep()
    val one = NoticeText.oneLine(text, detail)
    Row(
        modifier = Modifier.fillMaxWidth().background(theme.slow.copy(alpha = 0.18f))
            .padding(horizontal = theme.spacingLarge, vertical = theme.spacing).testTag(tag),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        BannerImage(icon, theme)
        Text(one.text, style = theme.body(FontWeight.Medium), maxLines = 1, overflow = TextOverflow.Ellipsis,
            modifier = Modifier.weight(1f).semantics { contentDescription = if (one.detail.isEmpty()) one.text else "${one.text}. ${one.detail}" })
        DetailsButton(model, one, tag, null)
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
    "provider_outage" to "Provider outage", "stalled" to "Pipeline stalled", "landed" to "Landed", "review" to "Pipeline review",
    "disk" to "Disk nearly full", "environment" to "Environment problem", "blocked" to "Task blocked", "host_lost" to "Host lost", "runtime" to "Runtime update failed", "definition" to "Definition refused", "tracker" to "Tracker problem", "provider_limit" to "Provider limit", "provider_overage" to "Provider overage", "provider_budget" to "Provider budget",
)
private val SOUND_NAMES = listOf("Glass", "Basso", "Tink", "Ping", "Pop", "Submarine", "Sosumi", "Funk")

/** Which conditions make noise, which sound each family uses, and which OS voice speaks (Part 19). */
@Composable
private fun SpeechSettingsDialog(model: MonitorModel, onClose: () -> Unit) {
    val theme = LocalTheme.current
    val a = model.announcer
    val s = a.settings
    androidx.compose.runtime.LaunchedEffect(Unit) { a.loadVoices(model.background) }
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
                Picker("Voice", s.voice.ifEmpty { "System default" }, listOf("System default") + (a.voices ?: emptyList()), "speech-voice") {
                    a.update(s.copy(voice = if (it == "System default") "" else it))
                }
                a.noChannelLine?.let { Text(it, style = theme.small()) }
                Text(
                    if (a.isSpeaker) "This monitor is the one that speaks." else "Another monitor speaks, or none is needed.",
                    style = theme.small(),
                )
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(theme.spacing)) {
                    Text("Volume", style = theme.body())
                    Slider(
                        value = s.volume.toFloat(),
                        onValueChange = { a.update(s.copy(volume = it.toDouble().coerceIn(0.0, 1.0))) },
                        valueRange = 0f..1f,
                        modifier = Modifier.weight(1f).testTag("speech-volume"),
                    )
                    Text("${Math.round(s.volume * 100)}%", style = theme.small())
                    OutlinedButton(onClick = { a.test() }, modifier = Modifier.testTag("speech-test")) { Text("Test") }
                }
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

/** The owner-session dialog (s364): the transcript, a field to write in and the End button. */
@Composable
internal fun OwnerSessionDialog(model: MonitorModel) {
    val theme = LocalTheme.current
    var draft by remember { mutableStateOf("") }
    AlertDialog(
        onDismissRequest = { model.closeOwnerSession() },
        modifier = Modifier.testTag("owner-session-sheet"),
        title = { Text(if (model.ownerSession?.kind == "health") "Pipeline health" else "Session", style = theme.title()) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(theme.spacing)) {
                model.ownerSession?.messages?.forEach { m ->
                    val who = when (m.role) { "owner" -> "you"; "agent" -> "agent"; else -> "·" }
                    Text("$who: ${m.text}", style = if (m.role == "tool") theme.small(color = theme.muted) else theme.body())
                }
                model.ownerSessionError?.let { Text(it, style = theme.small(color = theme.attention)) }
                OutlinedTextField(value = draft, onValueChange = { draft = it }, modifier = Modifier.testTag("session-input"),
                    label = { Text("Write to the agent") })
            }
        },
        confirmButton = {
            Row(horizontalArrangement = Arrangement.spacedBy(theme.spacing)) {
                TextButton(enabled = model.ownerSession?.working != true, onClick = { model.sendOwnerMessage(draft); draft = "" }) { Text("Send") }
                TextButton(onClick = { model.endOwnerSession() }) { Text("End session") }
                TextButton(onClick = { model.closeOwnerSession() }) { Text("Close") }
            }
        },
    )
}

/** What the notification bar leaves out, as one short line; a click lists it. */
@Composable
private fun InfoStatus(model: MonitorModel) {
    val theme = LocalTheme.current
    val summary = model.infoSummary
    if (summary.isEmpty()) return
    var open by remember { mutableStateOf(false) }
    Box {
        Text(
            summary, style = theme.small(color = theme.muted), maxLines = 1, softWrap = false,
            modifier = Modifier.clickable { open = true }.testTag("info-status"),
        )
        DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
            model.infoNotices.forEach { DropdownMenuItem(text = { Text(it) }, onClick = { open = false }) }
        }
    }
}
