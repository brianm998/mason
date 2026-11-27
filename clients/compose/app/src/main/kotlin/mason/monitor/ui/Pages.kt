// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.ScrollState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.Image
import androidx.compose.foundation.TooltipArea
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.input.pointer.PointerIcon
import androidx.compose.ui.input.pointer.pointerHoverIcon
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.foundation.horizontalScroll
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.flow.first
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
            Page.Settings -> SettingsPage(model)
            is Page.Section -> SectionPage(model, p.name)
        }
    }
}

/**
 * Home's CPU and disk sections: side by side (CPU about 3:2 wider) when the page is at least
 * [HOME_TWO_COLUMN_MIN_DP] wide, stacked with CPU first below that. With no CPU reading the disk
 * section takes the whole width, and the other way round.
 */
@Composable
internal fun CpuDiskRow(model: MonitorModel) {
    val theme = LocalTheme.current
    val cpu: @Composable () -> Unit = { CpuSection(model, open = { model.go(Page.Section("cpu")) }) }
    val disk: @Composable () -> Unit = { DiskSection(model, open = { model.go(Page.Section("disk")) }) }
    BoxWithConstraints(Modifier.fillMaxWidth()) {
        if (maxWidth.value >= HOME_TWO_COLUMN_MIN_DP && model.cpu != null && model.diskSeen) {
            Row(Modifier.fillMaxWidth().testTag("home:cpu-disk-row"), horizontalArrangement = Arrangement.spacedBy(theme.spacingLarge * 2)) {
                Box(Modifier.weight(HOME_CPU_WEIGHT).testTag("home:cpu-column")) { cpu() }
                Box(Modifier.weight(HOME_DISK_WEIGHT).testTag("home:disk-column")) { disk() }
            }
        } else {
            Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(theme.spacing)) {
                cpu()
                disk()
            }
        }
    }
}

@Composable
internal fun Section(title: String, state: String? = null, open: (() -> Unit)? = null, content: @Composable ColumnScope.() -> Unit) {
    val theme = LocalTheme.current
    Column(verticalArrangement = Arrangement.spacedBy(theme.spacing)) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(theme.spacingSmall)) {
            // The title and the '>' next to it both open the section's page.
            Text(title, style = theme.title(),
                modifier = if (open != null) Modifier.clickable(onClick = open).testTag("section-title:$title") else Modifier)
            if (state != null) StateIcon(state)
            if (open != null) {
                Text("›", style = theme.title(color = theme.muted),
                    modifier = Modifier.clickable(onClick = open).testTag("section-open:$title")
                        .semantics { contentDescription = "Open $title" })
            }
        }
        content()
    }
}

/**
 * The one look of a link: the link colour (`theme.link`), underlined, and the pointing hand on hover
 * (a touch screen ignores the cursor). Everything that opens a task's page or the tracker is drawn
 * with this, so a new link cannot forget a part of it.
 */
@Composable
internal fun Link(text: String, style: TextStyle? = null, modifier: Modifier = Modifier, onClick: () -> Unit) {
    val theme = LocalTheme.current
    Text(
        text, style = theme.linked(style ?: theme.body()), maxLines = 1, overflow = TextOverflow.Ellipsis,
        modifier = modifier.pointerHoverIcon(PointerIcon.Hand).clickable(onClick = onClick),
    )
}

/** [base] as link text: the link colour and an underline. For the title inside a row that is clickable as a whole. */
internal fun Theme.linked(base: TextStyle): TextStyle = base.copy(color = link, textDecoration = TextDecoration.Underline)

/** The pointing hand on hover, for a row that is clickable as a whole. */
internal fun Modifier.pointingHand(): Modifier = pointerHoverIcon(PointerIcon.Hand)

/**
 * A vertically scrolling page. Its position is put back to what the history saved when this
 * entry was left, once, when [ready] (the content has arrived) and only if the owner has not
 * scrolled meanwhile; a page that has shrunk clamps. Later updates never move it again.
 */
@Composable
private fun Scrolling(model: MonitorModel, ready: Boolean = true, content: @Composable ColumnScope.() -> Unit) {
    val theme = LocalTheme.current
    val visit = model.history.visit
    val target = model.history.restore
    val state = remember(visit) { ScrollState(0) }
    var settled by remember(visit) { mutableStateOf(target == 0) }
    LaunchedEffect(visit, ready) {
        if (ready && !settled) {
            // Wait for the content to be measured (or for the owner to scroll), then put it back.
            snapshotFlow { state.maxValue > 0 || state.value > 0 }.first { it }
            if (state.value == 0) state.scrollTo(target)
            settled = true
        }
    }
    LaunchedEffect(visit) {
        snapshotFlow { state.value }.collect { if (settled) model.reportScroll(it) }
    }
    Column(
        verticalArrangement = Arrangement.spacedBy(theme.spacingLarge),
        modifier = Modifier.fillMaxSize().verticalScroll(state).padding(theme.spacingLarge),
        content = content,
    )
}

/**
 * Home: the overall status, the attention queue, the active models with their task and
 * step, the session-limit stepper, the landing queue, and detail for the tasks in each
 * stage. (Counts and task lists live in the top bar and the sidebar, not here.)
 */
@OptIn(androidx.compose.foundation.ExperimentalFoundationApi::class)
@Composable
fun HomePage(model: MonitorModel) {
    val theme = LocalTheme.current
    Scrolling(model, ready = model.home != null) {
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
        if (model.versionLine.isNotEmpty()) {
            TooltipArea(tooltip = {
                Text(model.versionTip, style = theme.small(color = theme.text), modifier = Modifier.background(theme.surface).padding(theme.spacing))
            }) {
                SelectionContainer {
                    Text(model.versionLine, style = theme.small(), modifier = Modifier.testTag("home:version"))
                }
            }
        }
        if (adding) AddTaskDialog(model) { adding = false }
        h?.queue_hold?.let { QueueHoldBanner(model, it) }
        val attention = h?.attention.orEmpty()
        if (attention.isNotEmpty()) {
            Section("Needs you") {
                attention.forEach { a ->
                    val t = a.task ?: return@forEach
                    Column(verticalArrangement = Arrangement.spacedBy(theme.spacingSmall)) {
                        Link("${t.project} ${t.key}: ${t.title}", theme.body(FontWeight.SemiBold)) { model.go(Page.Task(t.id)) }
                        a.card?.let { DecisionCardView(model, t.id, it) }
                        a.approval?.let { ApprovalView(model, t.id, it) }
                    }
                }
            }
        }
        PlacementControl(model, PlacementScope.Global)
        h?.project?.takeIf { it.isNotEmpty() }?.let { PlacementControl(model, PlacementScope.Project(it)) }
        Row(horizontalArrangement = Arrangement.spacedBy(theme.spacingLarge * 2)) {
            Section("Active models", open = { model.go(Page.Section("active_models")) }) {
                val active = h?.active.orEmpty()
                val head = ActiveSummary.header(active)
                if (head == null) Text("Nothing is running.", style = theme.body(color = theme.muted))
                else Text(head, style = theme.body(FontWeight.SemiBold), modifier = Modifier.testTag("active:header"))
                ActiveSummary.github(h?.remote)?.let {
                    Text("On GitHub: $it", style = theme.body(color = theme.muted), modifier = Modifier.testTag("active:github"))
                }
                active.forEach { a ->
                    Row(horizontalArrangement = Arrangement.spacedBy(theme.spacing), modifier = Modifier.testTag("active:${a.task}")) {
                        Text(a.display.ifEmpty { a.model }, style = theme.body(FontWeight.SemiBold))
                        Link("${a.project} #${a.task}") { model.go(Page.Task(a.task)) }
                        Text(a.step, style = theme.body(color = theme.muted))
                        Text(ActiveSummary.state(a), style = theme.body(color = theme.muted))
                    }
                }
                SessionStepper(model)
            }
            Section("Landing queue", open = { model.go(Page.Landing) }) {
                val items = model.landing?.items.orEmpty()
                if (items.isNotEmpty()) {
                    LandingItems(model, items, model.landing?.batches.orEmpty())
                } else {
                    val queue = h?.stages?.firstOrNull { it.state == "landing" }?.rows.orEmpty()
                    if (queue.isEmpty()) Text("Nothing is landing.", style = theme.body(color = theme.muted))
                    queue.forEach { r -> Link("${r.key} ${r.title}") { model.go(Page.Task(r.id)) } }
                }
            }
        }
        CpuDiskRow(model)
        h?.github?.takeIf { it.enabled }?.let { GitHubSection(model, it, open = { model.go(Page.Section("github")) }) }
        h?.compilation?.let { CompilationSection(model, it, open = { model.go(Page.Section("compilation")) }) }
        h?.stages?.forEach { st ->
            Section(st.title, st.state, open = Page.sectionPage(st.state)?.let { p -> { model.go(p) } }) {
                st.rows.forEach { r ->
                    Row(horizontalArrangement = Arrangement.spacedBy(theme.spacing), verticalAlignment = Alignment.CenterVertically,
                        modifier = Modifier.pointingHand().clickable { model.go(Page.Task(r.id)) }) {
                        ProgressCells(model, r.progress, r.slow)
                        Text("${r.project} ${r.key}", style = theme.linked(theme.body(FontWeight.SemiBold)))
                        Text(r.title, style = theme.linked(theme.body()))
                        r.wait?.let { WaitBadge(it.cause) }
                        // LinkedText: a task this line names opens that task; elsewhere the row's click still applies.
                        LinkedText(
                            model,
                            if (r.why_waiting.isEmpty()) Format.duration(r.inState(model.daemonNow())) else "${r.why_waiting}, ${Format.duration(r.inState(model.daemonNow()))}",
                            theme.small(color = if (r.slow) theme.slow else theme.muted),
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
        Text("Sessions (all projects): $used of", style = theme.body())
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
fun CompilationSection(model: MonitorModel, c: Compilation, open: (() -> Unit)? = null) {
    val theme = LocalTheme.current
    val now = model.daemonNow()
    val age = { since: Long -> Format.duration(maxOf(0L, now - since)) }
    val who: @Composable (String, Long, String) -> Unit = { label, task, command ->
        if (task > 0) Link("#$task") { model.go(Page.Task(task)) }
        else if (command.isNotEmpty()) Link(label, modifier = Modifier.widthIn(max = 480.dp).testTag("compile:command")) { model.openCommand(label, command) }
        else Text(label, style = theme.body(), maxLines = 1, overflow = TextOverflow.Ellipsis,
            modifier = Modifier.widthIn(max = 480.dp))
    }
    Section("Compilation (all projects)", open = open) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(theme.spacing),
            modifier = Modifier.testTag("compile")) {
            Text("Compiling: ${c.used} of at most", style = theme.body())
            Pill("−", modifier = Modifier.testTag("compile:decrease")) { model.setCompileLimit(if (c.limit > 1) c.limit - 1 else 1) }
            Text("${c.limit}", style = theme.body(FontWeight.SemiBold))
            Pill("+", modifier = Modifier.testTag("compile:increase")) { model.setCompileLimit(c.limit + 1) }
            Text(c.cpuLine, style = theme.body(color = theme.muted))
            c.policyLine?.let { Text(it, style = theme.body(color = theme.muted)) }
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
                who(r.label, r.task, r.command)
                val detail = Format.compileHolder(r.project, r.title)
                if (r.task > 0 && detail.isNotEmpty()) Text(detail, style = theme.body(), maxLines = 1, overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.widthIn(max = 480.dp))
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
                who(w.label, w.task, w.command)
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
        if (c.leases.isNotEmpty()) Text("Commands with no task", style = theme.body(FontWeight.SemiBold))
        c.leases.forEachIndexed { i, l ->
            Row(horizontalArrangement = Arrangement.spacedBy(theme.spacing), verticalAlignment = Alignment.CenterVertically,
                modifier = Modifier.testTag("compile:lease:$i")) {
                who(l.label, 0L, l.command)
                LinkedText(model, QueueWait.leaseDetail(l, now), theme.body(color = theme.muted))
            }
        }
        c.notes.forEachIndexed { i, n ->
            Text(n, style = theme.small(color = theme.attention), modifier = Modifier.testTag("compile:note:$i"))
        }
        Text(c.rules, style = theme.small(color = theme.muted))
    }
    model.commandDialog?.let { CommandDialogView(it) { model.closeCommand() } }
}

/** A command shown whole: scrollable, monospaced, selectable, with Copy and Close; Escape closes it. */
@Composable
fun CommandDialogView(d: CommandDialog, close: () -> Unit) {
    val theme = LocalTheme.current
    androidx.compose.material3.AlertDialog(
        onDismissRequest = close,
        modifier = Modifier.onPreviewKeyEvent { e ->
            if (e.type == KeyEventType.KeyDown && e.key == Key.Escape) { close(); true } else false
        },
        title = { Text(d.title, style = theme.body(FontWeight.SemiBold), maxLines = 2) },
        text = {
            Box(Modifier.heightIn(max = 400.dp).verticalScroll(rememberScrollState()).horizontalScroll(rememberScrollState())) {
                SelectionContainer { Text(d.command, style = theme.mono(), modifier = Modifier.testTag("command:text")) }
            }
        },
        confirmButton = { androidx.compose.material3.Button(onClick = { copyToClipboard(d.command) }, modifier = Modifier.testTag("command:copy")) { Text("Copy") } },
        dismissButton = { androidx.compose.material3.Button(onClick = close, modifier = Modifier.testTag("command:close")) { Text("Close") } },
    )
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
        // The sidebar already has this task: draw it now, fill in the rest when masond sends the page.
        val known = if (model.connected) model.sidebarRow(id) else null
        if (known != null) {
            Scrolling(model) { TaskDetail(model, TaskDetailView(row = known), loading = true) }
            return
        }
        Text(if (model.connected) "Loading task #$id…" else "Not connected", style = theme.body(color = theme.muted),
            modifier = Modifier.padding(theme.spacingLarge))
        return
    }
    Scrolling(model) { TaskDetail(model, d) }
}

@Composable
private fun TaskDetail(model: MonitorModel, d: TaskDetailView, loading: Boolean = false) {
    val theme = LocalTheme.current
    val r = d.row ?: return
    Row(horizontalArrangement = Arrangement.spacedBy(theme.spacingLarge), verticalAlignment = Alignment.Top) {
    // Chosen by the daemon's own state; an unknown state draws nothing. Decorative.
    remember(r.state) { Mascot.illustration(r.state) }?.let {
        Image(it, contentDescription = null, contentScale = ContentScale.Fit, modifier = Modifier.size(theme.illustration))
    }
    Column(verticalArrangement = Arrangement.spacedBy(theme.spacingSmall)) {
        Text("${r.key} ${r.title}", style = theme.title(), modifier = Modifier.testTag("detail:${r.id}"))
        Text("${r.project}, ${r.state.replace('_', ' ')}, ${Format.duration(r.inState(model.daemonNow()))} in this state", style = theme.body(color = theme.muted))
        if (r.why_waiting.isNotEmpty()) {
            Row(horizontalArrangement = Arrangement.spacedBy(theme.spacing), verticalAlignment = Alignment.CenterVertically) {
                r.wait?.let { WaitBadge(it.cause) }
                LinkedText(model, r.why_waiting, theme.body(), Modifier.testTag("detail:why-waiting"))
            }
        }
        r.wait?.let { w ->
            QueueWait.chain(w, r.key).forEachIndexed { i, line ->
                LinkedText(model, line, theme.small(color = theme.muted), Modifier.testTag("detail:wait-chain:$i"))
            }
        }
        if (r.issue != 0L || d.ci_links.isNotEmpty()) CiLinksLine(d.ci_links, r.issue, r.issue_url)
        Text(cpuTaskLine(d.cpu_permille), style = theme.body(color = theme.muted), modifier = Modifier.testTag("cpu"))
        PlacementControl(model, PlacementScope.Task(r.id))
        Row(horizontalArrangement = Arrangement.spacedBy(theme.spacingLarge)) {
            CopyButton(r.key, r.key)
            CopyTarget.detail(d.branch, d.worktree).forEach { t ->
                CopyValueButton(t) { (model.copier ?: ::copyToClipboard)(it) }
            }
        }
    }
    }
    Row(horizontalArrangement = Arrangement.spacedBy(theme.spacing), verticalAlignment = Alignment.CenterVertically) {
        ProgressCells(model, r.progress, r.slow)
        if (r.typical.isNotEmpty()) Text(r.typical, style = theme.small())
    }
    r.progress.running?.let { ProgressDetail(model, it) }
    val failedPage = if (r.state == "failed") FailedPage.choose(d.card != null, d.diagnosis_running != null, r.allowed_actions) else null
    if (failedPage == FailedPage.Card) {
        // The diagnosis first: its summary is the card's why, the failure line under it.
        if (failedPage != FailedPage.Card) d.card?.let { DecisionCardView(model, r.id, it) }
        if (d.failure.isNotEmpty() && failedPage != FailedPage.Card) LinkedText(model, "Failed: ${d.failure}", theme.body(color = theme.attention), Modifier.testTag("detail:failure"))
    }
    if (failedPage == FailedPage.Diagnosing) {
        d.diagnosis_running?.let {
            Text(diagnosingLine(it.started_ms, it.ran_on, model.daemonNow()), style = theme.body(color = theme.attention), modifier = Modifier.testTag("detail:diagnosing"))
        }
    }
    ActionButtons(model, r, hasApprovalView = d.approval != null, failedPage = failedPage)
    if (loading) Text("Loading details…", style = theme.body(color = theme.muted), modifier = Modifier.testTag("detail:loading"))
    if (r.blockers.isNotEmpty()) {
        Row(horizontalArrangement = Arrangement.spacedBy(theme.spacingSmall), modifier = Modifier.testTag("waiting-for")) {
            Text("waiting for", style = theme.body(color = theme.muted))
            r.blockers.forEach { b ->
                Link("#$b") { model.go(Page.Task(b)) }
            }
        }
    }
    BlockersView(model, r)
    if (r.blocks.isNotEmpty()) {
        Row(horizontalArrangement = Arrangement.spacedBy(theme.spacingSmall), modifier = Modifier.testTag("blocks")) {
            Text("blocks", style = theme.body(color = theme.muted))
            r.blocks.forEach { b ->
                Link("#$b") { model.go(Page.Task(b)) }
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
    if (d.failure.isNotEmpty()) LinkedText(model, "Failed: ${d.failure}", theme.body(color = theme.attention), Modifier.testTag("detail:failure"))
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
                LinkedText(model, "${l.kind}: ${l.display}", theme.small(color = theme.muted), Modifier.padding(start = theme.spacingLarge))
            }
        }
    }
    if (d.closing.isNotEmpty()) Section("Tests") { LinkedText(model, d.closing, theme.body(), Modifier.testTag("detail:closing")) }
    if (d.review_findings.isNotEmpty()) Section("Review findings") {
        d.review_findings.forEach { LinkedText(model, it, theme.body()) }
    }
    if (d.debts.isNotEmpty()) Section("Debts") {
        // The debt's own `#id` is not a task, so only the rest is linked.
        d.debts.forEach {
            Row {
                Text("#${it.id} ", style = theme.body())
                LinkedText(model, "${it.scope}, ${it.status}${if (it.deferred) ", runs after landing" else ""}", theme.body())
            }
        }
    }
    if (d.plan.isNotEmpty() || d.plan_view != null) Section("Plan") { PlanSection(model, d) }
    if (d.body.isNotEmpty()) Section("Task") { LinkedText(model, d.body, theme.body(), Modifier.testTag("detail:body"), selectable = true) }
    if (model.liveLog.isNotEmpty()) Section("Log") {
        LinkedText(model, model.liveLog.joinToString("\n"), theme.mono(),
            Modifier.fillMaxWidth().background(theme.surface, RoundedCornerShape(theme.radius)).padding(theme.spacing),
            selectable = true)
    }
}

@Composable
fun LandingPage(model: MonitorModel) {
    val theme = LocalTheme.current
    Scrolling(model) {
        Text("Landing", style = theme.title())
        model.landing?.let { l ->
            if (l.note.isNotEmpty()) Text(l.note, style = theme.body(color = if (l.frozen) theme.attention else theme.muted))
            l.modes.forEach { Text(it, style = theme.body(color = theme.muted)) }
            // One line per project: tracker off, polling with counts, or stopped with why.
            l.trackers.forEach { t ->
                Text(t, style = theme.body(color = if (t.contains("stopped") || t.contains(" off;")) theme.attention else theme.muted))
            }
            l.freezes.forEach { f ->
                LinkedText(
                    model, "Frozen: ${f.project}: ${f.reason}${if (f.fix_task > 0) " (fix: task #${f.fix_task})" else ""}",
                    theme.body(color = theme.attention),
                )
            }
            Section("Queue") {
                if (l.queue.isEmpty()) {
                    MascotPicture("nothing_landing", theme.mascotEmpty)
                    Text("Nothing is landing.", style = theme.body(color = theme.muted))
                }
                l.queue.forEach { r -> Link("${r.key} ${r.title}") { model.go(Page.Task(r.id)) } }
            }
            if (l.items.isNotEmpty()) Section("Landing tasks") { LandingItems(model, l.items, l.batches) }
            Section("Batches") {
                if (l.batches.isEmpty()) Text("No batch has formed yet.", style = theme.body(color = theme.muted))
                l.batches.forEach { b ->
                    Row(horizontalArrangement = Arrangement.spacedBy(theme.spacing)) {
                        LinkedText(model, batchText(b), theme.body())
                        LandNowButton(model, LandNowTarget.Batch(b.id), model.canLandNow(b))
                    }
                    b.ci?.let { ci ->
                        val uri = androidx.compose.ui.platform.LocalUriHandler.current
                        Row(horizontalArrangement = Arrangement.spacedBy(theme.spacing)) {
                            Text(
                                batchCiText(ci),
                                style = theme.body(color = if (batchCiFailed(ci)) theme.attention else theme.muted),
                            )
                            batchCiUrl(ci)?.let { u -> Link("CI run") { uri.openUri(u) } }
                        }
                    }
                }
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
                    Row {
                        Text("#${x.id} ", style = theme.body())
                        LinkedText(model, "task #${x.task} ${x.scope}: ${x.reason.ifEmpty { "no reason given" }}", theme.body())
                    }
                }
            }
        }
    }
}

/** One line per batch: where it is, who landed, who was parked, and why. */
fun batchText(b: mason.v1.BatchLine): String {
    fun list(v: List<Long>) = v.joinToString(" ") { "#$it" }
    var t = "Batch #${b.id} ${b.project} ${b.state_text.ifEmpty { b.phase }}${if (b.running) " (running)" else ""}: ${list(b.tasks)}"
    if (b.landed.isNotEmpty()) t += ", landed ${list(b.landed)}"
    if (b.parked.isNotEmpty()) t += ", parked ${list(b.parked)}"
    if (b.note.isNotEmpty()) t += ". ${b.note}"
    return t
}

/** The CI line under a batch: per workflow "pr-gate: 11 jobs · 1 queued · 2 running · 7 passed". */
fun batchCiText(ci: mason.v1.BatchCi): String {
    val parts = ci.workflows.map { w ->
        val total = w.queued + w.running + w.passed + w.failed
        if (w.running + w.passed + w.failed == 0) {
            "${w.workflow}: queued for a runner"
        } else {
            "${w.workflow}: $total jobs · ${w.queued} queued · ${w.running} running · ${w.passed} passed" +
                if (w.failed > 0) " · ${w.failed} failed" else ""
        }
    }
    return "CI: " + (if (parts.isEmpty()) "queued for a runner" else parts.joinToString("; "))
}

fun batchCiFailed(ci: mason.v1.BatchCi): Boolean = ci.workflows.any { it.failed > 0 }

/** The run's address, only if it is http or https. */
fun batchCiUrl(ci: mason.v1.BatchCi): String? =
    ci.url.takeIf { it.startsWith("https://") || it.startsWith("http://") }

@Composable
fun UsagePage(model: MonitorModel) {
    val theme = LocalTheme.current
    Scrolling(model) {
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
fun HealthPage(
    model: MonitorModel,
    lateness: () -> LatenessWindow = { UiLateness.recent() },
    backgroundLateness: () -> LatenessWindow = { UiLateness.recentBackground() },
) {
    val theme = LocalTheme.current
    Scrolling(model) {
        Text("Health", style = theme.title())
        Text(if (model.connected) "Connected to masond. Event log at #${model.lastSeq}." else "Not connected.", style = theme.body())
        // Read again whenever the page redraws (the health poll and the event log do that).
        val window = remember(model.health, model.lastSeq) { lateness() }
        Text(window.line(windowMinutes = 10), style = theme.body(), modifier = Modifier.testTag("health:ui-thread"))
        val background = remember(model.health, model.lastSeq) { backgroundLateness() }
        Text(background.backgroundLine(windowMinutes = 10), style = theme.small(), modifier = Modifier.testTag("health:ui-thread-background"))
        model.health?.let { h ->
            Section("Resources") { h.resources.forEach { Text("${it.name} (${it.kind}): ${it.used} of ${it.limit} in use", style = theme.body()) } }
            Text(h.cpuLine, style = theme.body())
            h.policyLine?.let { Text(it, style = theme.body()) }
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
                    // The finding's own #id is not a task; only "(proposal #N)" is.
                    Row {
                        Text("#${f.id} [${f.verdict}] ${f.title} ", style = theme.body())
                        LinkedText(model, "(proposal #${f.task}, metric ${f.metric})", theme.body())
                    }
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

/** "Landing now" (in a batch) above "Queued", one line per task. */
@Composable
internal fun LandingItems(model: MonitorModel, items: List<mason.v1.TaskLanding>, batches: List<mason.v1.BatchLine>) {
    val theme = LocalTheme.current
    val g = landingGroups(items)
    if (g.first.isNotEmpty()) {
        Text("Landing now", style = theme.body(color = theme.muted))
        val layout = landingLayout(items, batches)
        layout.groups.forEach { grp ->
            Row(horizontalArrangement = Arrangement.spacedBy(theme.spacing), modifier = Modifier.testTag("batchheader:${grp.batch}")) {
                LinkedText(model, batchHeaderText(grp), theme.body(color = batchSeverityColor(grp.header?.severity ?: "", theme)))
                grp.header?.let { LandNowButton(model, LandNowTarget.Batch(it.id), model.canLandNow(it)) }
            }
            val why = grp.header?.severity_reason.orEmpty()
            if (why.isNotEmpty() && grp.header?.severity != "ok") {
                Text(why, style = theme.body(color = batchSeverityColor(grp.header?.severity ?: "", theme)))
            }
            grp.members.forEach { i ->
                Row(horizontalArrangement = Arrangement.spacedBy(theme.spacing), modifier = Modifier.padding(start = theme.spacingLarge)) {
                    Link(landingItemText(i)) { model.go(Page.Task(i.task)) }
                    LandNowButton(model, LandNowTarget.Task(i.task), model.canLandNow(i))
                }
            }
        }
        layout.loose.forEach { i ->
            Row(horizontalArrangement = Arrangement.spacedBy(theme.spacing)) {
                Link(landingItemText(i)) { model.go(Page.Task(i.task)) }
                LandNowButton(model, LandNowTarget.Task(i.task), model.canLandNow(i))
            }
        }
    }
    if (g.second.isNotEmpty()) {
        Text("Queued", style = theme.body(color = theme.muted))
        g.second.forEach { i ->
            Row(horizontalArrangement = Arrangement.spacedBy(theme.spacing), modifier = Modifier.padding(start = theme.spacingLarge)) {
                Link(landingItemText(i)) { model.go(Page.Task(i.task)) }
                LandNowButton(model, LandNowTarget.Task(i.task), model.canLandNow(i))
            }
        }
    }
}

/**
 * "Runs on" for one level (#834): the mode in force and where it came from, the four modes, and
 * "Follow MASON" / "Follow project" to clear the level's own choice. Owner devices that can act
 * only; a one-line confirm, then the pending label until the next view arrives.
 */
@Composable
internal fun PlacementControl(model: MonitorModel, scope: PlacementScope) {
    val theme = LocalTheme.current
    val p = model.placement(scope) ?: return
    val key = when (scope) {
        PlacementScope.Global -> "global"
        is PlacementScope.Project -> "project:${scope.name}"
        is PlacementScope.Task -> "task:${scope.id}"
    }
    val pending = model.placementPendingLabel(scope)
    Column(verticalArrangement = Arrangement.spacedBy(theme.spacingSmall)) {
        Row(horizontalArrangement = Arrangement.spacedBy(theme.spacing), verticalAlignment = Alignment.CenterVertically) {
            Text("Runs on", style = theme.body(FontWeight.SemiBold))
            Text(placementText(p), style = theme.body(color = theme.muted), modifier = Modifier.testTag("placement:$key:text"))
            if (pending != null) Text(pending, style = theme.small(color = theme.muted), modifier = Modifier.testTag("placement:$key:pending"))
        }
        if (model.canSetPlacement(scope) && pending == null) {
            Row(horizontalArrangement = Arrangement.spacedBy(theme.spacingSmall)) {
                placementModes.forEach { m ->
                    Pill(m, prominent = m == p.mode, modifier = Modifier.testTag("placement:$key:$m")) { model.askPlacement(scope, m) }
                }
                val follow = scope.followLabel
                if (follow != null && model.canFollow(scope)) {
                    Pill(follow, modifier = Modifier.testTag("placement:$key:default")) { model.askPlacement(scope, "default") }
                }
            }
        }
    }
    val ask = model.placementAsk
    if (ask != null && ask.scope == scope) {
        androidx.compose.material3.AlertDialog(
            onDismissRequest = { model.cancelPlacement() },
            text = { Text(ask.confirmText) },
            confirmButton = {
                androidx.compose.material3.Button(onClick = { model.confirmPlacement() }, modifier = Modifier.testTag("placement-confirm")) { Text("Change") }
            },
            dismissButton = {
                androidx.compose.material3.Button(onClick = { model.cancelPlacement() }) { Text("Cancel") }
            },
        )
    }
}

/** The Land now button of the landing queue: owner devices only ([shown]), a one-line confirm, then the pending-command label until the queue changes. */
@Composable
internal fun LandNowButton(model: MonitorModel, target: LandNowTarget, shown: Boolean) {
    if (!shown) return
    val pending = model.landNowLabel(target)
    androidx.compose.material3.OutlinedButton(
        onClick = { model.askLandNow(target) },
        enabled = pending == null,
        modifier = Modifier.testTag("landnow:${target.pendingKey.first}"),
    ) { Text(pending ?: "Land now") }
    if (model.landNowAsk == target) {
        androidx.compose.material3.AlertDialog(
            onDismissRequest = { model.cancelLandNow() },
            text = { Text(target.confirmText) },
            confirmButton = {
                androidx.compose.material3.Button(onClick = { model.confirmLandNow() }, modifier = Modifier.testTag("landnow-confirm")) { Text("Land now") }
            },
            dismissButton = {
                androidx.compose.material3.Button(onClick = { model.cancelLandNow() }) { Text("Cancel") }
            },
        )
    }
}

/** Tasks in a batch, then tasks still queued (in queue order). */
fun landingGroups(items: List<mason.v1.TaskLanding>): Pair<List<mason.v1.TaskLanding>, List<mason.v1.TaskLanding>> =
    items.filter { it.status == "in_batch" } to items.filter { it.status != "in_batch" }.sortedBy { it.position }

/** One running batch with its tasks (s788). [header] is null when an older daemon sent only the task rows. */
data class LandingBatchGroup(val batch: Long, val header: mason.v1.BatchLine?, val members: List<mason.v1.TaskLanding>)

/** "Landing now": one group per running batch, in batch order, above the landing rows that belong to no batch. */
class LandingLayout(val groups: List<LandingBatchGroup>, val loose: List<mason.v1.TaskLanding>)

fun landingLayout(items: List<mason.v1.TaskLanding>, batches: List<mason.v1.BatchLine>): LandingLayout {
    val landing = items.filter { it.status == "in_batch" }
    val ids = landing.map { it.batch }.filter { it != 0L }.toSortedSet()
    return LandingLayout(
        ids.map { id -> LandingBatchGroup(id, batches.firstOrNull { it.id == id }, landing.filter { it.batch == id }) },
        landing.filter { it.batch == 0L },
    )
}

/** "Batch #494 · 10 tasks · running suites · 17 min (usually ~6 min)". */
fun batchHeaderText(g: LandingBatchGroup): String {
    val parts = mutableListOf("Batch #${g.batch}")
    val n = g.header?.members?.size ?: 0
    val count = if (n > 0) n else g.members.size
    parts += "$count task${if (count == 1) "" else "s"}"
    val b = g.header ?: return parts.joinToString(" · ")
    parts += b.state_text.ifEmpty { b.phase }
    var time = "${b.phase_ms / 60000} min"
    if (b.typical_ms > 0) time += " (usually ~${maxOf(1L, b.typical_ms / 60000)} min)"
    parts += time
    return parts.joinToString(" · ")
}

/** Stuck is the attention colour, a warning the slow (amber) one, else the normal text. */
fun batchSeverityColor(severity: String, theme: Theme) = when (severity) {
    "stuck" -> theme.attention
    "warn" -> theme.slow
    else -> theme.text
}

/** "▰▰▱▱" for phase 2 of 4. */
fun phaseBar(index: Int, total: Int): String {
    val n = index.coerceIn(0, total)
    return "▰".repeat(n) + "▱".repeat(total - n)
}

fun landingItemText(i: mason.v1.TaskLanding): String {
    // Under a batch header the row is only the task and its own standing (a daemon of protocol 2).
    if (i.status == "in_batch" && i.standing.isNotEmpty()) return "${i.key} ${i.title}: ${i.standing}"
    if (i.status == "in_batch") {
        var t = "${i.key} ${i.title}: batch #${i.batch}, ${i.phase}, phase ${i.phase_index} of ${i.phase_total} ${phaseBar(i.phase_index, i.phase_total)}"
        if (i.ci_state.isNotEmpty()) t += ", CI ${i.ci_state}"
        return t + ", ${i.phase_ms / 60000} min"
    }
    return "${i.key} ${i.title}: queued #${i.position}, ${i.why}"
}

/**
 * Settings (s430): proposal policy, notification routes with a Test button, schedules with Run now,
 * and the tracker. Every control sends one small change to the daemon, which writes it to the file
 * and applies it without a restart. A secret is never shown or typed here.
 */
@Composable
fun SettingsPage(model: MonitorModel) {
    val theme = LocalTheme.current
    Scrolling(model) {
        Text("Settings", style = theme.title())
        model.settingsNotice?.let { Text(it, style = theme.body(color = theme.muted), modifier = Modifier.testTag("settings:notice")) }
        val s = model.settings
        if (s == null) {
            Text("Loading the project's settings.", style = theme.body(color = theme.muted))
        } else {
            Section("Proposal policy (${s.project})") {
                SettingsSnapshot.DISPOSITION_KEYS.forEach { k ->
                    SettingChoice(k, "settings:$k", s.dispositions[k] ?: "proposed") { model.changeProjectSetting(SettingsPayload.disposition(k, it)) }
                }
                Text("Introspection findings, by severity and cost", style = theme.small(color = theme.muted))
                SettingsSnapshot.INTROSPECTION_KEYS.forEach { k ->
                    SettingChoice(k, "settings:introspection:$k", s.introspection[k] ?: "proposed") { model.changeProjectSetting(SettingsPayload.introspection(k, it)) }
                }
                SettingStepper("At most ${s.perTask} per task", s.perTask, 1, 50, "settings:per_task") { model.changeProjectSetting(SettingsPayload.cap("per_task", it)) }
                SettingStepper("At most ${s.perDay} per day", s.perDay, 1, 500, "settings:per_day") { model.changeProjectSetting(SettingsPayload.cap("per_day", it)) }
            }
            Section("Recurring jobs") {
                if (s.schedules.isEmpty()) Text("None. Add one to the project file.", style = theme.body(color = theme.muted))
                s.schedules.forEach { j -> ScheduleRow(model, j) }
            }
            Section("Issue tracker") {
                val t = s.tracker
                if (t == null) {
                    Text("No tracker is configured for this project.", style = theme.body(color = theme.muted))
                } else {
                    SettingStepper("Look every ${t.intervalS} s", t.intervalS, 5, 3600, "settings:tracker:interval_s", step = 5) { model.changeProjectSetting(SettingsPayload.tracker("interval_s", it)) }
                    SettingChoice("import as", "settings:tracker:import_as", t.importAs) { model.changeProjectSetting(SettingsPayload.tracker("import_as", it)) }
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        androidx.compose.material3.Checkbox(
                            checked = t.requireLabel,
                            onCheckedChange = { model.changeProjectSetting(SettingsPayload.tracker("require_label", it)) },
                            modifier = Modifier.testTag("settings:tracker:require_label"),
                        )
                        Text("Only issues with the label \"${t.label}\"", style = theme.body())
                    }
                    if (t.token.isNotEmpty()) Text("Token: ${t.token}", style = theme.small(color = theme.muted))
                }
            }
        }
        if (s != null) {
            Section("Where runs go") {
                val r = model.remoteSettings
                if (r == null) Text("Loading where runs go.", style = theme.body(color = theme.muted))
                else RemotePlacementBody(r, s.project, { model.changeGlobalMode(it) }, { model.changeProjectMode(it) })
            }
        }
        model.notifySettings?.let { n ->
            Section("Notifications") {
                n.routes.forEach { r ->
                    Text(r.condition, style = theme.body())
                    Row(horizontalArrangement = Arrangement.spacedBy(theme.spacing), verticalAlignment = Alignment.CenterVertically) {
                        n.channels.forEach { ch ->
                            androidx.compose.material3.Checkbox(
                                checked = n.sends(r.condition, ch),
                                onCheckedChange = { on -> model.changeNotifySetting(SettingsPayload.route(r.condition, ch, if (on) "0" else null)) },
                                modifier = Modifier.testTag("settings:route:${r.condition}:$ch"),
                            )
                            Text(ch, style = theme.small())
                        }
                    }
                }
                Row(horizontalArrangement = Arrangement.spacedBy(theme.spacing)) {
                    n.channels.filter { it != "monitor" }.forEach { ch ->
                        androidx.compose.material3.TextButton(onClick = { model.testNotification(ch) }, modifier = Modifier.testTag("settings:test:$ch")) { Text("Test $ch") }
                    }
                }
            }
        }
        DevicesSection(model)
    }
}

/** The body of the "Where runs go" section (#891): the global mode and the project's. */
@Composable
internal fun RemotePlacementBody(remote: RemoteSettings, project: String, pickGlobal: (String) -> Unit, pickProject: (String) -> Unit) {
    val theme = LocalTheme.current
    SettingChoice("All of MASON", "settings:remote:global", remote.mode, options = RemoteSettings.MODES, onPick = pickGlobal)
    remote.problem?.let { Text(it, style = theme.small(color = theme.muted)) }
    remote.changedText?.let { Text(it, style = theme.small(color = theme.muted)) }
    remote.project(project)?.let { p ->
        SettingChoice(project, "settings:remote:project", p.mode, options = RemoteSettings.MODES, onPick = pickProject)
        Text(p.sourceText, style = theme.small(color = theme.muted))
        if (p.runsOn.isNotEmpty()) Text("GitHub runner: ${p.runsOn}", style = theme.small(color = theme.muted))
    }
    Text(RemoteSettings.EXPLANATION, style = theme.small(color = theme.muted))
}

@Composable
internal fun SettingChoice(
    title: String, tag: String, value: String,
    options: List<String> = SettingsSnapshot.DISPOSITIONS, enabled: Boolean = true, onPick: (String) -> Unit,
) {
    val theme = LocalTheme.current
    val pick = onPick
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(theme.spacing)) {
        Text(title, style = theme.body())
        options.forEach { o ->
            androidx.compose.material3.TextButton(onClick = { if (o != value) pick(o) }, enabled = enabled, modifier = Modifier.testTag("$tag:$o")) {
                Text(if (o == value) "[$o]" else o, style = theme.body())
            }
        }
    }
}

@Composable
private fun SettingStepper(label: String, value: Int, min: Int, max: Int, tag: String, step: Int = 1, set: (Int) -> Unit) {
    val theme = LocalTheme.current
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(theme.spacing)) {
        Text(label, style = theme.body())
        androidx.compose.material3.TextButton(onClick = { set((value - step).coerceAtLeast(min)) }, modifier = Modifier.testTag("$tag:less")) { Text("-") }
        androidx.compose.material3.TextButton(onClick = { set((value + step).coerceAtMost(max)) }, modifier = Modifier.testTag("$tag:more")) { Text("+") }
    }
}

@Composable
private fun ScheduleRow(model: MonitorModel, job: SettingsSnapshot.Schedule) {
    val theme = LocalTheme.current
    var every by remember(job.every) { mutableStateOf(job.every) }
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(theme.spacing)) {
        Text("${job.name} (${job.kind}) every", style = theme.body())
        androidx.compose.foundation.text.BasicTextField(
            value = every,
            onValueChange = { every = it },
            singleLine = true,
            textStyle = theme.body(),
            keyboardActions = androidx.compose.foundation.text.KeyboardActions(onDone = { model.changeProjectSetting(SettingsPayload.schedule(job.name, every)) }),
            keyboardOptions = androidx.compose.foundation.text.KeyboardOptions(imeAction = androidx.compose.ui.text.input.ImeAction.Done),
            modifier = Modifier.width(90.dp).testTag("settings:every:${job.name}"),
        )
        androidx.compose.material3.TextButton(onClick = { model.runSchedule(job.name) }, modifier = Modifier.testTag("settings:run:${job.name}")) { Text("Run now") }
    }
}

/**
 * A detail page for one section of home (`mason://section/<name>`): only that section, with
 * room for a task's progress, step, why it waits, time in state and model on one wide row.
 * The rows are the sidebar's, so they come in its order.
 */
@Composable
fun SectionPage(model: MonitorModel, name: String) {
    val theme = LocalTheme.current
    Scrolling(model) {
        when (name) {
            "active_models" -> Section(SectionPages.title(name)) {
                val active = model.home?.active.orEmpty()
                val uri = androidx.compose.ui.platform.LocalUriHandler.current
                val head = ActiveSummary.header(active)
                if (head == null) Text("Nothing is running.", style = theme.body(color = theme.muted))
                else Text(head, style = theme.body(FontWeight.SemiBold))
                active.forEach { a ->
                    Row(horizontalArrangement = Arrangement.spacedBy(theme.spacing), modifier = Modifier.testTag("active:${a.task}")) {
                        Text(a.display.ifEmpty { a.model }, style = theme.body(FontWeight.SemiBold))
                        Link("${a.project} #${a.task}") { model.go(Page.Task(a.task)) }
                        Text(a.step, style = theme.body(color = theme.muted))
                        Text(ActiveSummary.state(a), style = theme.body(color = theme.muted))
                        if (a.run_url.isNotEmpty()) Link("run") { runCatching { uri.openUri(a.run_url) } }
                    }
                }
                val remote = model.home?.remote
                ActiveSummary.github(remote)?.let { gh ->
                    Text("On GitHub: $gh", style = theme.body(FontWeight.SemiBold))
                    remote?.runs.orEmpty().forEach { r ->
                        Row(horizontalArrangement = Arrangement.spacedBy(theme.spacing)) {
                            Text(ActiveSummary.runLine(r), style = theme.body())
                            if (r.url.isNotEmpty()) Link("run") { runCatching { uri.openUri(r.url) } }
                            else Text("starting", style = theme.body(color = theme.muted))
                        }
                    }
                }
                SessionStepper(model)
            }
            "cpu" -> CpuSection(model)
            "disk" -> DiskSection(model)
            "github" -> {
                val g = model.home?.github
                if (g != null && g.enabled) GitHubSection(model, g, detail = true)
                else Section(SectionPages.title(name)) { Text(GitHubRuns.EMPTY, style = theme.body(color = theme.muted)) }
            }
            "compilation" -> {
                val c = model.home?.compilation
                if (c != null) CompilationSection(model, c)
                else Section(SectionPages.title(name)) { Text("Nothing is compiling.", style = theme.body(color = theme.muted)) }
            }
            else -> StageRows(model, name)
        }
    }
}

/** The wide task rows of a stage page. Testing adds suite progress and Skip testing. */
@Composable
private fun StageRows(model: MonitorModel, name: String) {
    val theme = LocalTheme.current
    val rows = model.sectionRows(name)
    var askAll by androidx.compose.runtime.remember { androidx.compose.runtime.mutableStateOf(false) }
    Section(SectionPages.title(name), name) {
        if (name == "testing" && rows.isNotEmpty()) {
            Pill("Skip testing for all", modifier = Modifier.testTag("testing:skip-all")) { model.skipAllTests(rows) }
            (model.skipDisabledReason ?: model.skipNotice ?: model.lastResult)?.let {
                Text(it, style = theme.body(color = theme.attention), modifier = Modifier.testTag("testing:skip-notice"))
            }
        }
        if (name == "landing" && rows.isNotEmpty()) {
            Pill("Land now for all", modifier = Modifier.testTag("landing:land-all")) { askAll = true }
            (model.landDisabledReason ?: model.landNotice ?: model.lastResult)?.let {
                Text(it, style = theme.body(color = theme.attention), modifier = Modifier.testTag("landing:land-notice"))
            }
            if (askAll) {
                androidx.compose.material3.AlertDialog(
                    onDismissRequest = { askAll = false },
                    text = { Text("Land ${model.landableRows(rows).size} tasks now? Their remaining tests run after the merge; a red result raises an alert.") },
                    confirmButton = { androidx.compose.material3.Button(onClick = { askAll = false; model.landAllNow(rows) }) { Text("Land now") } },
                    dismissButton = { androidx.compose.material3.Button(onClick = { askAll = false }) { Text("Cancel") } },
                )
            }
        }
        model.sectionCountText(name)?.let { Text(it, style = theme.body(color = theme.muted), modifier = Modifier.testTag("section-count:$name")) }
        if (rows.isEmpty()) Text("Nothing here.", style = theme.body(color = theme.muted))
        rows.forEach { r ->
            Row(horizontalArrangement = Arrangement.spacedBy(theme.spacing), verticalAlignment = Alignment.CenterVertically,
                modifier = Modifier.fillMaxWidth().testTag("stage-row:$name:${r.id}")) {
                ProgressCells(model, r.progress, r.slow)
                Text("${r.project} ", style = theme.body())
                Text(r.key, style = theme.linked(theme.body()), modifier = Modifier.pointingHand().clickable { model.go(Page.Task(r.id)) })
                Text("  ${r.title}", style = theme.body(), modifier = Modifier.weight(1f).clickable { model.go(Page.Task(r.id)) })
                LinkedText(
                    model,
                    listOf(r.step, SectionPages.suiteProgress(r), r.why_waiting, Format.duration(r.inState(model.daemonNow())), r.display.ifEmpty { r.model })
                        .filter { it.isNotEmpty() }.joinToString(" · "),
                    theme.small(color = if (r.slow) theme.slow else theme.muted),
                )
                if (name == "testing" && model.canAct && "skip_test" in r.allowed_actions) {
                    Pill("Skip testing", modifier = Modifier.testTag("testing:skip:${r.id}")) { model.skipTest(r.id, r.step) }
                }
                if (name == "landing") LandNowButton(model, LandNowTarget.Task(r.id), model.canAct && "land_now" in r.allowed_actions)
                if (name == "proposed" && model.canAct && "approve" in r.allowed_actions) {
                    PendingPill(model, r.id, "approve", "Approve", prominent = true, modifier = Modifier.testTag("row:${r.id}:approve")) { model.approve(r.id, MonitorModel.DEFAULT_APPROVAL_PRIORITY) }
                }
            }
        }
    }
}

/** The cause of a wait as a small badge on a task row (s527). */
@Composable
internal fun WaitBadge(cause: String) {
    val theme = LocalTheme.current
    Text(
        QueueWait.badge(cause),
        style = theme.small(color = theme.attention),
        modifier = Modifier.background(theme.attention.copy(alpha = 0.15f), RoundedCornerShape(50)).padding(horizontal = 6.dp)
            .testTag("wait:badge"),
    )
}

/** The lines of the queue-held banner: the summary, then the lease holding the queue (s527). */
internal fun queueHoldLines(h: mason.v1.QueueHold, now: Long): List<String> =
    listOfNotNull(h.summary, h.party?.takeIf { it.lease }?.let { QueueWait.lease(it, now) })

/** One line at the top of home when most waiting work is held by one request, gate or red test. */
@Composable
internal fun QueueHoldBanner(model: MonitorModel, h: mason.v1.QueueHold) {
    val theme = LocalTheme.current
    Column(
        verticalArrangement = Arrangement.spacedBy(theme.spacingSmall),
        modifier = Modifier.fillMaxWidth().background(theme.attention.copy(alpha = 0.12f), RoundedCornerShape(6.dp))
            .padding(theme.spacing).testTag("home:queue-hold"),
    ) {
        val lines = queueHoldLines(h, model.daemonNow())
        LinkedText(model, lines.first(), theme.body(FontWeight.SemiBold, color = theme.attention))
        lines.drop(1).forEach { LinkedText(model, it, theme.small(color = theme.muted), Modifier.testTag("home:queue-hold:lease")) }
        if (h.kind == "red") h.tasks.take(8).forEach { t -> Link("#$t") { model.go(Page.Task(t)) } }
    }
}
