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
import androidx.compose.material3.Text
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
import mason.monitor.OptionSelection
import mason.monitor.OtherKey
import mason.monitor.otherAnswer
import mason.monitor.otherBoxKey
import mason.v1.Approval
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
            Text(model.tileCount(count), style = theme.count(if (wants) theme.attention else theme.text), maxLines = 1)
            Text(label, style = theme.small(), maxLines = 1, softWrap = false)
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
        Text(card.ask, style = theme.title())
        if (card.why.isNotEmpty()) Text(card.why, style = theme.body(color = theme.muted))
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
        Text(approval.waiting_for, style = theme.title())
        approval.lines.forEach { Text("• $it", style = theme.body()) }
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
            it != "answer" && it != "edit" &&
                !(hasApprovalView && (it == "approve" || it == "reject")) &&
                !(!model.canAct && (it == "approve" || it == "reject"))
        }.forEach { a ->
            Pill(label(a), prominent = a == "approve", modifier = Modifier.testTag("action:${row.id}:$a")) { run(model, row, a) }
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
        "retry" -> model.retry(row.id)
        "approve" -> model.approve(row.id)
        "reject" -> model.reject(row.id)
        "explore_anyway" -> model.exploreAnyway(row.id)
        "land_now" -> model.landNow(row.id)
        "force" -> model.force(row.id)
        "skip_test" -> model.skipTest(row.id, row.step)
        "open_session" -> copyToClipboard("mason task open ${row.id}")
    }
}
