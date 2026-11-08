// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import mason.v1.*
import kotlin.math.max
import kotlin.math.min
import kotlin.math.roundToInt

/**
 * What the monitor knows and what the user has done to it. The client draws this
 * and sends commands; it never re-derives pipeline rules: counts, sorting, allowed
 * actions and waiting text all arrive from the daemon.
 */
class MonitorModel(private val prefs: Preferences = MemoryPreferences()) {
    // From the daemon.
    var header by mutableStateOf<HeaderView?>(null)
    var sidebar by mutableStateOf<SidebarView?>(null)
    var home by mutableStateOf<HomeView?>(null)
    var taskDetail by mutableStateOf<TaskDetailView?>(null)
    var landing by mutableStateOf<LandingView?>(null)
    var usage by mutableStateOf<UsageView?>(null)
    var editor by mutableStateOf<EditorView?>(null)
    var health by mutableStateOf<HealthSnapshot?>(null)
    var introspection by mutableStateOf<IntrospectionSnapshot?>(null)
    var lastSeq by mutableStateOf(0L)
    var connected by mutableStateOf(false)
    /** Down and being retried: the daemon is restarting or has not answered. Shown as "reconnecting", never as an error. */
    var reconnecting by mutableStateOf(false)

    /**
     * The daemon's clock minus this device's, ms, as of the last message that carried the daemon's
     * time (the welcome, each StepProgress): "updated 4 s ago" counts on the local clock, corrected by this.
     */
    var clockOffsetMs by mutableStateOf(0L)
        private set

    /** The open task's live log: its last 40 lines, followed from the stream. */
    var liveLog by mutableStateOf<List<String>>(emptyList())
        private set

    /**
     * Whether the daemon took this connection's identity for the owner's (its welcome says).
     * Only then are the answer, approve and reject buttons drawn: anyone else sees the card.
     */
    var isOwner by mutableStateOf(false)
    val canAct: Boolean get() = isOwner && readOnlyReason == null

    // About versions (Part 15).
    /** This monitor's own build: a commit, or "dev" for a build nobody versioned. */
    var ownVersion: String = System.getenv("MASON_MONITOR_VERSION") ?: "dev"

    /** The daemon's build, from its welcome. */
    var daemonVersion by mutableStateOf("")

    /** Set when the daemon needs a newer protocol than this monitor speaks: nothing can be sent. */
    var readOnlyReason by mutableStateOf<String?>(null)

    /** What the update is doing (or why it cannot), once the user has asked for one. */
    var updating by mutableStateOf<String?>(null)

    /** Run when the user presses the update button (the app does the work). */
    var onUpdate: (() -> Unit)? = null

    /** Is there a newer monitor? Only when both builds are real versions and differ. */
    val updateAvailable: Boolean
        get() = daemonVersion.isNotEmpty() && daemonVersion != "dev" && ownVersion != "dev" && daemonVersion != ownVersion

    // The user's.
    var history by mutableStateOf(NavHistory())
        private set
    var search by mutableStateOf(SearchState())
    var textScale by mutableStateOf(prefs.double(KEY_SCALE) ?: 1.0)
        private set
    var muted by mutableStateOf(prefs.bool(KEY_MUTE) ?: false)
        private set
    var project: String
        get() = projectState
        set(v) { projectState = v; attentionSeen = null }
    private var projectState by mutableStateOf("")
    /** Called when something new needs the owner and the monitor is not muted: a task asked a
     *  question, waits for approval, was proposed, or failed. The app plays its sound here. */
    var onAttention: (() -> Unit)? = null
    /** The tasks that needed the owner at the last update; null until the first one, so what
     *  was already waiting when the monitor opened (or the project changed) is silent. */
    private var attentionSeen: Set<Long>? = null
    private var overrides by mutableStateOf(mapOf<String, Boolean>())

    /** Where the sidebar should scroll to (a tile was clicked). */
    var scrollTarget by mutableStateOf<String?>(null)
    var lastResult by mutableStateOf<String?>(null)
    /** What `mason proposal publish --preview` would send, once asked for. */
    var publishPreview by mutableStateOf<PublishPreview?>(null)

    /** The step selected in the pipeline editor (for its inspector). */
    var editorStep by mutableStateOf("")
        private set

    var sender: ((Command) -> Unit)? = null
    var subscribe: (() -> Unit)? = null

    /** Called with every command result (the tools read them). */
    var onResult: ((CommandResult) -> Unit)? = null

    /**
     * Who is acting. Only the owner's identity counts as an answer (Part 10), so
     * every command says who sent it: `MASON_AS`, else the operating-system user.
     */
    var identity: String = System.getenv("MASON_AS") ?: System.getProperty("user.name") ?: ""

    // ---- navigation ----

    val page: Page get() = history.current
    val canGoBack get() = history.canGoBack
    val canGoForward get() = history.canGoForward

    fun go(p: Page) { history = history.go(p); subscribe?.invoke() }
    fun goBack() { history = history.goBack(); subscribe?.invoke() }
    fun goForward() { history = history.goForward(); subscribe?.invoke() }
    fun goHome() { history = history.goHome(); subscribe?.invoke() }
    fun open(url: String) { Page.from(url)?.let(::go) }

    // ---- text scale ----

    private fun setScale(v: Double) {
        textScale = v
        prefs.setDouble(KEY_SCALE, v)
    }

    private fun round1(v: Double) = (v * 10).roundToInt() / 10.0
    fun zoomIn() = setScale(min(SCALE_MAX, round1(textScale + SCALE_STEP)))
    fun zoomOut() = setScale(max(SCALE_MIN, round1(textScale - SCALE_STEP)))
    fun zoomReset() = setScale(1.0)
    fun toggleMute() {
        muted = !muted
        prefs.setBool(KEY_MUTE, muted)
    }

    // ---- counts and tiles ----

    /** The tile text: the daemon's label, or (older daemon) the state with spaces. */
    fun tileLabel(c: StateCount): String = c.label.ifEmpty { c.state.replace('_', ' ') }

    /** A dash while disconnected: never a stale count, never a false zero. */
    fun tileCount(c: StateCount): String = if (connected) c.count.toString() else "–"

    /** The attention indicator is shown only while something needs a look. */
    val attention: Int get() = if (connected) header?.attention ?: 0 else 0

    // ---- sidebar sections ----

    fun isExpanded(s: SidebarSection): Boolean =
        prefs.bool(sectionKey(s.state)) ?: overrides[s.state] ?: !s.collapsed

    fun setExpanded(state: String, open: Boolean) {
        overrides = overrides + (state to open)
        prefs.setBool(sectionKey(state), open)
    }

    fun toggle(s: SidebarSection) = setExpanded(s.state, !isExpanded(s))

    /** Clicking a tile expands its section and scrolls to it. */
    fun reveal(state: String) {
        setExpanded(state, true)
        scrollTarget = state
    }

    /** What a tile click does: reveal on every click; the second click of a double also opens the lone task. */
    fun tileClicked(state: String, clickCount: Int) {
        reveal(state)
        if (clickCount == 2) revealAndOpenIfOne(state)
    }

    /** Reveal the section and, if it holds exactly one task right now (search ignored), open it. Nothing while disconnected. */
    fun revealAndOpenIfOne(state: String) {
        if (!connected) return
        reveal(state)
        val sec = sidebar?.sections?.firstOrNull { it.state == state } ?: return
        if (sec.rows.size == 1) go(Page.Task(sec.rows[0].id))
    }

    /** The tile's tooltip; a count of exactly one says a double-click opens it. */
    fun tileTooltip(c: StateCount): String =
        if (connected && c.count == 1) c.tooltip + "\nDouble-click to open it" else c.tooltip

    // ---- search ----

    fun filteredRows(s: SidebarSection): List<TaskRow> = s.rows.filter { search.matches(it) }
    fun visibleCount(s: SidebarSection): Int = if (search.isActive) filteredRows(s).size else s.count

    // ---- commands ----

    fun send(c: Command) {
        readOnlyReason?.let {
            lastResult = "Read-only: $it"
            return
        }
        sender?.invoke(if (c.actor.isEmpty()) c.copy(actor = identity) else c)
    }

    fun cancel(task: Long) = send(Command(cancel = TaskRef(task = task)))
    fun retry(task: Long, from: String = "") = send(Command(retry = Retry(task = task, from = from)))
    fun approve(task: Long) = send(Command(approve = TaskRef(task = task)))
    /** Approve a held self-change: the exact diff the owner was shown. */
    fun approveHold(task: Long, diffHash: String) =
        send(Command(approve_hold = ApproveHold(task = task, diff_hash = diffHash)))

    /** What Approve sends for an approval: the held diff's hash when there is one, else a plain approve. */
    fun approve(task: Long, a: Approval) = if (a.diff_hash.isEmpty()) approve(task) else approveHold(task, a.diff_hash)

    /** The first task that needs the owner, in the Home queue's order. */
    val firstNeedingYou: Long? get() = if (connected) home?.attention?.firstOrNull()?.task?.id else null

    /** What clicking the attention count does: open the first task that needs the owner. */
    fun openFirstNeedingYou() {
        val t = firstNeedingYou
        if (t != null) go(Page.Task(t)) else goHome()
    }

    fun reject(task: Long, reason: String = "") = send(Command(reject = Reject(task = task, reason = reason)))
    fun exploreAnyway(task: Long) = send(Command(explore_anyway = TaskRef(task = task)))
    fun landNow(task: Long) = send(Command(land_now = TaskRef(task = task)))
    fun force(task: Long) = send(Command(force = TaskRef(task = task)))
    fun skipTest(task: Long, step: String) = send(Command(skip = Skip(task = task, step = step)))
    fun answer(task: Long, text: String, cardHash: String) =
        send(Command(answer = Answer(task = task, answer = text, card_hash = cardHash)))
    fun addTask(project: String, title: String, body: String) =
        send(Command(add_task = AddTask(project = project, title = title, body = body)))
    fun setSessionLimit(units: Int) {
        val name = usage?.providers?.firstOrNull()?.name ?: "claude"
        send(Command(set_limit = SetLimit(resource = "llm:$name", units = units)))
    }
    fun pause(mode: String, seconds: Long) =
        send(Command(pause = PauseWork(project = project, mode = mode, for_seconds = seconds)))
    fun resume() = send(Command(resume = ResumeWork(project = project)))
    /** MASON's own repository's project, when one is registered: "Improve the pipeline" files there. */
    val selfProject: String get() = header?.self_project ?: ""

    /** Change a proposal's title, body or evidence while it waits for the owner (null: unchanged). */
    fun editProposal(task: Long, title: String?, body: String?, evidence: String?) {
        val o = linkedMapOf<String, Any?>()
        title?.let { o["title"] = it }
        body?.let { o["body"] = it }
        evidence?.let { o["evidence"] = it }
        raw("POST", "/proposals/$task/edit", json = EditorJson.text(o))
    }

    /** "Improve the pipeline": ask MASON to change its own code. */
    fun improve(project: String, text: String) =
        raw("POST", "/projects/$project/improve", json = EditorJson.text(mapOf("text" to text)))

    /** Show the redacted version that would go upstream (`confirm = false`), or send it. */
    fun publish(task: Long, confirm: Boolean) =
        raw("POST", "/proposals/$task/publish", id = "publish:$task", json = EditorJson.text(mapOf("confirm" to confirm)))

    fun raw(method: String, path: String, id: String = "", json: String = "") =
        send(Command(id = id, raw = Raw(method = method, path = path, json = json)))

    // ---- the pipeline editor (Part 18) ----

    /** The project the editor is about: the one picked, else the first there is. */
    val editorProject: String get() = project.ifEmpty { header?.projects?.firstOrNull() ?: "" }

    fun selectStep(name: String) {
        editorStep = name
        subscribe?.invoke()
    }

    /** One operation on the draft, as the daemon's editor API takes it. */
    fun editorOp(op: Map<String, Any?>) = raw("POST", "/projects/$editorProject/editor/op", json = EditorJson.text(op))

    /** `undo`, `redo`, `discard`, `simulate` or `apply`. */
    fun editorCall(what: String, body: Map<String, Any?> = emptyMap()) =
        raw("POST", "/projects/$editorProject/editor/$what", json = EditorJson.text(body))

    // ---- live progress ----

    /** The daemon's time now, ms since the epoch, by this device's clock and the offset. */
    fun daemonNow(local: Long = System.currentTimeMillis()): Long = max(0L, local + clockOffsetMs)

    internal fun anchor(daemonNow: Long, local: Long = System.currentTimeMillis()) {
        if (daemonNow > 0) clockOffsetMs = daemonNow - local
    }

    /** A step's progress arrived without a view: put it into the task page and the rows, and follow the log. */
    fun apply(p: StepProgress, local: Long = System.currentTimeMillis()) {
        anchor(p.now_ms, local)
        val cell = p.progress ?: return
        fun merged(old: List<ProgressCell>): List<ProgressCell>? {
            val i = old.indexOfFirst { it.step == cell.step }
            if (i < 0) return null
            // A finished or stopped run says what it ended as on the next view; until then, no "running".
            val c = if (cell.state != CellState.PART) cell.copy(label = "") else cell
            return old.toMutableList().also { it[i] = c }
        }
        taskDetail?.let { d ->
            val r = d.row
            if (r != null && r.id == p.task) {
                merged(r.progress)?.let { taskDetail = d.copy(row = r.copy(progress = it)) }
                appendLog(p.log)
            }
        }
        sidebar?.let { s ->
            var changed = false
            val sections = s.sections.map { sec ->
                sec.copy(rows = sec.rows.map { row ->
                    val cells = if (row.id == p.task) merged(row.progress) else null
                    if (cells != null) { changed = true; row.copy(progress = cells) } else row
                })
            }
            if (changed) sidebar = s.copy(sections = sections)
        }
    }

    internal fun appendLog(lines: List<String>) {
        if (lines.isEmpty()) return
        liveLog = (liveLog + lines).takeLast(LOG_KEEP)
    }

    // ---- updates from the daemon ----

    fun apply(m: ServerMessage) {
        m.welcome?.let { w ->
            connected = true
            reconnecting = false
            if (w.resume == ResumeKind.SNAPSHOT) lastSeq = 0
            daemonVersion = w.version
            anchor(w.now_ms)
            isOwner = w.owner
            readOnlyReason = if (w.min_protocol > PROTOCOL)
                "This monitor speaks protocol $PROTOCOL, but the daemon needs ${w.min_protocol} or newer. Update the monitor; until then it can only look."
            else null
        }
        m.snapshot?.let { s ->
            lastSeq = max(lastSeq, s.seq)
            s.views.forEach(::apply)
        }
        m.events?.let { e -> e.events.lastOrNull()?.let { lastSeq = max(lastSeq, it.seq) } }
        m.view?.let { u ->
            lastSeq = max(lastSeq, u.seq)
            u.view?.let(::apply)
        }
        m.step_progress?.let { apply(it) }
        m.result?.let { r ->
            lastResult = if (r.ok) null else r.error
            if (r.id == "health" && r.ok) health = HealthSnapshot.parse(r.json)
            if (r.id == "introspection" && r.ok) introspection = IntrospectionSnapshot.parse(r.json)
            if (r.id.startsWith("publish:") && r.ok) publishPreview = PublishPreview.parse(r.json)
            onResult?.invoke(r)
        }
    }

    /** Chime once when a task that needs the owner appears that was not there before. */
    internal fun noteAttention(s: SidebarView) {
        val ids = s.sections.filter { it.state in WANTING }.flatMap { it.rows.map { r -> r.id } }.toSet()
        val seen = attentionSeen
        attentionSeen = ids
        if (seen != null && !muted && (ids - seen).isNotEmpty()) onAttention?.invoke()
    }

    fun apply(v: View) {
        v.header_?.let { header = it }
        v.sidebar?.let { sidebar = it; noteAttention(it) }
        v.home?.let { home = it }
        v.task_detail?.let { d ->
            // A different task's page starts its log over; the same task's view carries the tail again.
            taskDetail = d
            liveLog = d.log_tail.split("\n").dropLastWhile { it.isEmpty() }.takeLast(LOG_KEEP)
        }
        v.landing?.let { landing = it }
        v.usage?.let { usage = it }
        v.editor?.let { editor = it }
    }

    fun disconnected() { connected = false; reconnecting = true }

    companion object {
        /** The protocol this monitor speaks. */
        const val PROTOCOL = 1
        const val LOG_KEEP = 40
        const val KEY_SCALE = "textScale"
        const val KEY_MUTE = "muted"
        private val WANTING = setOf("needs_attention", "failed", "proposed")
        const val SCALE_MIN = 0.8
        const val SCALE_MAX = 2.4
        const val SCALE_STEP = 0.1
        fun sectionKey(state: String) = "section.$state.expanded"
    }
}

/** The search box and its three checkboxes: name, title, body. */
data class SearchState(
    val text: String = "",
    val inName: Boolean = true,
    val inTitle: Boolean = true,
    val inBody: Boolean = false,
) {
    val isActive get() = text.isNotBlank()

    fun matches(r: TaskRow): Boolean {
        val q = text.trim().lowercase()
        if (q.isEmpty()) return true
        if (inName && (r.project.lowercase().contains(q) || r.key.lowercase().contains(q))) return true
        if (inTitle && r.title.lowercase().contains(q)) return true
        if (inBody && r.body.lowercase().contains(q)) return true
        return false
    }
}
