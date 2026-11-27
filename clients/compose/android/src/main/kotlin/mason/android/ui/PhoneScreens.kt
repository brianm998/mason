// SPDX-License-Identifier: GPL-3.0-or-later
package mason.android.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import mason.monitor.*
import mason.v1.*

/** The sidebar, as the first screen: count tiles, Home, the state sections, Landing and Usage (`PhoneLayout.sidebarOrder`). */
@Composable
fun SidebarScreen(model: MonitorModel, push: (Page) -> Unit) {
    val sections = model.sidebar?.sections.orEmpty()
    LazyColumn(Modifier.fillMaxSize()) {
        for (block in PhoneLayout.sidebarOrder) when (block) {
            PhoneBlock.Tiles -> item(key = "tiles") {
                LazyRow(Modifier.padding(12.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    items(model.header?.counts.orEmpty(), key = { it.state }) { c -> Tile(model, c, push) }
                }
            }
            PhoneBlock.Home -> item(key = "home") { NavRow("Home") { push(Page.Home) } }
            PhoneBlock.Sections -> {
                for (s in sections) {
                    item(key = "h:${s.state}") {
                        val open = model.isExpanded(s)
                        SectionHeader("${s.title.ifEmpty { s.state }} (${model.countText(s)})", open) { model.toggle(s) }
                    }
                    if (model.isExpanded(s)) {
                        items(model.filteredRows(s), key = { "r:${s.state}:${it.id}" }) { r -> TaskRowItem(r) { push(Page.Task(r.id)) } }
                    }
                }
            }
            PhoneBlock.Landing -> item(key = "landing") { NavRow("Landing") { push(Page.Landing) } }
            PhoneBlock.Usage -> item(key = "usage") { NavRow("Usage") { push(Page.Usage) } }
        }
    }
}

/** A count tile: tinted when the state wants you. A tap opens its page when it has one, else opens its section here. */
@Composable
private fun Tile(model: MonitorModel, c: StateCount, push: (Page) -> Unit) {
    val tint = if (c.wants_you && model.connected) MaterialTheme.colorScheme.errorContainer else MaterialTheme.colorScheme.surfaceVariant
    Surface(color = tint, shape = MaterialTheme.shapes.medium, modifier = Modifier.clickable {
        if (!model.connected) return@clickable
        val p = Page.sectionPage(c.state)
        if (p != null && c.count > 0) push(p) else model.reveal(c.state)
    }) {
        Column(Modifier.padding(horizontal = 14.dp, vertical = 8.dp)) {
            Text(model.tileCount(c), style = MaterialTheme.typography.titleLarge)
            Text(model.tileLabel(c), style = MaterialTheme.typography.bodySmall)
        }
    }
}

@Composable
fun NavRow(label: String, onClick: () -> Unit) {
    Row(Modifier.fillMaxWidth().clickable(onClick = onClick).padding(horizontal = 16.dp, vertical = 14.dp)) {
        Text(label, Modifier.weight(1f), style = MaterialTheme.typography.titleMedium)
        Text("›")
    }
    HorizontalDivider()
}

@Composable
private fun SectionHeader(text: String, open: Boolean, onClick: () -> Unit) {
    Row(Modifier.fillMaxWidth().clickable(onClick = onClick).padding(horizontal = 16.dp, vertical = 10.dp)) {
        Text(if (open) "▾" else "▸", Modifier.padding(end = 8.dp))
        Text(text, style = MaterialTheme.typography.titleSmall, fontWeight = FontWeight.Bold)
    }
}

@Composable
fun TaskRowItem(r: TaskRow, onClick: () -> Unit) {
    Column(Modifier.fillMaxWidth().clickable(onClick = onClick).padding(horizontal = 24.dp, vertical = 8.dp)) {
        Text("#${r.id}  ${r.title}", style = MaterialTheme.typography.bodyLarge)
        val sub = mason.monitor.QueueWait.line(r)
        if (sub.isNotEmpty()) Text(sub, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

/** The page a row pushed. Pages a phone does not have (the pipeline's graph, health, agent sessions) are not offered. */
@Composable
fun DetailScreen(model: MonitorModel, page: Page, onOpen: (Page) -> Unit, settings: @Composable () -> Unit) {
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(bottom = 24.dp)) {
        if (!model.connected && page != Page.Settings) {
            Text("Not connected: showing nothing until the daemon answers.", Modifier.padding(16.dp))
            return@Column
        }
        when (page) {
            Page.Home -> HomePage(model, onOpen)
            is Page.Task -> TaskPage(model, page.id)
            Page.Landing -> LandingPage(model)
            Page.Usage -> UsagePage(model)
            Page.Settings -> settings()
            is Page.Section -> SectionPage(model, page.name, onOpen)
            Page.Pipeline, Page.Health -> Text("Not on a phone. Use the Mac or desktop monitor.", Modifier.padding(16.dp))
        }
    }
}

@Composable
private fun Heading(text: String) = Text(text, Modifier.padding(start = 16.dp, top = 16.dp, end = 16.dp, bottom = 4.dp), style = MaterialTheme.typography.titleMedium)

@Composable
private fun HomePage(model: MonitorModel, open: (Page) -> Unit) {
    val home = model.home
    Text(home?.status ?: model.header?.status.orEmpty(), Modifier.padding(16.dp), style = MaterialTheme.typography.titleLarge)
    if (home == null) return
    home.queue_hold?.let { h ->
        Text(h.summary, Modifier.padding(horizontal = 16.dp, vertical = 4.dp),
            color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodyMedium)
        h.party?.takeIf { it.lease }?.let {
            Text(mason.monitor.QueueWait.lease(it, model.daemonNow()), Modifier.padding(horizontal = 24.dp), style = MaterialTheme.typography.bodySmall)
        }
    }
    if (home.attention.isNotEmpty()) {
        Heading("Needs you")
        for (a in home.attention) a.task?.let { TaskRowItem(it) { open(Page.Task(it.id)) } }
    }
    if (home.active.isNotEmpty()) {
        Heading("Active models")
        for (m in home.active) Text("${m.display.ifEmpty { m.model }}  ·  #${m.task}", Modifier.padding(horizontal = 24.dp, vertical = 4.dp))
    }
    model.placementText(PlacementScope.Global)?.let { Text("Runs on: $it", Modifier.padding(horizontal = 16.dp, vertical = 2.dp), style = MaterialTheme.typography.bodySmall) }
    SessionStepper(model, home)
    Heading("Stages")
    for (s in home.stages) {
        val p = Page.sectionPage(s.state)
        NavRow("${s.title.ifEmpty { s.state }} (${s.count})") { p?.let(open) }
    }
}

/** The session limit: how many LLM sessions are held and may be, with − and +. Only the owner changes it. */
@Composable
private fun SessionStepper(model: MonitorModel, home: HomeView) {
    val limit = home.sessions_limit.toInt()
    Heading("LLM sessions")
    Row(Modifier.padding(horizontal = 16.dp), verticalAlignment = androidx.compose.ui.Alignment.CenterVertically) {
        OutlinedButton(onClick = { model.setSessionLimit(maxOf(0, limit - 1)) }, enabled = model.canAct && limit > 0) { Text("−") }
        Text("${home.sessions_used} of $limit", Modifier.padding(horizontal = 16.dp), style = MaterialTheme.typography.titleMedium)
        OutlinedButton(onClick = { model.setSessionLimit(limit + 1) }, enabled = model.canAct) { Text("+") }
    }
}

@Composable
private fun SectionPage(model: MonitorModel, name: String, open: (Page) -> Unit) {
    val rows = model.sectionRows(name)
    model.sectionCountText(name)?.let { Text(it, Modifier.padding(horizontal = 16.dp), style = MaterialTheme.typography.bodySmall) }
    if (rows.isEmpty()) Text("Nothing here.", Modifier.padding(16.dp))
    if (name == "testing" && rows.isNotEmpty()) {
        OutlinedButton(onClick = { model.skipAllTests(rows) }, Modifier.padding(horizontal = 16.dp), enabled = model.skipDisabledReason == null) { Text("Skip testing for all") }
        (model.skipDisabledReason ?: model.skipNotice ?: model.lastResult)?.let { Text(it, Modifier.padding(horizontal = 16.dp), style = MaterialTheme.typography.bodySmall) }
    }
    if (name == "landing" && rows.isNotEmpty()) {
        var askAll by androidx.compose.runtime.remember { androidx.compose.runtime.mutableStateOf(false) }
        OutlinedButton(onClick = { askAll = true }, Modifier.padding(horizontal = 16.dp), enabled = model.landDisabledReason == null) { Text("Land now for all") }
        (model.landDisabledReason ?: model.landNotice ?: model.lastResult)?.let { Text(it, Modifier.padding(horizontal = 16.dp), style = MaterialTheme.typography.bodySmall) }
        if (askAll) {
            AlertDialog(
                onDismissRequest = { askAll = false },
                text = { Text("Land ${model.landableRows(rows).size} tasks now? Their remaining tests run after the merge; a red result raises an alert.") },
                confirmButton = { Button(onClick = { askAll = false; model.landAllNow(rows) }) { Text("Land now") } },
                dismissButton = { OutlinedButton(onClick = { askAll = false }) { Text("Cancel") } },
            )
        }
    }
    for (r in rows) {
        TaskRowItem(r) { open(Page.Task(r.id)) }
        if (name == "landing") LandNowButton(model, LandNowTarget.Task(r.id), model.canAct && "land_now" in r.allowed_actions)
        val progress = SectionPages.suiteProgress(r)
        if (progress.isNotEmpty()) Text(progress, Modifier.padding(horizontal = 24.dp), style = MaterialTheme.typography.bodySmall)
    }
}

/** Land now on the landing queue: owner devices only, a one-line confirm, then the pending-command label. */
@Composable
private fun LandNowButton(model: MonitorModel, target: LandNowTarget, shown: Boolean) {
    if (!shown) return
    val pending = model.landNowLabel(target)
    OutlinedButton(onClick = { model.askLandNow(target) }, Modifier.padding(horizontal = 24.dp), enabled = pending == null) { Text(pending ?: "Land now") }
    if (model.landNowAsk == target) {
        AlertDialog(
            onDismissRequest = { model.cancelLandNow() },
            text = { Text(target.confirmText) },
            confirmButton = { Button(onClick = { model.confirmLandNow() }) { Text("Land now") } },
            dismissButton = { OutlinedButton(onClick = { model.cancelLandNow() }) { Text("Cancel") } },
        )
    }
}

@Composable
private fun LandingPage(model: MonitorModel) {
    val l = model.landing
    if (l == null) { Text("Loading…", Modifier.padding(16.dp)); return }
    if (l.frozen) Text("Landing is frozen. ${l.note}", Modifier.padding(16.dp), color = MaterialTheme.colorScheme.error)
    for (m in l.modes) Text(m, Modifier.padding(horizontal = 16.dp, vertical = 2.dp), style = MaterialTheme.typography.bodySmall)
    Heading("Queue (${l.queue.size})")
    if (l.queue.isEmpty()) Text("Nothing is waiting to land.", Modifier.padding(horizontal = 16.dp))
    for (r in l.queue) {
        TaskRowItem(r) {}
        if (model.canAct && "land_now" in r.allowed_actions) {
            LandNowButton(model, LandNowTarget.Task(r.id), true)
        }
    }
    if (l.batches.isNotEmpty()) {
        Heading("Batches")
        for (b in l.batches) {
            Text("Batch #${b.id} ${b.phase}${if (b.running) " (running)" else ""}: ${b.tasks.joinToString(" ") { "#$it" }}", Modifier.padding(horizontal = 24.dp, vertical = 2.dp), style = MaterialTheme.typography.bodySmall)
            LandNowButton(model, LandNowTarget.Batch(b.id), model.canLandNow(b))
        }
    }
    if (l.debts.isNotEmpty()) {
        Heading("Debts")
        for (d in l.debts) Text("#${d.task}  ${d.scope}  ${d.status}: ${d.reason}", Modifier.padding(horizontal = 24.dp, vertical = 2.dp), style = MaterialTheme.typography.bodySmall)
    }
}

@Composable
private fun UsagePage(model: MonitorModel) {
    val u = model.usage
    if (u == null) { Text("Loading…", Modifier.padding(16.dp)); return }
    if (u.note.isNotEmpty()) Text(u.note, Modifier.padding(16.dp), style = MaterialTheme.typography.bodySmall)
    for (p in u.providers) {
        Heading(p.name)
        Text("${p.sessions_used} of ${p.sessions_limit} sessions  ·  ${p.status}", Modifier.padding(horizontal = 16.dp))
        for (w in p.windows) {
            val pct = if (w.has_percent) "${w.percent.toInt()}%" else if (w.reported) "" else "not reported"
            Text("${w.label} $pct  ${w.resets_text}".trim(), Modifier.padding(horizontal = 24.dp, vertical = 2.dp), style = MaterialTheme.typography.bodyMedium)
        }
        if (p.blocked_text.isNotEmpty()) Text(p.blocked_text, Modifier.padding(horizontal = 24.dp), color = MaterialTheme.colorScheme.error)
    }
}

/** The task: where it is, its decision card (options and Other), the actions the daemon allows, the plan and the log tail. */
@Composable
private fun TaskPage(model: MonitorModel, id: Long) {
    val d = model.taskDetail?.takeIf { it.row?.id == id }
    if (d == null) { Text("Loading task #$id…", Modifier.padding(16.dp)); return }
    val row = d.row ?: return
    var confirmCancel by remember(id) { mutableStateOf(false) }
    Text(row.title, Modifier.padding(16.dp), style = MaterialTheme.typography.titleLarge)
    Text("#${row.id}  ${row.state}  ${row.project}", Modifier.padding(horizontal = 16.dp), style = MaterialTheme.typography.bodySmall)
    val sub = row.why_waiting.ifEmpty { row.activity }
    if (sub.isNotEmpty()) Text(sub, Modifier.padding(horizontal = 16.dp, vertical = 4.dp))
    row.wait?.let { w ->
        Text(mason.monitor.QueueWait.badge(w.cause), Modifier.padding(horizontal = 16.dp), color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.labelMedium)
        for (line in mason.monitor.QueueWait.chain(w, row.key)) Text(line, Modifier.padding(horizontal = 24.dp, vertical = 1.dp), style = MaterialTheme.typography.bodySmall)
    }
    d.card?.takeIf { it.ask.isNotEmpty() || it.options.isNotEmpty() }?.let { DecisionCardView(model, id, it) }
    if (d.failure.isNotEmpty()) Text(d.failure, Modifier.padding(16.dp), color = MaterialTheme.colorScheme.error)
    if (d.closing.isNotEmpty()) Text(d.closing, Modifier.padding(horizontal = 16.dp, vertical = 4.dp))
    val failedPage = if (row.state == "failed") mason.monitor.FailedPage.choose(d.card != null, d.diagnosis_running != null, row.allowed_actions) else null
    d.diagnosis_running?.takeIf { failedPage == mason.monitor.FailedPage.Diagnosing }?.let {
        Text(mason.monitor.diagnosingLine(it.started_ms, it.ran_on, model.daemonNow()), Modifier.padding(horizontal = 16.dp, vertical = 4.dp), color = MaterialTheme.colorScheme.error)
    }
    TaskActions(model, row, failedPage) { confirmCancel = true }
    d.plan_view?.let { p ->
        Heading("Plan")
        Text(PlanText.copyAsText(p), Modifier.padding(horizontal = 16.dp), style = MaterialTheme.typography.bodyMedium)
    }
    if (d.steps.isNotEmpty()) {
        Heading("Steps")
        for (s in d.steps) Text("${s.name}  ${s.outcome}", Modifier.padding(horizontal = 24.dp, vertical = 2.dp), style = MaterialTheme.typography.bodySmall)
    }
    // The log tail only: full logs and run history are not on a phone (deliberate difference phone.log-deep-dives).
    if (d.log_tail.isNotEmpty()) {
        Heading("Log tail")
        Text(d.log_tail, Modifier.padding(horizontal = 16.dp), style = MaterialTheme.typography.bodySmall, fontFamily = androidx.compose.ui.text.font.FontFamily.Monospace)
    }
    if (confirmCancel) {
        AlertDialog(
            onDismissRequest = { confirmCancel = false },
            title = { Text("Cancel task #${row.id}?") },
            text = { Text("The task stops and its work is dropped.") },
            confirmButton = { TextButton(onClick = { confirmCancel = false; model.cancel(row.id) }) { Text("Cancel task") } },
            dismissButton = { TextButton(onClick = { confirmCancel = false }) { Text("Keep") } },
        )
    }
}

/** Cancel, skip testing and land now, each only when the daemon allows it for this task and this device may act. */
@Composable
private fun TaskActions(model: MonitorModel, row: TaskRow, failedPage: mason.monitor.FailedPage?, askCancel: () -> Unit) {
    if (!model.canAct) return
    var otherOpen by remember(row.id) { mutableStateOf(false) }
    // Behind a diagnosis, Cancel folds into a collapsed "Other actions" row.
    val folded = failedPage?.hidesUnderOther("cancel") == true
    Row(Modifier.padding(horizontal = 16.dp, vertical = 8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        if ("diagnose" in row.allowed_actions) OutlinedButton(onClick = { model.diagnose(row.id) }) { Text("Diagnose") }
        if (folded && "cancel" in row.allowed_actions) {
            TextButton(onClick = { otherOpen = !otherOpen }) { Text(if (otherOpen) "Other actions ▾" else "Other actions ▸") }
            if (otherOpen) OutlinedButton(onClick = askCancel) { Text("Cancel") }
        } else if ("cancel" in row.allowed_actions) OutlinedButton(onClick = askCancel) { Text("Cancel") }
        if ("skip_test" in row.allowed_actions) OutlinedButton(onClick = { model.skipTest(row.id, row.step) }) { Text("Skip testing") }
        if ("land_now" in row.allowed_actions) OutlinedButton(onClick = { model.landNow(row.id) }) { Text("Land now") }
    }
}

/** A question: each option is a button (several can be checked on a multi-select card), and Other takes free text. */
@Composable
private fun DecisionCardView(model: MonitorModel, task: Long, card: DecisionCard) {
    var selection by remember(card.hash) { mutableStateOf(MultiSelection.starting(card.options.size, card.options.indexOfFirst { it.recommended }.takeIf { it >= 0 })) }
    var other by remember(card.hash) { mutableStateOf("") }
    var context by remember(card.hash) { mutableStateOf("") }
    Surface(color = MaterialTheme.colorScheme.surfaceVariant, shape = MaterialTheme.shapes.medium, modifier = Modifier.padding(16.dp).fillMaxWidth()) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(card.ask, style = MaterialTheme.typography.titleMedium)
            if (card.why.isNotEmpty()) Text(card.why, style = MaterialTheme.typography.bodyMedium)
            for (e in card.evidence) Text(e, style = MaterialTheme.typography.bodySmall)
            card.options.forEachIndexed { i, o ->
                val label = if (o.recommended) "${o.label} (recommended)" else o.label
                if (card.multi_select) {
                    Row(Modifier.fillMaxWidth().clickable(enabled = model.canAct) { selection = selection.toggle(i) }, verticalAlignment = androidx.compose.ui.Alignment.CenterVertically) {
                        Checkbox(checked = i in selection.checked, onCheckedChange = { selection = selection.toggle(i) }, enabled = model.canAct)
                        Text(label)
                    }
                } else {
                    Button(onClick = { model.answer(task, mason.monitor.AnswerRequest.option(i, context), card.hash); context = "" }, enabled = model.canAct, modifier = Modifier.fillMaxWidth()) { Text(label) }
                }
            }
            if (card.multi_select) {
                Button(onClick = { selection.answer?.let { model.answer(task, mason.monitor.AnswerRequest.multi(it, context), card.hash); context = "" } }, enabled = model.canAct && selection.answer != null) { Text("Send") }
            }
            OutlinedTextField(value = context, onValueChange = { context = it }, label = { Text("Add context (optional)") }, enabled = model.canAct, modifier = Modifier.fillMaxWidth())
            OutlinedTextField(value = other, onValueChange = { other = it }, label = { Text("Other") }, enabled = model.canAct, modifier = Modifier.fillMaxWidth())
            Button(onClick = { mason.monitor.AnswerRequest.other(other, context)?.let { model.answer(task, it, card.hash); other = ""; context = "" } }, enabled = model.canAct && otherAnswer(other) != null) { Text("Send other") }
            if (!model.canAct) Text("This device can look but not answer.", style = MaterialTheme.typography.bodySmall)
        }
    }
}

/** The paired daemon, its role and the address last reached, with Forget. */
@Composable
fun PairedSettings(record: PairedRecordView?, host: String?, forget: () -> Unit) {
    var confirm by remember { mutableStateOf(false) }
    if (record == null) { Text("No daemon is paired.", Modifier.padding(16.dp)); return }
    Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
        Text("Name: ${record.host}")
        Text("Role: ${record.role}")
        Text("Fingerprint: ${mason.android.PairingScreenState.shortFingerprint(record.fingerprint)}")
        host?.let { Text("Last used: $it") }
        Button(onClick = { confirm = true }, colors = ButtonDefaults.buttonColors(containerColor = MaterialTheme.colorScheme.error)) { Text("Forget this daemon") }
    }
    if (confirm) {
        AlertDialog(
            onDismissRequest = { confirm = false },
            title = { Text("Forget this daemon?") },
            text = { Text("The token on this phone is deleted. You will pair again to use it.") },
            confirmButton = { TextButton(onClick = { confirm = false; forget() }) { Text("Forget") } },
            dismissButton = { TextButton(onClick = { confirm = false }) { Text("Keep") } },
        )
    }
}
