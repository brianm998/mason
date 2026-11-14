// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor.ui

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.TooltipArea
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.waitForUpOrCancellation
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.clipPath
import androidx.compose.ui.layout.layout
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import mason.monitor.CellFill
import mason.monitor.Format
import mason.monitor.MonitorModel
import mason.monitor.countText
import mason.monitor.fill
import mason.monitor.isStale
import mason.monitor.spokenLabel
import mason.monitor.timeText
import mason.monitor.tokensCompact
import mason.v1.ProgressSource
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.text.style.TextOverflow
import mason.v1.CellState
import androidx.compose.foundation.focusable
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.isShiftPressed
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.input.TextFieldValue
import mason.monitor.OpenSession
import mason.monitor.OptionSelection
import mason.monitor.OtherKey
import mason.monitor.otherAnswer
import mason.monitor.otherBoxKey
import mason.v1.Approval
import mason.v1.ProposalInfo
import mason.v1.DecisionCard
import mason.v1.ProgressCell
import mason.v1.StateCount
import mason.v1.TaskRow
import java.awt.Toolkit
import java.awt.datatransfer.StringSelection

fun copyToClipboard(s: String) {
    runCatching { Toolkit.getDefaultToolkit().systemClipboard.setContents(StringSelection(s), null) }
}

/**
 * Progress cells (Part 17): filled means finished; hatched means waiting, labelled with what for;
 * empty means not started. A running cell with a real total (reported or counted) fills in
 * proportion to it; with only a typical total from past runs it fills the same way, lighter; with
 * neither it stays an outline. A percentage is never estimated. A step running past its typical
 * time, or quiet past its usual gap, turns amber, never red: slow is not broken.
 */
@Composable
fun ProgressCells(model: MonitorModel, cells: List<ProgressCell>, slow: Boolean = false) {
    val theme = LocalTheme.current
    val now = rememberDaemonNow(model)
    Row(horizontalArrangement = Arrangement.spacedBy(theme.spacingSmall)) {
        cells.forEach { c ->
            val part = c.state == CellState.PART
            val tint = if ((slow || (part && c.isStale(now))) && part) theme.slow else theme.accent
            Cell(c.state, tint, theme.border, Modifier.size(theme.cell).semantics {
                contentDescription = c.spokenLabel
            }, if (part) c.fill else CellFill.Outline)
        }
    }
}

/** The daemon's time, re-read every second: the clock the "updated N s ago" text counts on. */
@Composable
fun rememberDaemonNow(model: MonitorModel): Long {
    var now by remember { mutableStateOf(model.daemonNow()) }
    LaunchedEffect(model) {
        while (true) {
            kotlinx.coroutines.delay(1000)
            now = model.daemonNow()
        }
    }
    return now
}

/**
 * One line for the running step: its count by the rule for numbers, its message cut to fit, and
 * how long ago it last moved. The time counts up on this device's clock between pushes (corrected
 * by the daemon's clock) and turns amber, with "no output for", past the step's usual gap. At a
 * narrow width the message is cut first. The ticking time is cleared from the accessibility tree:
 * it is not announced every second.
 */
@Composable
fun ProgressLine(model: MonitorModel, cell: ProgressCell) {
    val theme = LocalTheme.current
    val now = rememberDaemonNow(model)
    val stale = cell.isStale(now)
    val time = cell.timeText(now)
    Row(horizontalArrangement = Arrangement.spacedBy(theme.spacingSmall), modifier = Modifier.testTag("progress-line:${cell.step}")) {
        val count = cell.countText
        if (count.isNotEmpty()) Text(count, style = theme.small(color = theme.text), maxLines = 1, softWrap = false)
        val tokens = cell.tokensCompact
        if (tokens.isNotEmpty()) {
            Text(tokens, style = theme.small(color = theme.muted), maxLines = 1, softWrap = false,
                modifier = Modifier.testTag("progress-tokens:${cell.step}"))
        }
        if (cell.message.isNotEmpty()) {
            Text(cell.message, style = theme.small(color = theme.muted), maxLines = 1, overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f, fill = false))
        }
        if (time.isNotEmpty()) {
            Text(time, style = theme.small(color = if (stale) theme.slow else theme.muted), maxLines = 1, softWrap = false,
                modifier = Modifier.clearAndSetSemantics { })
        }
    }
}

/**
 * The running step's whole record on the task page: count and time, sub-counts, message, the
 * agent's todo list with each item's status, the output figures, and where each number came from.
 */
@Composable
fun ProgressDetail(model: MonitorModel, cell: ProgressCell) {
    val theme = LocalTheme.current
    Column(verticalArrangement = Arrangement.spacedBy(theme.spacingSmall), modifier = Modifier.testTag("progress:${cell.step}")) {
        ProgressLine(model, cell)
        if (cell.message.isNotEmpty()) Text(cell.message, style = theme.body())
        cell.subs.forEach { s ->
            Text("${s.name}: " + Format.countText(s.unit, s.done.toLong(), s.done_source, s.total.toLong(), s.total_source, 0), style = theme.body())
            s.jobs?.let { j ->
                Text(Format.jobsText(s.name, j), style = theme.body(color = if (j.failed > 0) theme.attention else theme.text))
            }
        }
        cell.todo.forEach { t ->
            val mark = when (t.status) { "done" -> "✓"; "doing" -> "▶"; else -> "○" }
            Row(horizontalArrangement = Arrangement.spacedBy(theme.spacingSmall),
                modifier = Modifier.semantics(mergeDescendants = true) { contentDescription = "${t.status}: ${t.text}" }) {
                Text(mark, style = theme.body())
                Text(t.text, style = theme.body(color = if (t.status == "pending") theme.muted else theme.text))
                Text(t.status, style = theme.small(color = theme.muted))
            }
        }
        if (cell.lines > 0 || cell.bytes > 0) {
            Text("output ${Format.group(cell.lines)} lines, ${Format.group(cell.bytes)} bytes", style = theme.small(color = theme.muted))
        }
        val figures = Format.tokensDetail(cell.tokens)
        if (figures.isNotEmpty()) {
            Text("tokens " + figures.joinToString(", "), style = theme.small(color = theme.muted),
                modifier = Modifier.testTag("progress-token-detail:${cell.step}"))
        }
        val sources = listOfNotNull(
            if (cell.done_source == ProgressSource.SOURCE_NONE) null else "count " + Format.sourceWords(cell.done_source, cell.history_samples),
            if (cell.total_source == ProgressSource.SOURCE_NONE) null else "total " + Format.sourceWords(cell.total_source, cell.history_samples),
        )
        if (sources.isNotEmpty()) Text(sources.joinToString("; "), style = theme.small(color = theme.muted))
    }
}

@Composable
fun Cell(state: CellState, tint: Color, border: Color, modifier: Modifier = Modifier, fill: CellFill = CellFill.Outline) {
    Canvas(modifier) {
        val r = CornerRadius(2.dp.toPx())
        val stroke = Stroke(width = 1.dp.toPx())
        when (state) {
            CellState.FILLED -> drawRoundRect(tint, cornerRadius = r)
            CellState.PART -> {
                drawRoundRect(tint, cornerRadius = r, style = stroke)
                val clip = Path().apply { addRoundRect(androidx.compose.ui.geometry.RoundRect(0f, 0f, size.width, size.height, r)) }
                when (fill) {
                    is CellFill.Proportional -> clipPath(clip) { drawRect(tint, size = Size(size.width * fill.fraction, size.height)) }
                    // Lighter than a real count: this is the typical figure, not a measured one.
                    is CellFill.Typical -> clipPath(clip) { drawRect(tint.copy(alpha = 0.4f), size = Size(size.width * fill.fraction, size.height)) }
                    CellFill.Outline -> {}
                }
            }
            CellState.HATCHED -> {
                drawRoundRect(tint, cornerRadius = r, style = stroke)
                clipPath(Path().apply { addRoundRect(androidx.compose.ui.geometry.RoundRect(0f, 0f, size.width, size.height, r)) }) {
                    var x = -size.height
                    while (x < size.width) {
                        drawLine(tint, Offset(x, size.height), Offset(x + size.height, 0f), strokeWidth = 1.dp.toPx())
                        x += 4.dp.toPx()
                    }
                }
            }
            else -> drawRoundRect(border, cornerRadius = r, style = stroke)
        }
    }
}

/**
 * The colour of a tile's number: the attention colour when the state wants the owner, gray when
 * the count is zero, else the text colour. The dash shown while disconnected is plain text.
 */
fun tileCountColor(theme: Theme, count: Int, wants: Boolean, connected: Boolean): Color =
    if (wants) theme.attention else if (connected && count == 0) theme.muted else theme.text

/**
 * A count tile: a large count over a one-line label, sized to its label so a narrow window
 * squeezes other text and never a tile or label; tinted when the state wants the owner;
 * a dash while disconnected.
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
fun CountTile(model: MonitorModel, count: StateCount) {
    val theme = LocalTheme.current
    val wants = count.wants_you && model.connected
    val label = model.tileLabel(count)
    val shape = RoundedCornerShape(theme.radius)
    TooltipArea(tooltip = {
        Text(model.tileTooltip(count), style = theme.small(color = theme.text), modifier = Modifier.background(theme.surface).padding(theme.spacing))
    }) {
        Box {
        Column(
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.Center,
            modifier = Modifier
                .height(theme.tileHeight)
                .widthIn(min = theme.tileMinWidth)
                .background(if (wants) theme.attentionTint else theme.surface, shape)
                .border(BorderStroke(1.dp, if (wants) theme.attention else theme.border), shape)
                .pointerInput(count.state) {
                    // Reveal on the first release; a second press within the AWT multi-click
                    // interval counts as a double. Nothing waits to tell them apart.
                    val interval = (java.awt.Toolkit.getDefaultToolkit().getDesktopProperty("awt.multiClickInterval") as? Int ?: 500).toLong()
                    var last = 0L
                    var clicks = 0
                    awaitEachGesture {
                        awaitFirstDown()
                        val now = System.currentTimeMillis()
                        clicks = if (now - last <= interval) clicks + 1 else 1
                        last = now
                        val up = waitForUpOrCancellation()
                        if (up != null) {
                            model.tileClicked(count.state, clicks)
                            if (clicks >= 2) clicks = 0
                        }
                    }
                }
                .testTag("tile:${count.state}")
                .semantics { contentDescription = "${model.tileCount(count)} $label" }
                .padding(horizontal = theme.spacing),
        ) {
            Text(model.tileCount(count), style = theme.count(tileCountColor(theme, count.count, wants, model.connected)), maxLines = 1)
            Text(label, style = theme.small(), maxLines = 1, softWrap = false)
        }
        // An overlay, not part of the layout: Part 17 sizes a tile by its label and text size
        // alone, so a count of any width and the icon never change it. Zero-size layout, drawn
        // outside the tile's top-left corner, and it takes no clicks.
        val out = with(androidx.compose.ui.platform.LocalDensity.current) { theme.spacingSmall.roundToPx() }
        StateIcon(
            count.state,
            Modifier.layout { measurable, constraints ->
                val placeable = measurable.measure(androidx.compose.ui.unit.Constraints())
                layout(0, 0) { placeable.place(-out, -out) }
            },
        )
        }
    }
}

@Composable
fun Pill(text: String, prominent: Boolean = false, modifier: Modifier = Modifier, onClick: () -> Unit) {
    val theme = LocalTheme.current
    val shape = RoundedCornerShape(theme.radius)
    Row(
        modifier = modifier
            .background(if (prominent) theme.accent else theme.surface, shape)
            .border(BorderStroke(if (prominent) 0.dp else 1.dp, theme.border), shape)
            .clickable(onClick = onClick)
            .padding(horizontal = theme.spacing, vertical = theme.spacingSmall),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(text, style = theme.body(if (prominent) FontWeight.SemiBold else FontWeight.Normal, if (prominent) Color.White else theme.text))
    }
}

@Composable
fun CopyButton(label: String, value: String) {
    val theme = LocalTheme.current
    Text("⧉ $label", style = theme.small(), modifier = Modifier.clickable { copyToClipboard(value) })
}

/**
 * The decision card (Part 10): the ask, why only the owner can settle it, 2 to 4 options
 * with one recommended, "Other" with a text box, and the evidence. An answer carries the
 * card's hash, so a late click on a stale card is refused.
 */
@OptIn(androidx.compose.ui.ExperimentalComposeUiApi::class)
@Composable
fun DecisionCardView(model: MonitorModel, task: Long, card: DecisionCard) {
    val theme = LocalTheme.current
    var other by remember { mutableStateOf(TextFieldValue("")) }
    var selection by remember(card.hash) {
        mutableStateOf(OptionSelection.starting(card.options.size, card.options.indexOfFirst { it.recommended }.takeIf { it >= 0 }))
    }
    val shape = RoundedCornerShape(theme.radius)
    val focus = remember { FocusRequester() }
    fun sendOther() {
        val text = otherAnswer(other.text) ?: return
        model.answer(task, text, card.hash)
        other = TextFieldValue("")
    }
    Column(
        verticalArrangement = Arrangement.spacedBy(theme.spacing),
        modifier = Modifier.fillMaxWidth()
            .background(theme.attentionTint, shape)
            .border(BorderStroke(1.dp, theme.attention), shape)
            .padding(theme.spacingLarge)
            // Arrow keys move the selection and Return answers with it, as in Claude Code's prompts.
            .focusRequester(focus)
            .focusable()
            .onPreviewKeyEvent { e ->
                if (!model.canAct || e.type != KeyEventType.KeyDown) return@onPreviewKeyEvent false
                when (e.key) {
                    Key.DirectionDown -> { selection = selection.down(); true }
                    Key.DirectionUp -> { selection = selection.up(); true }
                    Key.Enter, Key.NumPadEnter -> {
                        val a = selection.answer
                        if (a != null) { model.answer(task, a, card.hash); true } else false
                    }
                    else -> false
                }
            }
            .testTag("card:$task"),
    ) {
        Row(verticalAlignment = Alignment.Top, horizontalArrangement = Arrangement.spacedBy(theme.spacingSmall)) {
            StateIcon("needs_attention")
            LinkedText(model, card.ask, theme.title())
        }
        if (card.why.isNotEmpty()) LinkedText(model, card.why, theme.body(color = theme.muted))
        Column(verticalArrangement = Arrangement.spacedBy(theme.spacingSmall)) {
            card.options.forEachIndexed { i, o ->
                val label = "${i + 1}.  ${o.label}${if (o.recommended) "   recommended" else ""}"
                if (model.canAct) {
                    Pill(
                        label,
                        prominent = o.recommended,
                        modifier = Modifier.fillMaxWidth()
                            .border(BorderStroke(if (selection.index == i) 2.dp else 0.dp, theme.text), shape)
                            .testTag("card:$task:option:${i + 1}"),
                    ) { model.answer(task, (i + 1).toString(), card.hash) }
                } else {
                    Text(label, style = theme.body())
                }
            }
            if (model.canAct) Row(horizontalArrangement = Arrangement.spacedBy(theme.spacing), verticalAlignment = Alignment.Top) {
                // Multi-line: Enter sends, Shift-Enter starts a new line.
                androidx.compose.foundation.text.BasicTextField(
                    value = other,
                    onValueChange = { other = it },
                    textStyle = theme.body(),
                    singleLine = false,
                    decorationBox = { inner ->
                        Box(
                            Modifier.fillMaxWidth().background(theme.background, RoundedCornerShape(theme.radius))
                                .border(1.dp, theme.border, RoundedCornerShape(theme.radius)).padding(theme.spacing),
                        ) {
                            if (other.text.isEmpty()) Text("Other… (Enter sends, Shift-Enter for a new line)", style = theme.body(color = theme.muted))
                            inner()
                        }
                    },
                    modifier = Modifier.weight(1f)
                        .onPreviewKeyEvent { e ->
                            if (e.type == KeyEventType.KeyDown && (e.key == Key.Enter || e.key == Key.NumPadEnter)) {
                                when (otherBoxKey(e.isShiftPressed)) {
                                    OtherKey.SEND -> { sendOther(); true }
                                    OtherKey.NEWLINE -> {
                                        val t = other
                                        val text = t.text.replaceRange(t.selection.min, t.selection.max, "\n")
                                        other = TextFieldValue(text, TextRange(t.selection.min + 1))
                                        true
                                    }
                                }
                            } else false
                        }
                        .testTag("card:$task:other"),
                )
                Pill("Send", modifier = Modifier.testTag("card:$task:send")) { sendOther() }
            }
        }
        if (card.evidence.isNotEmpty()) {
            Column {
                card.evidence.forEach { Text("| $it", style = theme.mono()) }
            }
        }
        if (card.origin != "agent") {
            Text("This card was ${card.origin} because the agent's did not fit.", style = theme.small())
        }
        if (!model.canAct && model.connected) {
            Text("Only the owner can answer. You are connected as ${model.identity}.", style = theme.small(), modifier = Modifier.testTag("card:$task:not-owner"))
        }
        model.lastResult?.let { Text(it, style = theme.body(color = theme.attention), modifier = Modifier.testTag("card:$task:refused")) }
    }
}

/** What a task waiting for the owner's approval waits on, in full, with Approve and Reject. */
@Composable
fun ApprovalView(model: MonitorModel, task: Long, approval: Approval) {
    val theme = LocalTheme.current
    var reason by remember { mutableStateOf("") }
    var rejecting by remember { mutableStateOf(false) }
    val shape = RoundedCornerShape(theme.radius)
    fun reject() { model.reject(task, reason); reason = ""; rejecting = false }
    Column(
        verticalArrangement = Arrangement.spacedBy(theme.spacing),
        modifier = Modifier.fillMaxWidth()
            .background(theme.attentionTint, shape)
            .border(BorderStroke(1.dp, theme.attention), shape)
            .padding(theme.spacingLarge)
            .testTag("approval:$task"),
    ) {
        LinkedText(model, approval.waiting_for, theme.title())
        approval.lines.forEach { LinkedText(model, "• $it", theme.body()) }
        if (approval.diff_hash.isNotEmpty()) Text("Diff ${approval.diff_hash}", style = theme.mono(color = theme.muted))
        if (approval.branch.isNotEmpty()) Text("Branch ${approval.branch}", style = theme.mono(color = theme.muted))
        if (approval.diff_command.isNotEmpty()) {
            Row(horizontalArrangement = Arrangement.spacedBy(theme.spacing), verticalAlignment = Alignment.CenterVertically) {
                Text(approval.diff_command, style = theme.mono(color = theme.muted))
                CopyButton("copy", approval.diff_command)
            }
        }
        if (model.canAct) {
            Row(horizontalArrangement = Arrangement.spacedBy(theme.spacing)) {
                Pill("Approve", prominent = true, modifier = Modifier.testTag("approval:$task:approve")) { model.approve(task, approval) }
                Pill("Reject", modifier = Modifier.testTag("approval:$task:reject")) { rejecting = !rejecting }
            }
            if (rejecting) Row(horizontalArrangement = Arrangement.spacedBy(theme.spacing), verticalAlignment = Alignment.CenterVertically) {
                androidx.compose.foundation.text.BasicTextField(
                    value = reason,
                    onValueChange = { reason = it },
                    textStyle = theme.body(),
                    decorationBox = { inner ->
                        Box(
                            Modifier.fillMaxWidth().background(theme.background, RoundedCornerShape(theme.radius))
                                .border(1.dp, theme.border, RoundedCornerShape(theme.radius)).padding(theme.spacing),
                        ) {
                            if (reason.isEmpty()) Text("Why? (optional)", style = theme.body(color = theme.muted))
                            inner()
                        }
                    },
                    modifier = Modifier.weight(1f)
                        .onPreviewKeyEvent { e ->
                            if (e.type == KeyEventType.KeyDown && e.key == Key.Enter) { reject(); true } else false
                        }
                        .testTag("approval:$task:reason"),
                )
                Pill("Send rejection", modifier = Modifier.testTag("approval:$task:send-reject")) { reject() }
            }
        } else if (model.connected) {
            Text("Only the owner can approve or reject. You are connected as ${model.identity}.", style = theme.small())
        }
        model.lastResult?.let { Text(it, style = theme.body(color = theme.attention), modifier = Modifier.testTag("approval:$task:refused")) }
    }
}

/** The actions a task offers now: exactly the daemon's `allowed_actions`. */
@Composable
fun ActionButtons(model: MonitorModel, row: TaskRow, hasApprovalView: Boolean = false) {
    val theme = LocalTheme.current
    Row(horizontalArrangement = Arrangement.spacedBy(theme.spacing)) {
        // Approve and Reject have their own view wherever there is one; a non-owner sees no
        // buttons at all for what only the owner may do.
        row.allowed_actions.filter {
            // Block and unblock are drawn by BlockersView, one control per blocker.
            it != "answer" && it != "edit" && it != "block" && it != "unblock" &&
                !(hasApprovalView && (it == "approve" || it == "reject")) &&
                !(!model.canAct && (it == "approve" || it == "reject"))
        }.forEach { a ->
            Pill(label(a), prominent = a == "approve", modifier = Modifier.testTag("action:${row.id}:$a")) { run(model, row, a) }
        }
        model.sessionNotice?.let {
            Text(it, style = theme.small(color = theme.attention), modifier = Modifier.testTag("action:${row.id}:open_session:notice"))
        }
        if (model.sessionPrompt == row.id) {
            AlertDialog(
                onDismissRequest = { model.answerSessionPrompt(false) },
                text = { Text(OpenSession.CONFIRM_QUESTION, style = theme.body()) },
                confirmButton = { TextButton(onClick = { model.answerSessionPrompt(true) }) { Text("Open anyway") } },
                dismissButton = { TextButton(onClick = { model.answerSessionPrompt(false) }) { Text("Cancel") } },
            )
        }
    }
}

private fun label(a: String) = when (a) {
    "skip_test" -> "Skip test"
    "land_now" -> "Land now"
    "explore_anyway" -> "Explore anyway"
    "open_session" -> "Open session"
    else -> a.replaceFirstChar { it.uppercase() }
}

private fun run(model: MonitorModel, row: TaskRow, a: String) {
    when (a) {
        "cancel" -> model.cancel(row.id)
        "retry" -> model.retry(row.id, seen = row.last_run)
        "approve" -> model.approve(row.id)
        "revive" -> model.revive(row.id)
        "reject" -> model.reject(row.id)
        "explore_anyway" -> model.exploreAnyway(row.id)
        "land_now" -> model.landNow(row.id)
        "force" -> model.force(row.id)
        "skip_test" -> model.skipTest(row.id, row.step)
        "open_session" -> model.openSession(row.id)
    }
}

/**
 * A held task's blockers: each with its state and where it came from ("from GitHub" or "set by
 * you"), Release (and Revive for a canceled one, Retry for a failed one) for the owner, and an
 * Add blocker field with a task picker. A non-owner sees the list and no controls.
 */
@Composable
fun BlockersView(model: MonitorModel, row: TaskRow) {
    val theme = LocalTheme.current
    val canAdd = model.canAct && "block" in row.allowed_actions
    if (row.blocker_links.isEmpty() && !canAdd) return
    var open by remember { mutableStateOf(false) }
    var picked by remember { mutableStateOf<Long?>(null) }
    val candidates = model.blockerCandidates(row)
    Column(verticalArrangement = Arrangement.spacedBy(theme.spacingSmall)) {
        row.blocker_links.forEach { l ->
            Row(
                horizontalArrangement = Arrangement.spacedBy(theme.spacing),
                verticalAlignment = Alignment.CenterVertically,
                modifier = Modifier.testTag("blocker:${row.id}:${l.task}"),
            ) {
                Text("#${l.task}", style = theme.body(color = theme.accent), modifier = Modifier.clickable { model.go(mason.monitor.Page.Task(l.task)) })
                Text("${l.state}, ${l.origin}", style = theme.small())
                if (model.canAct) l.actions.forEach { a ->
                    Pill(a.replaceFirstChar { it.uppercase() }, modifier = Modifier.testTag("blocker:${row.id}:${l.task}:$a")) {
                        when (a) {
                            "release" -> model.release(row.id, l.task)
                            "revive" -> model.revive(l.task)
                            "retry" -> model.retry(l.task)
                        }
                    }
                }
            }
        }
        if (canAdd) {
            Row(horizontalArrangement = Arrangement.spacedBy(theme.spacing), verticalAlignment = Alignment.CenterVertically) {
                Box {
                    Pill(
                        candidates.firstOrNull { it.id == picked }?.let { "${it.key} ${it.title}" } ?: "Choose a task",
                        modifier = Modifier.testTag("add-blocker:${row.id}:picker"),
                    ) { open = true }
                    androidx.compose.material3.DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
                        candidates.forEach { c ->
                            androidx.compose.material3.DropdownMenuItem(
                                text = { Text("${c.key} ${c.title}") },
                                onClick = { picked = c.id; open = false },
                            )
                        }
                    }
                }
                Pill("Add blocker", prominent = picked != null, modifier = Modifier.testTag("add-blocker:${row.id}")) {
                    picked?.let { model.addBlocker(row.id, it); picked = null }
                }
            }
        }
    }
}

/**
 * A proposal or an improvement: who noticed it, how bad, where it goes, the proof, and (for an
 * improvement without write access) what would go upstream. Edit is offered only while it waits.
 */
@Composable
fun ProposalView(model: MonitorModel, row: TaskRow, info: ProposalInfo) {
    val theme = LocalTheme.current
    var editing by remember { mutableStateOf(false) }
    val shape = RoundedCornerShape(theme.radius)
    Column(
        verticalArrangement = Arrangement.spacedBy(theme.spacing),
        modifier = Modifier.fillMaxWidth().border(BorderStroke(1.dp, theme.border), shape).padding(theme.spacingLarge).testTag("proposal:${row.id}"),
    ) {
        Text(
            "${info.source} · ${info.severity} · ${if (info.route == "pipeline") "MASON's own repository" else "this project"}",
            style = theme.body(FontWeight.SemiBold),
            modifier = Modifier.testTag("proposal:${row.id}:tag"),
        )
        if (info.filed_by != 0L) LinkedText(model, "Filed by the agent of task #${info.filed_by}", theme.small())
        if (info.evidence.isNotEmpty()) {
            Text("Evidence", style = theme.body(FontWeight.SemiBold))
            LinkedText(model, info.evidence, theme.mono(), Modifier.testTag("proposal:${row.id}:evidence"), selectable = true)
        }
        if (info.status == "rejected") Text("Rejected" + (if (info.reject_reason.isEmpty()) "" else ": ${info.reject_reason}"), style = theme.body(color = theme.muted))
        if (info.branch.isNotEmpty()) {
            Text(
                "Branch ${info.branch} (${info.access} access)" + (if (info.pr != 0L) ", upstream pull request #${info.pr} ${info.pr_state}" else ""),
                style = theme.mono(color = theme.muted),
            )
        }
        if (info.channel.isNotEmpty()) Text("This machine follows the channel ${info.channel}", style = theme.small(), modifier = Modifier.testTag("proposal:${row.id}:channel"))
        if (model.canAct) {
            Row(horizontalArrangement = Arrangement.spacedBy(theme.spacing)) {
                if ("edit" in row.allowed_actions) Pill("Edit", modifier = Modifier.testTag("proposal:${row.id}:edit")) { editing = true }
                if (info.access == "fork" && info.pr == 0L) {
                    Pill("Preview what would go upstream", modifier = Modifier.testTag("proposal:${row.id}:preview")) { model.publish(row.id, false) }
                    if (info.publish_state == "previewed") {
                        Pill("Send it upstream", modifier = Modifier.testTag("proposal:${row.id}:publish")) { model.publish(row.id, true) }
                    }
                }
            }
        }
        model.publishPreview?.takeIf { info.access == "fork" }?.let { p ->
            Column(modifier = Modifier.testTag("proposal:${row.id}:preview-text")) {
                Text("Redacted version (${p.redactions} removed), no client code attached", style = theme.small())
                Text(p.title, style = theme.body(FontWeight.SemiBold))
                Text(p.body, style = theme.body())
                p.files.forEach { Text("changes $it", style = theme.mono(color = theme.muted)) }
                p.blocked?.let { Text("Not publishable: $it", style = theme.body(color = theme.attention)) }
            }
        }
    }
    if (editing) EditProposalDialog(model, row, info) { editing = false }
}

/** Change a waiting proposal's title, text or evidence. Nothing else about it changes. */
@Composable
private fun EditProposalDialog(model: MonitorModel, row: TaskRow, info: ProposalInfo, onClose: () -> Unit) {
    val theme = LocalTheme.current
    var title by remember { mutableStateOf(row.title) }
    var text by remember { mutableStateOf(info.text) }
    var evidence by remember { mutableStateOf(info.evidence) }
    AlertDialog(
        onDismissRequest = onClose,
        title = { Text("Edit proposal #${row.id}", style = theme.title()) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(theme.spacing)) {
                OutlinedTextField(title, { title = it }, label = { Text("Title") }, singleLine = true, modifier = Modifier.testTag("edit-proposal:title"))
                OutlinedTextField(text, { text = it }, label = { Text("Text") }, minLines = 4, modifier = Modifier.testTag("edit-proposal:text"))
                OutlinedTextField(evidence, { evidence = it }, label = { Text("Evidence (a file and line, or the output seen)") }, minLines = 3, modifier = Modifier.testTag("edit-proposal:evidence"))
            }
        },
        confirmButton = {
            TextButton(
                enabled = title.isNotBlank(),
                onClick = { model.editProposal(row.id, title, text, evidence); onClose() },
                modifier = Modifier.testTag("edit-proposal:save"),
            ) { Text("Save") }
        },
        dismissButton = { TextButton(onClick = onClose) { Text("Cancel") } },
    )
}
