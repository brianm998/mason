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

    // The user's.
    var history by mutableStateOf(NavHistory())
        private set
    var search by mutableStateOf(SearchState())
    var textScale by mutableStateOf(prefs.double(KEY_SCALE) ?: 1.0)
        private set
    var muted by mutableStateOf(prefs.bool(KEY_MUTE) ?: false)
        private set
    var project by mutableStateOf("")
    private var overrides by mutableStateOf(mapOf<String, Boolean>())

    /** Where the sidebar should scroll to (a tile was clicked). */
    var scrollTarget by mutableStateOf<String?>(null)
    var lastResult by mutableStateOf<String?>(null)

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
        sender?.invoke(if (c.actor.isEmpty()) c.copy(actor = identity) else c)
    }

    fun cancel(task: Long) = send(Command(cancel = TaskRef(task = task)))
    fun retry(task: Long, from: String = "") = send(Command(retry = Retry(task = task, from = from)))
    fun approve(task: Long) = send(Command(approve = TaskRef(task = task)))
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
    fun raw(method: String, path: String, id: String = "") =
        send(Command(id = id, raw = Raw(method = method, path = path)))

    // ---- updates from the daemon ----

    fun apply(m: ServerMessage) {
        m.welcome?.let { w ->
            connected = true
            if (w.resume == ResumeKind.SNAPSHOT) lastSeq = 0
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

    fun apply(v: View) {
        v.header_?.let { header = it }
        v.sidebar?.let { sidebar = it }
        v.home?.let { home = it }
        v.task_detail?.let { taskDetail = it }
        v.landing?.let { landing = it }
        v.usage?.let { usage = it }
        v.editor?.let { editor = it }
    }

    fun disconnected() { connected = false }

    companion object {
        const val KEY_SCALE = "textScale"
        const val KEY_MUTE = "muted"
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
