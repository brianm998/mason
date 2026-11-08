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
    var lastSeq by mutableStateOf(0L)
    var connected by mutableStateOf(false)

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

    // ---- updates from the daemon ----

    fun apply(m: ServerMessage) {
        m.welcome?.let { w ->
            connected = true
            if (w.resume == ResumeKind.SNAPSHOT) lastSeq = 0
            daemonVersion = w.version
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
        m.result?.let { r ->
            lastResult = if (r.ok) null else r.error
            if (r.id == "health" && r.ok) health = HealthSnapshot.parse(r.json)
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
        v.task_detail?.let { taskDetail = it }
        v.landing?.let { landing = it }
        v.usage?.let { usage = it }
        v.editor?.let { editor = it }
    }

    fun disconnected() { connected = false }

    companion object {
        /** The protocol this monitor speaks. */
        const val PROTOCOL = 1
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
