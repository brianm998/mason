// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import mason.monitor.*
import mason.v1.TaskDetailView

/** The main view: exactly one of Home, a task's detail, or another page. */
@Composable
fun MainPane(model: MonitorModel, modifier: Modifier = Modifier) {
    val theme = LocalTheme.current
    Box(modifier.background(theme.background)) {
        when (val p = model.page) {
            Page.Home -> HomePage(model)
            is Page.Task -> TaskDetailPage(model, p.id)
            Page.Landing -> LandingPage(model)
            Page.Usage -> UsagePage(model)
            Page.Pipeline -> EditorPage(model)
            Page.Health -> HealthPage(model)
        }
    }
}

@Composable
private fun Section(title: String, content: @Composable ColumnScope.() -> Unit) {
    val theme = LocalTheme.current
    Column(verticalArrangement = Arrangement.spacedBy(theme.spacing)) {
        Text(title, style = theme.title())
        content()
    }
}

@Composable
private fun Link(text: String, onClick: () -> Unit) {
    val theme = LocalTheme.current
    Text(text, style = theme.body(color = theme.accent), modifier = Modifier.clickable(onClick = onClick))
}

@Composable
private fun Scrolling(content: @Composable ColumnScope.() -> Unit) {
    val theme = LocalTheme.current
    Column(
        verticalArrangement = Arrangement.spacedBy(theme.spacingLarge),
        modifier = Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(theme.spacingLarge),
        content = content,
    )
}

/**
 * Home: the overall status, the attention queue, the active models with their task and
 * step, the session-limit stepper, the landing queue, and detail for the tasks in each
 * stage. (Counts and task lists live in the top bar and the sidebar, not here.)
 */
@Composable
fun HomePage(model: MonitorModel) {
    val theme = LocalTheme.current
    Scrolling {
        val h = model.home
        Text(
            if (model.connected) h?.status ?: "…" else "Not connected",
            style = theme.title(color = if (model.attention > 0) theme.attention else theme.text),
            modifier = Modifier.testTag("status"),
        )
        val attention = h?.attention.orEmpty()
        if (attention.isNotEmpty()) {
            Section("Needs you") {
                attention.forEach { a ->
                    val t = a.task ?: return@forEach
                    Column(verticalArrangement = Arrangement.spacedBy(theme.spacingSmall)) {
                        Text(
                            "${t.project} ${t.key}: ${t.title}",
                            style = theme.body(FontWeight.SemiBold, theme.muted),
                            modifier = Modifier.clickable { model.go(Page.Task(t.id)) },
                        )
                        a.card?.let { DecisionCardView(model, t.id, it) }
                        a.approval?.let { ApprovalView(model, t.id, it) }
                    }
                }
            }
        }
        Row(horizontalArrangement = Arrangement.spacedBy(theme.spacingLarge * 2)) {
            Section("Active models") {
                val active = h?.active.orEmpty()
                if (active.isEmpty()) Text("Nothing is running.", style = theme.body(color = theme.muted))
                active.forEach { a ->
                    Row(horizontalArrangement = Arrangement.spacedBy(theme.spacing), modifier = Modifier.testTag("active:${a.task}")) {
                        Text(a.model, style = theme.body(FontWeight.SemiBold))
                        Link("${a.project} #${a.task}") { model.go(Page.Task(a.task)) }
                        Text(a.step, style = theme.body(color = theme.muted))
                    }
                }
                SessionStepper(model)
            }
            Section("Landing queue") {
                val queue = model.landing?.queue ?: h?.stages?.firstOrNull { it.state == "landing" }?.rows.orEmpty()
                if (queue.isEmpty()) Text("Nothing is landing.", style = theme.body(color = theme.muted))
                queue.forEach { r -> Link("${r.key} ${r.title}") { model.go(Page.Task(r.id)) } }
            }
        }
        h?.stages?.forEach { st ->
            Section(st.title) {
                st.rows.forEach { r ->
                    Row(horizontalArrangement = Arrangement.spacedBy(theme.spacing), verticalAlignment = Alignment.CenterVertically,
                        modifier = Modifier.clickable { model.go(Page.Task(r.id)) }) {
                        ProgressCells(r.progress, r.slow)
                        Text("${r.project} ${r.key}", style = theme.body(FontWeight.SemiBold))
                        Text(r.title, style = theme.body())
                        Text(
                            if (r.why_waiting.isEmpty()) Format.duration(r.in_state_ms) else "${r.why_waiting}, ${Format.duration(r.in_state_ms)}",
                            style = theme.small(color = if (r.slow) theme.slow else theme.muted),
                        )
                    }
                }
            }
        }
    }
}

/** "Sessions: 3 of [-] 4 [+]": changes how many LLM sessions may be held. */
@Composable
fun SessionStepper(model: MonitorModel) {
    val theme = LocalTheme.current
    val used = model.home?.sessions_used ?: 0
    val limit = model.home?.sessions_limit ?: 0
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(theme.spacing),
        modifier = Modifier.testTag("sessions")) {
        Text("Sessions: $used of", style = theme.body())
        Pill("−", modifier = Modifier.testTag("sessions:decrease")) { model.setSessionLimit(if (limit > 0) limit - 1 else 0) }
        Text("$limit", style = theme.body(FontWeight.SemiBold))
        Pill("+", modifier = Modifier.testTag("sessions:increase")) { model.setSessionLimit(limit + 1) }
    }
}

/**
 * A task's page: step progress, activity, the live log, the plan, checks, the decision
 * card and the actions; copy buttons; why exploring was skipped.
 */
@Composable
fun TaskDetailPage(model: MonitorModel, id: Long) {
    val theme = LocalTheme.current
    val d = model.taskDetail
    val row = d?.row
    if (d == null || row == null || row.id != id) {
        Text(if (model.connected) "Loading task #$id…" else "Not connected", style = theme.body(color = theme.muted),
            modifier = Modifier.padding(theme.spacingLarge))
        return
    }
    Scrolling { TaskDetail(model, d) }
}

@Composable
private fun TaskDetail(model: MonitorModel, d: TaskDetailView) {
    val theme = LocalTheme.current
    val r = d.row ?: return
    Column(verticalArrangement = Arrangement.spacedBy(theme.spacingSmall)) {
        Text("${r.key} ${r.title}", style = theme.title(), modifier = Modifier.testTag("detail:${r.id}"))
        Text("${r.project}, ${r.state.replace('_', ' ')}, ${Format.duration(r.in_state_ms)} in this state", style = theme.body(color = theme.muted))
        if (r.why_waiting.isNotEmpty()) Text(r.why_waiting, style = theme.body())
        Row(horizontalArrangement = Arrangement.spacedBy(theme.spacingLarge)) {
            CopyButton(r.key, r.key)
            if (d.branch.isNotEmpty()) CopyButton("branch", d.branch)
            if (d.worktree.isNotEmpty()) CopyButton("worktree", d.worktree)
        }
    }
    Row(horizontalArrangement = Arrangement.spacedBy(theme.spacing), verticalAlignment = Alignment.CenterVertically) {
        ProgressCells(r.progress, r.slow)
        if (r.typical.isNotEmpty()) Text(r.typical, style = theme.small())
    }
    ActionButtons(model, r, hasApprovalView = d.approval != null)
    if (r.blockers.isNotEmpty()) {
        Row(horizontalArrangement = Arrangement.spacedBy(theme.spacingSmall), modifier = Modifier.testTag("waiting-for")) {
            Text("waiting for", style = theme.body(color = theme.muted))
            r.blockers.forEach { b ->
                Text("#$b", style = theme.body(color = theme.accent), modifier = Modifier.clickable { model.go(Page.Task(b)) })
            }
        }
    }
    if (r.blocks.isNotEmpty()) {
        Row(horizontalArrangement = Arrangement.spacedBy(theme.spacingSmall), modifier = Modifier.testTag("blocks")) {
            Text("blocks", style = theme.body(color = theme.muted))
            r.blocks.forEach { b ->
                Text("#$b", style = theme.body(color = theme.accent), modifier = Modifier.clickable { model.go(Page.Task(b)) })
            }
        }
    }
    if (d.route.isNotEmpty()) {
        Row(horizontalArrangement = Arrangement.spacedBy(theme.spacing)) {
            Text("Started at ${d.started_at}: ${d.route}.", style = theme.body(color = theme.muted))
            if ("explore_anyway" in r.allowed_actions) Text("Use “Explore anyway” to explore it after all.", style = theme.small())
            if (d.explore_returns > 0) Text("Sent back to exploring ${d.explore_returns} time(s), at no cost.", style = theme.small())
        }
    }
    d.numbers.forEach { n ->
        Text(
            "Number ${n.kind} ${n.number}: ${n.path}" + (if (n.source == "stage") " (its stage)" else "") + (if (n.released) " (released, not reused)" else ""),
            style = theme.small(),
            modifier = Modifier.testTag("number:${n.kind}"),
        )
    }
    d.card?.let { DecisionCardView(model, r.id, it) }
    d.approval?.let { ApprovalView(model, r.id, it) }
    if (d.failure.isNotEmpty()) Text("Failed: ${d.failure}", style = theme.body(color = theme.attention))
    Section("Steps") {
        d.steps.forEach { s ->
            Row(horizontalArrangement = Arrangement.spacedBy(theme.spacing), modifier = Modifier.testTag("step:${s.run}")) {
                Text("run ${s.run}", style = theme.body(color = theme.muted))
                Text(s.name.ifEmpty { s.category }, style = theme.body(FontWeight.SemiBold))
                Text("attempt ${s.attempt}", style = theme.body(color = theme.muted))
                Text(s.outcome, style = theme.body())
            }
        }
    }
    if (d.closing.isNotEmpty()) Section("Tests") { Text(d.closing, style = theme.body()) }
    if (d.debts.isNotEmpty()) Section("Debts") {
        d.debts.forEach { Text("#${it.id} ${it.scope}, ${it.status}${if (it.deferred) ", runs after landing" else ""}", style = theme.body()) }
    }
    if (d.plan.isNotEmpty()) Section("Plan") { SelectionContainer { Text(d.plan, style = theme.mono()) } }
    if (d.body.isNotEmpty()) Section("Task") { SelectionContainer { Text(d.body, style = theme.body()) } }
    if (d.log_tail.isNotEmpty()) Section("Log") {
        SelectionContainer {
            Text(d.log_tail, style = theme.mono(),
                modifier = Modifier.fillMaxWidth().background(theme.surface, RoundedCornerShape(theme.radius)).padding(theme.spacing))
        }
    }
}

@Composable
fun LandingPage(model: MonitorModel) {
    val theme = LocalTheme.current
    Scrolling {
        Text("Landing", style = theme.title())
        model.landing?.let { l ->
            if (l.note.isNotEmpty()) Text(l.note, style = theme.body(color = if (l.frozen) theme.attention else theme.muted))
            l.modes.forEach { Text(it, style = theme.body(color = theme.muted)) }
            // One line per project: tracker off, polling with counts, or stopped with why.
            l.trackers.forEach { t ->
                Text(t, style = theme.body(color = if (t.contains("stopped") || t.contains(" off;")) theme.attention else theme.muted))
            }
            l.freezes.forEach { f ->
                Text(
                    "Frozen: ${f.project}: ${f.reason}${if (f.fix_task > 0) " (fix: task #${f.fix_task})" else ""}",
                    style = theme.body(color = theme.attention),
                )
            }
            Section("Queue") {
                if (l.queue.isEmpty()) Text("Nothing is landing.", style = theme.body(color = theme.muted))
                l.queue.forEach { r -> Link("${r.key} ${r.title}") { model.go(Page.Task(r.id)) } }
            }
            Section("Batches") {
                if (l.batches.isEmpty()) Text("No batch has formed yet.", style = theme.body(color = theme.muted))
                l.batches.forEach { Text(batchText(it), style = theme.body()) }
                if (l.queue_idle_ms > 0) {
                    Text(
                        "Queue idle: ${l.queue_idle_ms / 1000} s with tasks waiting and nothing landing.",
                        style = theme.body(color = theme.muted),
                    )
                }
            }
            Section("Debts") {
                if (l.debts.isEmpty()) Text("Nothing is owed.", style = theme.body(color = theme.muted))
                l.debts.forEach { x ->
                    Text("#${x.id} task #${x.task} ${x.scope}: ${x.reason.ifEmpty { "no reason given" }}", style = theme.body())
                }
            }
        }
    }
}

/** One line per batch: where it is, who landed, who was parked, and why. */
fun batchText(b: mason.v1.BatchLine): String {
    fun list(v: List<Long>) = v.joinToString(" ") { "#$it" }
    var t = "Batch #${b.id} ${b.project} ${b.phase}${if (b.running) " (running)" else ""}: ${list(b.tasks)}"
    if (b.landed.isNotEmpty()) t += ", landed ${list(b.landed)}"
    if (b.parked.isNotEmpty()) t += ", parked ${list(b.parked)}"
    if (b.note.isNotEmpty()) t += ". ${b.note}"
    return t
}

@Composable
fun UsagePage(model: MonitorModel) {
    val theme = LocalTheme.current
    Scrolling {
        Text("LLM usage", style = theme.title())
        model.usage?.providers?.forEach {
            val budget = if (it.monthly_budget_usd > 0) ", budget \$${it.monthly_budget_usd.toInt()}/month" else ""
            Text("${it.name} (${it.adapter}, ${it.billing}): ${it.status}, ${it.sessions_used} of ${it.sessions_limit} sessions in use; warn at ${(it.warn_at * 100).toInt()}%, overage ${it.overage}$budget", style = theme.body())
        }
        model.usage?.note?.let { Text(it, style = theme.body(color = theme.muted)) }
        SessionStepper(model)
    }
}

@Composable
fun HealthPage(model: MonitorModel) {
    val theme = LocalTheme.current
    Scrolling {
        Text("Health", style = theme.title())
        Text(if (model.connected) "Connected to masond. Event log at #${model.lastSeq}." else "Not connected.", style = theme.body())
        model.health?.let { h ->
            Section("Resources") { h.resources.forEach { Text("${it.name} (${it.kind}): ${it.used} of ${it.limit} in use", style = theme.body()) } }
            Section("Waiting") {
                if (h.waiting.isEmpty()) Text("Nothing is waiting.", style = theme.body(color = theme.muted))
                h.waiting.forEach { Text(it, style = theme.body()) }
            }
        }
    }
}
