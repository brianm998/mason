// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import mason.MasonTokens
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
    /** The CPU shares, apart from `home` so the CPU section reads only this. Null from an older daemon. */
    var cpu by mutableStateOf<CpuUsage?>(null)
    /** Each task's colour in the CPU bar while it is among the shares. */
    var cpuColors by mutableStateOf<Map<Long, Int>>(emptyMap())
    private val cpuPalette = CpuColors()
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
    var ownVersion: String = System.getenv("MASON_MONITOR_VERSION") ?: System.getProperty("mason.monitor.version") ?: "dev"

    /** The daemon's build, from its welcome. */
    var daemonVersion by mutableStateOf("")

    /** Set when the daemon needs a newer protocol than this monitor speaks: nothing can be sent. */
    var readOnlyReason by mutableStateOf<String?>(null)

    /** What this monitor's update is doing, while the user has asked for one. */
    var updating by mutableStateOf<String?>(null)

    /** Why the last update of this monitor did not happen. */
    var updateError by mutableStateOf<String?>(null)

    /**
     * Starts the finished build (a path on this machine) and quits. Only the desktop app
     * sets this; a phone has no way to replace itself.
     */
    var onInstall: ((String) -> Unit)? = null

    /** The daemon said this monitor's build is on its way. */
    private var monitorAsked = false

    /** The connection is to a daemon on this machine, so a path it gives is one we can open. */
    var localDaemon by mutableStateOf(true)
        private set

    private val updateInfo: UpdateInfo? get() = header?.update
    private val monitorBuild: MonitorBuild? get() = updateInfo?.monitors?.firstOrNull { it.kind == MONITOR_KIND }

    /**
     * Has this monitor's own source changed on the update channel? Never for a build
     * nobody stamped, so development builds do not nag.
     */
    val updateAvailable: Boolean
        get() {
            if (!canAct || !localDaemon || onInstall == null || ownVersion == "dev") return false
            val latest = monitorBuild?.latest ?: return false
            return latest.isNotEmpty() && latest != ownVersion
        }

    /** The commit a monitor update would go to. */
    val latestMonitor: String get() = monitorBuild?.latest ?: ""

    /** What to say about the daemon's own version; empty when there is nothing to say. */
    val daemonUpdateText: String get() = updateInfo?.daemon_text ?: ""
    val canUpdateDaemon: Boolean get() = canAct && updateInfo?.daemon_can_update == true

    /** On the `release` channel (a packaged install) there is no source to build from: the button opens the download page. */
    val downloadUrl: String get() = updateInfo?.download_url ?: ""
    var onOpenUrl: ((String) -> Unit)? = null
    fun openDownloadPage() {
        val u = downloadUrl
        if (u.startsWith("https://") || u.startsWith("http://")) onOpenUrl?.invoke(u)
    }

    /** The owner's button: the daemon builds its newest version, proves it and takes over. */
    fun updateDaemon() = raw("POST", "/update", json = "{}")

    /** The owner's button: the daemon builds the newest monitor; [noteMonitorBuild] takes it from there. */
    fun updateMonitor() {
        updateError = null
        updating = "Building the new monitor…"
        raw("POST", "/update/monitor", id = "update-monitor", json = "{\"kind\":\"$MONITOR_KIND\"}")
    }

    /** The update of this monitor stopped; this one keeps running. */
    fun monitorUpdateFailed(why: String) {
        monitorAsked = false
        updating = null
        updateError = why
    }

    /** Follow the build the owner asked for, from each new header. */
    private fun noteMonitorBuild() {
        if (!monitorAsked) return
        val b = monitorBuild
        when (b?.phase) {
            "building" -> {}
            "ready" -> {
                monitorAsked = false
                updating = "Starting the new monitor…"
                onInstall?.invoke(b.path)
            }
            "failed" -> monitorUpdateFailed("The new monitor did not build: ${b.error}")
            else -> monitorUpdateFailed("The daemon restarted during the build. Press Update to start it again.")
        }
    }

    // The user's.
    var history by mutableStateOf(NavHistory())
        private set
    var search by mutableStateOf(SearchState())
    var textScale by mutableStateOf(prefs.double(KEY_SCALE) ?: 1.0)
        private set
    /** The panel's width before text-zoom scaling; always inside the token limits. */
    var sidebarBase by mutableStateOf(clampSidebar(prefs.double(KEY_SIDEBAR) ?: MasonTokens.sizeSidebarWidth.toDouble()))
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
    /** Open session found the step running: the task to ask about. Answer with [answerSessionPrompt]. */
    var sessionPrompt by mutableStateOf<Long?>(null)
        private set
    /** What Open session did or could not do, in words, until the next click. */
    var sessionNotice by mutableStateOf<String?>(null)
        private set
    /** Starts a terminal running this argument vector; false when it could not. The app sets it. */
    var launcher: ((List<String>) -> Boolean)? = null
    /** Puts text on the clipboard. The app sets it. */
    var copier: ((String) -> Unit)? = null
    /** The CLI by its full path. */
    var cliPath: String = OpenSession.defaultCli()
    /** What `mason proposal publish --preview` would send, once asked for. */
    var publishPreview by mutableStateOf<PublishPreview?>(null)

    /** The SSH key sheet after a refused push: its project (null: closed), the keys, what choosing said. */
    var sshKeyProject by mutableStateOf<String?>(null)
    var sshKeys by mutableStateOf<SshKeyList?>(null)
    var sshKeyResult by mutableStateOf<String?>(null)
    var sshKeyBusy by mutableStateOf(false)

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
    /** Whether [n] is a task this monitor has heard of (in the sidebar, or named by the open task's page). */
    fun isKnownTask(n: Long): Boolean {
        if (sidebar?.sections?.any { sec -> sec.rows.any { it.id == n } } == true) return true
        val d = taskDetail ?: return false
        val r = d.row ?: return false
        return r.id == n || n in r.blockers || n in r.blocks || d.debts.any { it.task == n } || d.proposal?.filed_by == n
    }

    /** [text] as plain text, task links and URL links, for the open task's page. */
    fun linkify(text: String): List<LinkSegment> =
        Linkify.segments(text, { isKnownTask(it) }, taskDetail?.ci_links ?: emptyList())

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
        // Muting silences sound and speech at once. Banners and the attention count are drawn
        // from the views, not from here, so they stay.
        if (muted) announcer.silence()
    }

    /** The queue of sounds and sentences (Part 19). The app gives it a sink that plays them. */
    val announcer = Announcer(isMuted = { muted }, prefs = prefs)
    /** This monitor's name when it asks to speak; one per launch. */
    val clientId: String = java.util.UUID.randomUUID().toString()
    private var lastClaim = 0L

    /**
     * Called about once a second by the app: the lost-connection clock, and every few seconds a
     * claim to speak, whose answer carries what the daemon wants said.
     */
    fun pulse(nowMs: Long = System.currentTimeMillis()) {
        announcer.check(nowMs)
        if (connected && nowMs - lastClaim >= CLAIM_EVERY_MS) {
            lastClaim = nowMs
            raw("POST", "/announce/claim", id = "announce", json = announcer.claimBody(clientId))
        }
    }

    // ---- counts and tiles ----

    /** The tile text: the daemon's label, or (older daemon) the state with spaces. */
    fun tileLabel(c: StateCount): String = c.label.ifEmpty { c.state.replace('_', ' ') }

    /** A dash while disconnected: never a stale count, never a false zero. */
    fun tileCount(c: StateCount): String = if (connected) c.count.toString() else "–"

    /** The attention indicator is shown only while something needs a look. */
    val attention: Int get() = if (connected) header?.attention ?: 0 else 0

    // ---- sidebar width ----

    /** Sets and stores the unscaled width, clamped to the limits. */
    fun setSidebarWidth(w: Double) {
        val c = clampSidebar(w)
        if (c == sidebarBase) return
        sidebarBase = c
        prefs.setDouble(KEY_SIDEBAR, c)
    }

    /** The stored width as drawn at this text zoom. */
    fun panelWidth(scale: Double): Double = sidebarBase * sidebarFactor(scale)

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

    /** The daemon sends only the newest rows of a long history section; `count` stays the true total. */
    fun isCapped(s: SidebarSection): Boolean = s.count > s.rows.size

    /** The number in a section header: "500 of 2000" when capped; while searching, "3 in newest 500". */
    fun countText(s: SidebarSection): String = when {
        !isCapped(s) -> visibleCount(s).toString()
        search.isActive -> "${filteredRows(s).size} in newest ${s.rows.size}"
        else -> "${s.rows.size} of ${s.count}"
    }

    // ---- commands ----

    fun send(c: Command) {
        readOnlyReason?.let {
            lastResult = "Read-only: $it"
            return
        }
        sender?.invoke(if (c.actor.isEmpty()) c.copy(actor = identity) else c)
    }

    fun cancel(task: Long) = send(Command(cancel = TaskRef(task = task)))
    fun retry(task: Long, from: String = "", seen: Long = 0) = send(Command(retry = Retry(task = task, from = from, seen = seen)))
    fun approve(task: Long) = send(Command(approve = TaskRef(task = task)))
    /**
     * The tasks the Add blocker picker offers for [row]: any task in the sidebar that is not itself,
     * not already a blocker, and could still land (not landed, canceled or failed).
     */
    fun blockerCandidates(row: TaskRow): List<TaskRow> =
        (sidebar?.sections ?: emptyList()).flatMap { it.rows }
            .filter { it.id != row.id && it.id !in row.blockers && it.state !in setOf("landed", "canceled", "failed") }
            .distinctBy { it.id }
            .sortedBy { it.id }

    /** Release a task from one blocker (owner only; the daemon refuses anyone else). */
    fun release(task: Long, blocker: Long) = send(Command(unblock = Unblock(task = task, from = listOf(blocker))))
    /** Make a task wait for another one as well (owner only). */
    fun addBlocker(task: Long, after: Long) = send(Command(block = Block(task = task, after = listOf(after))))
    /** Put a canceled task whose branch was kept back in the queue (owner only). */
    fun revive(task: Long) = send(Command(revive = TaskRef(task = task)))
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
    /** How many compiles may run at once (the daemon refuses 0). */
    fun setCompileLimit(units: Int) =
        send(Command(set_limit = SetLimit(resource = "compile", units = maxOf(1, units))))
    /** Start another compile only below [pct] percent CPU (kept to 10-100; the daemon holds 1-100 and says if it refuses). */
    fun setCompileCpuGate(pct: Int) =
        send(Command(set_gate = SetGate(name = "compile_cpu_max", value_ = pct.coerceIn(CPU_GATE_MIN, CPU_GATE_MAX).toLong())))
    /** Seconds after one compile starts before the next may (0: no wait). */
    fun setCompileSettle(seconds: Int) =
        send(Command(set_gate = SetGate(name = "compile_settle_s", value_ = seconds.coerceIn(0, SETTLE_MAX).toLong())))
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

    /** Open session: ask the daemon about the task's session first; the result carries on. */
    fun openSession(task: Long) {
        sessionNotice = null
        raw("GET", "/tasks/$task/session", id = "session:$task")
    }

    fun answerSessionPrompt(yes: Boolean) {
        val task = sessionPrompt ?: return
        sessionPrompt = null
        if (yes) launchSession(task, anyway = true)
    }

    private fun launchSession(task: Long, anyway: Boolean) {
        if (launcher?.invoke(OpenSession.argv(cliPath, task, anyway)) == true) return
        val line = OpenSession.commandLine(cliPath, task, anyway)
        copier?.invoke(line)
        sessionNotice = OpenSession.copiedNotice(line)
    }

    /** Open the key sheet for [project] and ask the daemon which keys it could use. */
    fun chooseSshKey(project: String) {
        sshKeyProject = project
        sshKeys = null
        sshKeyResult = null
        raw("GET", "/projects/$project/ssh-keys", id = "ssh-keys:$project")
    }

    /** Use this key for the project's remote. The passphrase, if any, is asked for in a dialog on the daemon's Mac, never here. */
    fun useSshKey(project: String, path: String) {
        sshKeyBusy = true
        sshKeyResult = null
        raw("POST", "/projects/$project/ssh-key", id = "ssh-key:$project", json = EditorJson.text(mapOf("path" to path)))
    }

    fun closeSshKeys() {
        sshKeyProject = null
        sshKeys = null
        sshKeyResult = null
        sshKeyBusy = false
    }

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
            announcer.connectionRestored()
            if (w.resume == ResumeKind.SNAPSHOT) lastSeq = 0
            daemonVersion = w.version
            localDaemon = w.role.isEmpty()
            // A build asked for on the connection that just went has no answer coming.
            if (!monitorAsked) updating = null
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
            if (r.id.startsWith("session:")) r.id.removePrefix("session:").toLongOrNull()?.let { task ->
                when (val d = OpenSession.decide(r.ok, r.json, r.error)) {
                    OpenSession.Decision.Confirm -> sessionPrompt = task
                    is OpenSession.Decision.Launch -> launchSession(task, d.anyway)
                    is OpenSession.Decision.Failed -> sessionNotice = d.why
                }
            }
            if (r.id.startsWith("ssh-keys:")) { if (r.ok) sshKeys = SshKeyList.parse(r.json) else sshKeyResult = r.error }
            if (r.id.startsWith("ssh-key:")) {
                sshKeyBusy = false
                if (r.ok) { sshKeyResult = "The remote accepted the key. Landing is released."; sshKeyProject = null } else sshKeyResult = r.error
            }
            if (r.id == "announce" && r.ok) announcer.handleClaim(r.json)
            if (r.id == "update-monitor") {
                if (r.ok) monitorAsked = true else monitorUpdateFailed(r.error)
            }
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
        if (seen != null && !muted && !announcer.heardFromDaemon && (ids - seen).isNotEmpty()) onAttention?.invoke()
    }

    fun apply(v: View) {
        v.header_?.let {
            header = it
            noteMonitorBuild()
        }
        v.sidebar?.let { sidebar = it; noteAttention(it) }
        v.home?.let {
            home = it
            if (it.cpu != cpu) {
                cpu = it.cpu
                cpuColors = cpuPalette.assign(it.cpu?.shares.orEmpty().filter { s -> s.kind == "task" }.map { s -> s.task })
            }
        }
        v.task_detail?.let { d ->
            // A different task's page starts its log over; the same task's view carries the tail again.
            taskDetail = d
            liveLog = d.log_tail.split("\n").dropLastWhile { it.isEmpty() }.takeLast(LOG_KEEP)
        }
        v.landing?.let { landing = it }
        v.usage?.let { usage = it }
        v.editor?.let { editor = it }
    }

    fun disconnected(nowMs: Long = System.currentTimeMillis()) {
        connected = false
        reconnecting = true
        announcer.connectionLost(nowMs)
    }

    companion object {
        /** The CPU gate's range in the monitor, and the most the settle stepper asks for. */
        const val CPU_GATE_MIN = 10
        const val CPU_GATE_MAX = 100
        const val SETTLE_MAX = 600

        /** The project a `push_auth` banner is about: its text starts "<project>: ". */
        fun pushAuthProject(b: Banner): String = b.text.substringBefore(":")

        /** The protocol this monitor speaks. */
        const val PROTOCOL = 1
        const val MONITOR_KIND = "compose"
        const val LOG_KEEP = 40
        const val CLAIM_EVERY_MS = 3_000L
        const val KEY_SCALE = "textScale"
        const val KEY_MUTE = "muted"
        const val KEY_SIDEBAR = "sidebarWidth"
        /** Text zoom stops scaling the panel here, as before. */
        const val SIDEBAR_SCALE_CAP = 1.5
        fun clampSidebar(w: Double): Double =
            if (!w.isFinite()) MasonTokens.sizeSidebarWidth.toDouble()
            else min(MasonTokens.sizeSidebarWidthMax.toDouble(), max(MasonTokens.sizeSidebarWidthMin.toDouble(), w))
        fun sidebarFactor(scale: Double) = min(scale, SIDEBAR_SCALE_CAP)
        /** The width to draw: the panel gives way first, so the main view keeps its minimum unless the panel is at its (scaled) floor. */
        fun fit(desired: Double, available: Double, scale: Double): Double {
            val floor = MasonTokens.sizeSidebarWidthMin * sidebarFactor(scale)
            val room = available - MasonTokens.sizeMainWidthMin - MasonTokens.sizeSidebarHandle
            return max(floor, min(desired, room))
        }
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
