// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.Image
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import mason.monitor.*
import mason.v1.CellState
import mason.v1.Compilation
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
private fun Section(title: String, state: String? = null, content: @Composable ColumnScope.() -> Unit) {
    val theme = LocalTheme.current
    Column(verticalArrangement = Arrangement.spacedBy(theme.spacing)) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(theme.spacingSmall)) {
            Text(title, style = theme.title())
            if (state != null) StateIcon(state)
        }
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
        var adding by remember { mutableStateOf(false) }
        val pic = model.homeMascot
        val statusLine: @Composable () -> Unit = {
            Text(
                if (model.connected) h?.status ?: "…" else "Not connected",
                style = theme.title(color = if (model.attention > 0) theme.attention else theme.text),
                modifier = Modifier.testTag("status"),
            )
        }
        if (pic.large) {
            Column(Modifier.fillMaxWidth(), horizontalAlignment = Alignment.CenterHorizontally,
                verticalArrangement = Arrangement.spacedBy(theme.spacing)) {
                MascotPicture(pic.id, theme.mascotEmpty)
                statusLine()
                Pill("Add task", modifier = Modifier.testTag("home:add-task")) { adding = true }
            }
        } else {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(theme.spacingLarge)) {
                MascotPicture(pic.id, theme.mascotStatus)
                statusLine()
            }
        }
        if (adding) AddTaskDialog(model) { adding = false }
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
                        Text(a.display.ifEmpty { a.model }, style = theme.body(FontWeight.SemiBold))
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
        h?.compilation?.let { CompilationSection(model, it) }
        h?.stages?.forEach { st ->
            Section(st.title, st.state) {
                st.rows.forEach { r ->
                    Row(horizontalArrangement = Arrangement.spacedBy(theme.spacing), verticalAlignment = Alignment.CenterVertically,
                        modifier = Modifier.clickable { model.go(Page.Task(r.id)) }) {
                        ProgressCells(model, r.progress, r.slow)
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
 * Home's Compilation section: what holds a `compile` slot, what waits for one and why
 * (in line order), the CPU reading, and the limit's stepper.
 */
@Composable
fun CompilationSection(model: MonitorModel, c: Compilation) {
    val theme = LocalTheme.current
    val now = model.daemonNow()
    val age = { since: Long -> Format.duration(maxOf(0L, now - since)) }
    val who: @Composable (String, Long) -> Unit = { label, task ->
        if (task > 0) Link("#$task") { model.go(Page.Task(task)) }
        else Text(label, style = theme.body(), maxLines = 1, overflow = TextOverflow.Ellipsis,
            modifier = Modifier.widthIn(max = 480.dp))
    }
    Section("Compilation") {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(theme.spacing),
            modifier = Modifier.testTag("compile")) {
            Text("Compiling: ${c.used} of at most", style = theme.body())
            Pill("−", modifier = Modifier.testTag("compile:decrease")) { model.setCompileLimit(if (c.limit > 1) c.limit - 1 else 1) }
            Text("${c.limit}", style = theme.body(FontWeight.SemiBold))
            Pill("+", modifier = Modifier.testTag("compile:increase")) { model.setCompileLimit(c.limit + 1) }
            Text(c.cpuLine, style = theme.body(color = theme.muted))
        }
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(theme.spacing),
            modifier = Modifier.testTag("compile:cpu")) {
            Text("Start another compile below", style = theme.body())
            Pill("−", modifier = Modifier.testTag("compile:cpu:decrease")) { model.setCompileCpuGate(c.cpu_max_pct - 5) }
            Text("${c.cpu_max_pct}% CPU", style = theme.body(FontWeight.SemiBold))
            Pill("+", modifier = Modifier.testTag("compile:cpu:increase")) { model.setCompileCpuGate(c.cpu_max_pct + 5) }
        }
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(theme.spacing),
            modifier = Modifier.testTag("compile:settle")) {
            Text("and wait", style = theme.body())
            Pill("−", modifier = Modifier.testTag("compile:settle:decrease")) { model.setCompileSettle(c.settle_s - 5) }
            Text("${c.settle_s} s", style = theme.body(FontWeight.SemiBold))
            Pill("+", modifier = Modifier.testTag("compile:settle:increase")) { model.setCompileSettle(c.settle_s + 5) }
            Text("after the last one started", style = theme.body())
        }
        model.lastResult?.let {
            Text(it, style = theme.body(color = theme.attention), modifier = Modifier.testTag("compile:refused"))
        }
        if (c.running.isEmpty()) Text("Nothing is compiling.", style = theme.body(color = theme.muted))
        c.running.forEachIndexed { i, r ->
            Row(horizontalArrangement = Arrangement.spacedBy(theme.spacing), modifier = Modifier.testTag("compile:running:$i")) {
                who(r.label, r.task)
                if (r.interactive) Text("interactive", style = theme.body(color = theme.muted))
                Text("for ${age(r.since_ms)}", style = theme.body(color = theme.muted))
                if (r.idle_ms >= 60_000) Text("no CPU for ${Format.duration(r.idle_ms)}", style = theme.body(color = theme.slow))
            }
        }
        if (c.pending.isNotEmpty()) Text("Waiting, in line order", style = theme.body(FontWeight.SemiBold))
        c.pending.forEach { w ->
            Row(horizontalArrangement = Arrangement.spacedBy(theme.spacing), verticalAlignment = Alignment.CenterVertically,
                modifier = Modifier.testTag("compile:pending:${w.position}")) {
                Text("${w.position}.", style = theme.body(color = theme.muted))
                who(w.label, w.task)
                Text(w.reason, style = theme.body())
                Text(
                    when {
                        w.interactive -> "interactive, ${age(w.since_ms)}"
                        w.landing -> "landing, ${age(w.since_ms)}"
                        else -> "${w.steps_remaining} steps from landing, ${age(w.since_ms)}"
                    },
                    style = theme.small(color = theme.muted),
                )
            }
        }
        c.notes.forEachIndexed { i, n ->
            Text(n, style = theme.small(color = theme.attention), modifier = Modifier.testTag("compile:note:$i"))
        }
        Text(c.rules, style = theme.small(color = theme.muted))
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
    Row(horizontalArrangement = Arrangement.spacedBy(theme.spacingLarge), verticalAlignment = Alignment.Top) {
    // Chosen by the daemon's own state; an unknown state draws nothing. Decorative.
    remember(r.state) { Mascot.illustration(r.state) }?.let {
        Image(it, contentDescription = null, contentScale = ContentScale.Fit, modifier = Modifier.size(theme.illustration))
    }
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
    }
    Row(horizontalArrangement = Arrangement.spacedBy(theme.spacing), verticalAlignment = Alignment.CenterVertically) {
        ProgressCells(model, r.progress, r.slow)
        if (r.typical.isNotEmpty()) Text(r.typical, style = theme.small())
    }
    r.progress.running?.let { ProgressDetail(model, it) }
    ActionButtons(model, r, hasApprovalView = d.approval != null)
    if (r.blockers.isNotEmpty()) {
        Row(horizontalArrangement = Arrangement.spacedBy(theme.spacingSmall), modifier = Modifier.testTag("waiting-for")) {
            Text("waiting for", style = theme.body(color = theme.muted))
            r.blockers.forEach { b ->
                Text("#$b", style = theme.body(color = theme.accent), modifier = Modifier.clickable { model.go(Page.Task(b)) })
            }
        }
    }
    BlockersView(model, r)
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
    d.proposal?.let { ProposalView(model, r, it) }
    if (d.failure.isNotEmpty()) Text("Failed: ${d.failure}", style = theme.body(color = theme.attention))
    if (d.step_tokens.isNotEmpty()) Section("Tokens") {
        d.step_tokens.forEach { Text(tokenLineText(it), style = theme.body()) }
    }
    Section("Steps") {
        d.steps.forEach { s ->
            Row(horizontalArrangement = Arrangement.spacedBy(theme.spacing), modifier = Modifier.testTag("step:${s.run}")) {
                Text("run ${s.run}", style = theme.body(color = theme.muted))
                Text(s.name.ifEmpty { s.category }, style = theme.body(FontWeight.SemiBold))
                Text("attempt ${s.attempt}", style = theme.body(color = theme.muted))
                Text(s.outcome, style = theme.body())
                r.progress.firstOrNull { it.step == s.name && it.state == CellState.PART && it.run == s.run }?.let { c ->
                    val now = rememberDaemonNow(model)
                    Text(c.countText, style = theme.body(color = theme.muted))
                    Text(c.timeText(now), style = theme.body(color = if (c.isStale(now)) theme.slow else theme.muted))
                }
            }
            // Every session this run started, so a retry up the ladder shows.
            s.launches.forEach { l ->
                Text("${l.kind}: ${l.display}", style = theme.small(color = theme.muted), modifier = Modifier.padding(start = theme.spacingLarge))
            }
        }
    }
    if (d.closing.isNotEmpty()) Section("Tests") { Text(d.closing, style = theme.body()) }
    if (d.debts.isNotEmpty()) Section("Debts") {
        d.debts.forEach { Text("#${it.id} ${it.scope}, ${it.status}${if (it.deferred) ", runs after landing" else ""}", style = theme.body()) }
    }
    if (d.plan.isNotEmpty() || d.plan_view != null) Section("Plan") { PlanSection(d) }
    if (d.body.isNotEmpty()) Section("Task") { SelectionContainer { Text(d.body, style = theme.body()) } }
    if (model.liveLog.isNotEmpty()) Section("Log") {
        SelectionContainer {
            Text(model.liveLog.joinToString("\n"), style = theme.mono(),
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
                if (l.queue.isEmpty()) {
                    MascotPicture("nothing_landing", theme.mascotEmpty)
                    Text("Nothing is landing.", style = theme.body(color = theme.muted))
                }
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
        model.usage?.providers?.forEach { UsageAccount(it) }
        model.usage?.note?.let { Text(it, style = theme.body(color = theme.muted)) }
        SessionStepper(model)
    }
}

/** The text of a window's percentage, worded as the conformance runner words it. */
fun windowPercentText(w: mason.v1.UsageWindow): String = when {
    !w.reported -> "not reported yet"
    w.has_percent -> "${Math.round(w.percent)}%"
    else -> "no percentage reported"
}

fun countText(n: Long): String = when {
    n < 1000 -> n.toString()
    n < 1_000_000 -> String.format("%.1fk", n / 1e3)
    else -> String.format("%.1fM", n / 1e6)
}

fun tokenLineText(l: mason.v1.TokenLine): String {
    val parts = mutableListOf<String>()
    if (l.has_input) parts += "in ${countText(l.input)}"
    if (l.has_output) parts += "out ${countText(l.output)}"
    if (l.has_cache_read) parts += "cache read ${countText(l.cache_read)}"
    if (l.has_cache_write) parts += "cache write ${countText(l.cache_write)}"
    if (l.has_cost) parts += String.format("\$%.2f", l.cost_usd)
    return "${l.name}: " + (if (parts.isEmpty()) "nothing reported" else parts.joinToString(", ")) + " (${l.age})"
}

/**
 * One provider account: its slots, each window as a bar with its reset, overage and blocked
 * text, spend for an api account, and tokens. Every figure carries its age.
 */
@Composable
fun UsageAccount(p: mason.v1.ProviderStatus) {
    val theme = LocalTheme.current
    Column(verticalArrangement = Arrangement.spacedBy(theme.spacingSmall)) {
        val budget = if (p.monthly_budget_usd > 0) ", budget \$${p.monthly_budget_usd.toInt()}/month" else ""
        Text(
            "${p.name} (${p.adapter_name}, ${p.billing}): ${p.status}, ${p.sessions_used} of ${p.sessions_limit} sessions",
            style = theme.body(),
            fontWeight = FontWeight.SemiBold,
            modifier = Modifier.testTag("usage:${p.name}"),
        )
        Text("warn at ${(p.warn_at * 100).toInt()}%, overage ${p.overage}$budget", style = theme.small(color = theme.muted))
        Text(if (p.has_snapshot) p.age else "nothing reported yet", style = theme.small(color = theme.muted))
        p.windows.forEach { w ->
            Column(Modifier.testTag("usage-window:${p.name}:${w.id}")) {
                Text("${w.label}: ${windowPercentText(w)}" + (if (w.reported) ", ${w.state}" else ""), style = theme.body())
                if (w.reported && w.has_percent) {
                    androidx.compose.material3.LinearProgressIndicator(
                        progress = { (w.percent / 100).toFloat().coerceIn(0f, 1f) },
                        color = if (w.state == "blocked") theme.attention else theme.accent,
                    )
                }
                if (w.reported) {
                    Text(if (w.has_reset) w.resets_text else "no reset time reported", style = theme.small(color = theme.muted))
                    if (w.trend.isNotEmpty()) Text(w.trend, style = theme.small(color = theme.muted))
                    Text(w.age, style = theme.small(color = theme.muted))
                }
            }
        }
        p.spend?.let { s ->
            if (s.has_spend) {
                Text(
                    String.format("Spend: \$%.2f today, \$%.2f this week, \$%.2f this month", s.today_usd, s.week_usd, s.month_usd) +
                        (if (s.has_budget) String.format(" of \$%.2f budget", s.budget_usd) else "") + " (${s.age})",
                    style = theme.body(),
                )
                if (s.has_budget && s.budget_usd > 0) {
                    androidx.compose.material3.LinearProgressIndicator(
                        progress = { (s.month_usd / s.budget_usd).toFloat().coerceIn(0f, 1f) },
                        color = theme.accent,
                    )
                }
            } else {
                Text("Spend: nothing reported yet", style = theme.body(color = theme.muted))
            }
        }
        if (p.overage_text.isNotEmpty()) Text(p.overage_text, style = theme.body())
        if (p.blocked_text.isNotEmpty()) Text(p.blocked_text, style = theme.body(color = theme.attention))
        listOf(p.tokens_window, p.tokens_week).forEach { t ->
            if (t != null && (t.by_project.isNotEmpty() || t.by_model.isNotEmpty() || t.top_tasks.isNotEmpty())) {
                Text("Tokens, ${t.period}", style = theme.body(), fontWeight = FontWeight.SemiBold)
                t.by_project.forEach { Text("project " + tokenLineText(it), style = theme.small()) }
                t.by_model.forEach { Text("model " + tokenLineText(it), style = theme.small()) }
                t.top_tasks.forEach { Text("task " + tokenLineText(it), style = theme.small()) }
            }
        }
        Text("Doing: ${p.doing}", style = theme.small(color = theme.muted))
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
            Text(h.cpuLine, style = theme.body())
            Section("Waiting") {
                if (h.waiting.isEmpty()) Text("Nothing is waiting.", style = theme.body(color = theme.muted))
                h.waiting.forEach { Text(it, style = theme.body()) }
            }
        }
        model.introspection?.let { i ->
            Section("Findings") {
                if (i.findings.isEmpty()) {
                    MascotPicture("all_clear", theme.mascotStatus)
                    Text("Nothing found yet.", style = theme.body(color = theme.muted))
                }
                i.findings.forEach { f ->
                    Text("#${f.id} [${f.verdict}] ${f.title} (proposal #${f.task}, metric ${f.metric})", style = theme.body())
                    if (f.text.isNotEmpty()) Text(f.text, style = theme.body(color = theme.muted))
                }
            }
            Section("Recurring jobs") { i.jobs.forEach { Text("${it.name} (${it.kind}): ${it.runs} run(s). ${it.status}", style = theme.body()) } }
            Section("Digest (last 7 days)") { i.digest.forEach { Text(it, style = theme.body()) } }
            Section("Triage") {
                Text(
                    if (i.triageOn) "On. ${i.triageWaiting} suggestion(s) wait for your review; triage never answers a question." else "Off.",
                    style = theme.body(),
                )
            }
        }
    }
}

/** A mascot picture: decorative, [size] already scaled with the text zoom, never tinted. An id the art lacks draws nothing. */
@Composable
fun MascotPicture(id: String, size: androidx.compose.ui.unit.Dp, modifier: Modifier = Modifier) {
    val bmp = remember(id) { Mascot.illustration(id) } ?: return
    androidx.compose.foundation.Image(bmp, contentDescription = null, modifier = modifier.size(size))
}
