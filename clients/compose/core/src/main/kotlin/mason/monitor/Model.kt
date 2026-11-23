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
    private var headerState by mutableStateOf<HeaderView?>(null)
    var header: HeaderView?
        get() = headerState
        set(v) {
            headerState = v
            prunePauseCollapse()
        }
    var sidebar by mutableStateOf<SidebarView?>(null)
    var home by mutableStateOf<HomeView?>(null)
    /** The CPU shares, apart from `home` so the CPU section reads only this. Null from an older daemon. */
    var cpu by mutableStateOf<CpuUsage?>(null)
    /** Each task's colour in the CPU bar while it is among the shares. */
    var cpuColors by mutableStateOf<Map<Long, Int>>(emptyMap())
    private val cpuPalette = CpuColors()
    /** The latest CPU history for [cpuSpan]; the graph is built from it off the main thread. */
    var cpuHistory by mutableStateOf<mason.v1.CpuHistory?>(null)
    var cpuSpan by mutableStateOf(CpuSpan.Hour)
    /** The latest disk history for [diskSpan]; null until it arrives (and just after a new span is picked). */
    var diskHistory by mutableStateOf<mason.v1.DiskHistory?>(null)
    var diskSpan by mutableStateOf(DiskSpan.Hour)
    /** True once the daemon has sent any disk history; an older daemon never does, which hides the section. */
    var diskSeen by mutableStateOf(false)
    /** The volume ids whose line is off, kept across restarts. */
    var hiddenVolumes by mutableStateOf(decodeHiddenVolumes(prefs.string(KEY_HIDDEN_VOLUMES)))
        private set
    private val diskPalette = DiskColors()
    /** Each listed volume's colour slot, stable while it is listed. */
    var diskColors by mutableStateOf<Map<String, Int>>(emptyMap())
        private set
    var taskDetail by mutableStateOf<TaskDetailView?>(null)
    var landing by mutableStateOf<LandingView?>(null)
    var usage by mutableStateOf<UsageView?>(null)
    var editor by mutableStateOf<EditorView?>(null)
    var health by mutableStateOf<HealthSnapshot?>(null)
    var introspection by mutableStateOf<IntrospectionSnapshot?>(null)
    /** The Settings page (s430): the picked project's settings, the notification routes, and what the last change said. */
    var settings by mutableStateOf<SettingsSnapshot?>(null)
    var notifySettings by mutableStateOf<NotifySettings?>(null)
    var settingsNotice by mutableStateOf<String?>(null)
    var remoteSettings by mutableStateOf<RemoteSettings?>(null)
    /** The Devices section (s433): LAN state, paired devices, the code on show, and what the last action said. */
    var lanStatus by mutableStateOf<LanStatus?>(null)
    var devices by mutableStateOf<DeviceList?>(null)
    var pairOffer by mutableStateOf<PairOffer?>(null)
    var devicesNotice by mutableStateOf<String?>(null)
    var lastSeq by mutableStateOf(0L)
    /** Highest seq of a view update actually applied; events and stale views never move it. */
    var lastViewSeq = 0L
    /** The highest sequence number of a view applied so far; an older one is dropped. */
    private val appliedViewSeq = HashMap<String, Long>()

    /** The kinds of view applied at least once, for tools that wait for every subscribed view. */
    fun appliedKinds(): Set<String> = appliedViewSeq.keys.toSet()

    private fun viewKind(v: View): String = when {
        v.sidebar != null -> "sidebar"
        v.home != null -> "home"
        v.header_ != null -> "header"
        v.task_detail != null -> "task"
        else -> "other"
    }
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

    /** How many commits behind develop the daemon says it is (parsed from its sentence); null when it does not say. */
    val daemonCommitsBehind: Int?
        get() = Regex("""(\d+) commits? behind""").find(daemonUpdateText)?.groupValues?.get(1)?.toIntOrNull()

    /** Home's version line: masond's short commit (or "unversioned build"), how far behind develop, and this monitor's own commit. Empty before a welcome. */
    val versionLine: String
        get() {
            if (daemonVersion.isEmpty()) return ""
            var d = if (daemonVersion == "dev") "masond (unversioned build)" else "masond ${daemonVersion.take(8)}"
            daemonCommitsBehind?.let { d += " ($it commit${if (it == 1) "" else "s"} behind develop)" }
            val o = if (ownVersion == "dev") "monitor (unversioned build)" else "monitor ${ownVersion.take(8)}"
            return "$d · $o"
        }

    /** The full hashes, for the tooltip. */
    val versionTip: String get() = if (daemonVersion.isEmpty()) "" else "masond $daemonVersion\nmonitor $ownVersion"

    /** What to say about the daemon's own version; empty when there is nothing to say. */
    val daemonUpdateText: String get() = updateInfo?.daemon_text ?: ""
    /** The masond update is only progressing (fetching, building, testing, restarting): not a bar row. */
    val daemonUpdateIsInfo: Boolean get() = updateInfo?.daemon_level == "info"

    /** Everything the notification bar leaves out, one line each. An older daemon sends no level: a warning. */
    val infoNotices: List<String>
        get() = buildList {
            if (daemonUpdateText.isNotEmpty() && daemonUpdateIsInfo) add(daemonUpdateText)
            header?.banners?.filter { it.level == "info" }?.forEach { add(it.text) }
        }

    /** The top bar's status line: "masond updating · 2 CI jobs being fixed". Empty when nothing is quiet. */
    val infoSummary: String
        get() {
            val parts = mutableListOf<String>()
            if (daemonUpdateText.isNotEmpty() && daemonUpdateIsInfo) parts += "masond updating"
            var jobs = 0
            var other = 0
            header?.banners?.filter { it.level == "info" }?.forEach {
                val n = Regex("""(\d+) red CI jobs? being fixed""").find(it.text)?.groupValues?.get(1)?.toIntOrNull()
                if (n != null) jobs += n else other++
            }
            if (jobs > 0) parts += "$jobs CI job${if (jobs == 1) "" else "s"} being fixed"
            if (other > 0) parts += if (other == 1) "1 notice" else "$other notices"
            return parts.joinToString(" · ")
        }

    val canUpdateDaemon: Boolean get() = canAct && updateInfo?.daemon_can_update == true

    /** On the `release` channel (a packaged install) there is no source to build from: the button opens the download page. */
    val downloadUrl: String get() = updateInfo?.download_url ?: ""
    var onOpenUrl: ((String) -> Unit)? = null
    fun openDownloadPage() {
        val u = downloadUrl
        if (u.startsWith("https://") || u.startsWith("http://")) onOpenUrl?.invoke(u)
    }


    /** The button's label, as the daemon words it ("Update masond", or "Update now" while a finished update waits for landing batches). */
    val daemonUpdateButton: String get() = updateInfo?.daemon_button?.takeIf { it.isNotEmpty() } ?: "Update masond"

    /** What to ask before the button acts; empty when it acts at once. */
    val daemonUpdateConfirm: String get() = updateInfo?.daemon_confirm ?: ""

    /** The confirmation dialog is open. */
    var confirmingDaemonUpdate by mutableStateOf(false)

    /** The Land now the owner pressed and has not yet confirmed (the landing queue's one-line confirm). */
    var landNowAsk by mutableStateOf<LandNowTarget?>(null)
    /** The placement change the owner chose and has not yet confirmed (#834). */
    var placementAsk by mutableStateOf<PlacementAsk?>(null)

    /** The button was pressed: ask first if the daemon says to, else act. */
    fun pressUpdateDaemon() {
        if (daemonUpdateConfirm.isEmpty()) updateDaemon() else confirmingDaemonUpdate = true
    }

    /** "Keep waiting". */
    fun cancelDaemonUpdate() { confirmingDaemonUpdate = false }

    /** "Update now" in the dialog. */
    fun confirmDaemonUpdate() {
        confirmingDaemonUpdate = false
        updateDaemon()
    }

    /**
     * The owner's button: the daemon builds its newest version, proves it and takes over; while an
     * update waits for landing batches it takes over now. A refusal lands in [updateError].
     */
    fun updateDaemon() {
        updateError = null
        val now = daemonUpdateButton == "Update now"
        raw("POST", if (now) "/update/now" else "/update", id = "update-daemon", json = "{}")
    }

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
    private var searchState by mutableStateOf(SearchState())
    /** The sidebar search. Turning body search on or off re-subscribes: the daemon sends history-row bodies only then. */
    var search: SearchState
        get() = searchState
        set(v) {
            val was = searchState.wantsRowBodies
            searchState = v
            if (was != v.wantsRowBodies) subscribe?.invoke()
        }
    var textScale by mutableStateOf(prefs.double(KEY_SCALE) ?: 1.0)
        private set
    /** The panel's width before text-zoom scaling; always inside the token limits. */
    var sidebarBase by mutableStateOf(clampSidebar(prefs.double(KEY_SIDEBAR) ?: MasonTokens.sizeSidebarWidth.toDouble()))
        private set
    var muted by mutableStateOf(prefs.bool(KEY_MUTE) ?: false)
        private set
    var project: String
        get() = projectState
        set(v) {
            val changed = v != projectState
            projectState = v
            attentionSeen = null
            if (changed) clearProjectScoped()
        }
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
    /** What the last Skip testing click did; kept apart from [lastResult] so the next ok result does not wipe it. */
    var skipNotice by mutableStateOf<String?>(null)
    /** What the last Land now for all click did, apart from [lastResult]. */
    var landNotice by mutableStateOf<String?>(null)
    /** Commands sent and not yet settled, by id (task #522). */
    var pending by mutableStateOf<Map<String, PendingCommand>>(emptyMap())
        internal set
    /** The daemon's refusal of the last command about a task, by task id (0: not about a task). */
    var actionErrors by mutableStateOf<Map<Long, String>>(emptyMap())
        internal set
    internal var nextActionId = 0
    /** A long command opened from a Compilation row; null: the dialog is closed. */
    var commandDialog by mutableStateOf<CommandDialog?>(null)
        private set
    fun openCommand(label: String, command: String) {
        Format.commandDialog(label, command)?.let { commandDialog = it }
    }
    fun closeCommand() { commandDialog = null }
    /** Open session found the step running: the task to ask about. Answer with [answerSessionPrompt]. */
    var sessionPrompt by mutableStateOf<Long?>(null)
        private set
    /** What Open session did or could not do, in words, until the next click. */
    var sessionNotice by mutableStateOf<String?>(null)
        private set
    /** Starts a terminal running this argument vector; false when it could not. The app sets it. */
    var launcher: ((List<String>) -> Boolean)? = null
    /** Where process launches run. The app sets a worker pool; the default runs inline. */
    var background: java.util.concurrent.Executor = java.util.concurrent.Executor { it.run() }
    /** Where results of [background] work come back (the UI thread). The app sets it; the default runs inline. */
    var ui: java.util.concurrent.Executor = java.util.concurrent.Executor { it.run() }
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

    /** The owner session whose dialog is open (null: closed) and what the daemon last said of it (s364, s432). */
    var ownerSessionDialog by mutableStateOf<Long?>(null)
    var ownerSession by mutableStateOf<OwnerSessionTranscript?>(null)
    var ownerSessionError by mutableStateOf<String?>(null)

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

    /**
     * Where the shown page is scrolled now. Plain, not state: it changes every frame of a
     * scroll, and is saved into the history only when the page is left.
     */
    var liveScroll: Int = 0
        private set
    fun reportScroll(y: Int) { liveScroll = maxOf(0, y) }

    private fun navigate(f: (NavHistory, Int) -> NavHistory) {
        val before = history.visit
        history = f(history, liveScroll)
        if (history.visit != before) liveScroll = history.restore
        subscribe?.invoke()
    }

    fun go(p: Page) = navigate { h, s -> h.go(p, s) }
    /** Whether [n] is a task this monitor has heard of (in the sidebar, or named by the open task's page). */
    fun isKnownTask(n: Long): Boolean = n in knownIds()

    /** How many times the known-id set was rebuilt (tests). */
    internal var knownRebuilds = 0
        private set
    private var knownSidebar: SidebarView? = null
    private var knownDetail: TaskDetailView? = null
    private var knownSet: Set<Long>? = null

    /** The ids this monitor has heard of, rebuilt only when the sidebar or the task page is a new value. */
    private fun knownIds(): Set<Long> {
        val sb = sidebar
        val d = taskDetail
        knownSet?.let { if (sb === knownSidebar && d === knownDetail) return it }
        knownRebuilds++
        val ids = HashSet<Long>()
        sb?.sections?.forEach { sec -> sec.rows.forEach { ids += it.id } }
        d?.row?.let { r ->
            ids += r.id; ids += r.blockers; ids += r.blocks
            d.debts.forEach { ids += it.task }
            d.proposal?.filed_by?.let { ids += it }
        }
        knownSidebar = sb; knownDetail = d; knownSet = ids
        return ids
    }

    /** [text] as plain text, task links and URL links, for the open task's page. */
    fun linkify(text: String): List<LinkSegment> =
        Linkify.segments(text, { isKnownTask(it) }, taskDetail?.ci_links ?: emptyList())

    /** Pick the CPU graph's span and ask the daemon for that range. */
    fun pickCpuSpan(s: CpuSpan) {
        if (s == cpuSpan) return
        cpuSpan = s
        cpuHistory = null
        subscribe?.invoke()
    }
    /** Pick the disk graph's span and ask the daemon for that range. */
    fun pickDiskSpan(s: DiskSpan) {
        if (s == diskSpan) return
        diskSpan = s
        diskHistory = null
        subscribe?.invoke()
    }

    /** Show or hide a volume's line; remembered. */
    fun toggleVolume(id: String) {
        hiddenVolumes = if (id in hiddenVolumes) hiddenVolumes - id else hiddenVolumes + id
        prefs.setString(KEY_HIDDEN_VOLUMES, encodeHiddenVolumes(hiddenVolumes))
    }

    private fun noteDisk(h: mason.v1.DiskHistory) {
        diskHistory = h
        diskSeen = true
        diskColors = diskPalette.assign(h.volumes.map { it.id })
        if (h.volumes.isNotEmpty()) {
            val kept = pruneHiddenVolumes(hiddenVolumes, h)
            if (kept != hiddenVolumes) {
                hiddenVolumes = kept
                prefs.setString(KEY_HIDDEN_VOLUMES, encodeHiddenVolumes(kept))
            }
        }
    }

    fun goBack() = navigate { h, s -> h.goBack(s) }
    fun goForward() = navigate { h, s -> h.goForward(s) }
    fun goHome() = navigate { h, s -> h.goHome(s) }
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

    /** Stored section flags, each read from the store once (the store is a native call on macOS). */
    private val storedExpanded = HashMap<String, Boolean?>()

    fun isExpanded(s: SidebarSection): Boolean {
        val over = overrides[s.state]  // read first: the state read is what recomposes on a change
        val key = sectionKey(s.state)
        val stored = if (storedExpanded.containsKey(key)) storedExpanded[key]
            else prefs.bool(key).also { storedExpanded[key] = it }
        return stored ?: over ?: !s.collapsed
    }

    fun setExpanded(state: String, open: Boolean) {
        overrides = overrides + (state to open)
        storedExpanded[sectionKey(state)] = open
        prefs.setBool(sectionKey(state), open)
    }

    // ---- paused banners ----

    /** The pause keys whose banner is collapsed to its icon; kept per device, across launches. */
    var collapsedPauses by mutableStateOf(
        (prefs.string(PAUSE_INDEX) ?: "").split("\n").filter { it.isNotEmpty() }.toSet(),
    )
        private set

    fun isPauseCollapsed(project: String, mode: String) = pauseKey(project, mode) in collapsedPauses

    fun setPauseCollapsed(project: String, mode: String, collapsed: Boolean) {
        val key = pauseKey(project, mode)
        prefs.setBool(key, collapsed)
        storeCollapsedPauses(if (collapsed) collapsedPauses + key else collapsedPauses - key)
    }

    private fun storeCollapsedPauses(keys: Set<String>) {
        collapsedPauses = keys
        prefs.setString(PAUSE_INDEX, keys.joinToString("\n"))
    }

    /** A pause no longer in the header (resumed or expired) forgets its collapse, so a later one starts expanded. */
    private fun prunePauseCollapse() {
        val h = headerState ?: return
        val live = h.banners.filter { it.kind == "paused" }.map { pauseKey(it.project, it.mode) }.toSet()
        val gone = collapsedPauses - live
        if (gone.isEmpty()) return
        gone.forEach { prefs.setBool(it, false) }
        storeCollapsedPauses(collapsedPauses - gone)
    }

    /** One row of the pause group: banner indices. Collapsed icons lead the row; `expanded` null is an icon-only row. */
    data class PauseRow(val collapsed: List<Int>, val expanded: Int?)

    /** Collapsed icons ride in the first expanded pause's row, or share one icon-only row when all are collapsed. */
    fun pauseRows(): List<PauseRow> {
        val all = (header?.banners ?: emptyList()).withIndex().filter { it.value.kind == "paused" }
        val (c, e) = all.partition { isPauseCollapsed(it.value.project, it.value.mode) }
        val collapsed = c.map { it.index }
        if (e.isEmpty()) return if (collapsed.isEmpty()) emptyList() else listOf(PauseRow(collapsed, null))
        return e.mapIndexed { n, x -> PauseRow(if (n == 0) collapsed else emptyList(), x.index) }
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
        if (clickCount == 2) tileDoubleClicked(state)
    }

    /** A double click: the lone task when the stage holds exactly one, else the stage's page (if it has one). */
    fun tileDoubleClicked(state: String) {
        if (!connected) return
        val n = sidebar?.sections?.firstOrNull { it.state == state }?.rows?.size ?: 0
        if (n == 0 && state == "failed") return
        if (n == 1) revealAndOpenIfOne(state) else Page.sectionPage(state)?.let { go(it) }
    }

    /** Reveal the section and, if it holds exactly one task right now (search ignored), open it. Nothing while disconnected. */
    fun revealAndOpenIfOne(state: String) {
        if (!connected) return
        reveal(state)
        val sec = sidebar?.sections?.firstOrNull { it.state == state } ?: return
        if (sec.rows.size == 1) go(Page.Task(sec.rows[0].id))
    }

    /**
     * Whether the main view shows one of the tasks behind this tile: the page is a task page and
     * its id is a row of the sidebar section with the tile's state.
     */
    fun tileShowsCurrentTask(c: StateCount): Boolean {
        val p = page as? Page.Task ?: return false
        return sidebar?.sections?.firstOrNull { it.state == c.state }?.rows?.any { it.id == p.id } ?: false
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
        val withActor = if (c.actor.isEmpty()) c.copy(actor = identity) else c
        // Recorded before the write, so the pressed control redraws at once; a second click
        // while one is in flight sends nothing.
        val out = beginPending(withActor) ?: return
        sender?.invoke(out)
    }

    fun cancel(task: Long) = send(Command(cancel = TaskRef(task = task)))
    fun retry(task: Long, from: String = "", seen: Long = 0) = send(Command(retry = Retry(task = task, from = from, seen = seen)))
    fun approve(task: Long, priority: String = "") = send(Command(approve = ApproveTask(task = task, priority = priority)))
    /** Put a proposed task aside; it comes back only by hand. */
    fun deferTask(task: Long) = send(Command(defer = TaskRef(task = task)))
    /** Move a deferred task back to proposed. */
    fun undefer(task: Long) = send(Command(undefer = TaskRef(task = task)))
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
    fun approve(task: Long, a: Approval, priority: String = MonitorModel.DEFAULT_APPROVAL_PRIORITY) = if (a.diff_hash.isEmpty()) approve(task, priority) else approveHold(task, a.diff_hash)

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
    /** Land now for a whole running batch: every member lands past its remaining tests, which run on the base after the merge. */
    fun landNowBatch(batch: Long) = send(Command(land_now_batch = BatchRef(batch = batch)))
    fun force(task: Long) = send(Command(force = TaskRef(task = task)))
    fun skipTest(task: Long, step: String) = send(Command(skip = Skip(task = task, step = step)))
    /** Make a task immune to testing (a fast track): no test runs for it through landing; the full run is owed. */
    fun makeTestImmune(task: Long) = send(Command(skip = Skip(task = task, all_tests = true)))
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

    /** Lift exactly one pause: its project (empty for everything) and its mode. */
    fun resumePause(project: String, mode: String) = send(Command(resume = ResumeWork(project = project, mode = mode)))
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
        val argv = OpenSession.argv(cliPath, task, anyway)
        val launch = launcher
        val cli = cliPath
        background.execute {
            val ok = runCatching { launch?.invoke(argv) == true }.getOrDefault(false)
            if (!ok) ui.execute {
                val line = OpenSession.commandLine(cli, task, anyway)
                copier?.invoke(line)
                sessionNotice = OpenSession.copiedNotice(line)
            }
        }
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

    /** Open the owner-session dialog on a session (the health banner's and the sidebar's button). */
    fun openOwnerSession(id: Long) {
        ownerSessionDialog = id
        ownerSession = null
        ownerSessionError = null
        refreshOwnerSession()
    }

    fun closeOwnerSession() {
        ownerSessionDialog = null
        ownerSession = null
        ownerSessionError = null
    }

    private fun refreshOwnerSession() {
        val id = ownerSessionDialog ?: return
        raw("GET", "/sessions/$id", id = "osession:$id")
    }

    fun sendOwnerMessage(text: String) {
        val id = ownerSessionDialog ?: return
        if (text.isBlank()) return
        raw("POST", "/sessions/$id/messages", id = "osession-send:$id", json = EditorJson.text(mapOf("text" to text)))
    }

    fun endOwnerSession() {
        val id = ownerSessionDialog ?: return
        raw("POST", "/sessions/$id/end", id = "osession-end:$id", json = "{}")
    }

    fun closeSshKeys() {
        sshKeyProject = null
        sshKeys = null
        sshKeyResult = null
        sshKeyBusy = false
    }

    fun raw(method: String, path: String, id: String = "", json: String = "") =
        send(Command(id = id, raw = Raw(method = method, path = path, json = json)))

    // ---- settings (s430) ----

    /** Ask for the Settings page's data: the project's settings and the notification routes. */
    fun loadSettings() {
        val p = editorProject
        if (p.isNotEmpty()) raw("GET", SettingsPayload.path(p), id = "settings")
        raw("GET", "/notify/settings", id = "notify-settings")
        raw("GET", "/remote", id = "remote")
    }

    /** Change the global placement mode; the page reloads `/remote` on the answer. */
    fun changeGlobalMode(mode: String) {
        settingsNotice = null
        raw("POST", "/remote", id = "remote-save", json = SettingsPayload.remoteMode(mode))
    }

    /** Change the selected project's placement mode. */
    fun changeProjectMode(mode: String) {
        settingsNotice = null
        raw("POST", "/remote", id = "remote-save", json = SettingsPayload.remoteMode(mode, editorProject))
    }

    /** Change one project setting (a body from [SettingsPayload]); the page reloads on the answer. */
    fun changeProjectSetting(body: String) {
        settingsNotice = null
        raw("POST", SettingsPayload.path(editorProject), id = "settings-save", json = body)
    }

    fun changeNotifySetting(body: String) {
        settingsNotice = null
        raw("POST", "/notify/settings", id = "notify-save", json = body)
    }

    fun testNotification(channel: String) {
        settingsNotice = null
        raw("POST", "/notify/test", id = "notify-test", json = SettingsPayload.test(channel))
    }

    /** Ask for the LAN state and the paired devices. */
    fun loadDevices() {
        raw("GET", "/lan", id = "lan")
        raw("GET", "/devices", id = "devices")
    }

    /** Make a pairing code for a new device; [role] is "owner" or "viewer". */
    fun issuePairCode(role: String) {
        devicesNotice = null
        raw("POST", "/pair/codes", id = "pair-code", json = Codec.body(mapOf("role" to role)))
    }

    fun closePairing() { pairOffer = null }

    fun revokeDevice(id: Long) {
        devicesNotice = null
        raw("POST", "/devices/$id/revoke", id = "device-revoke:$id", json = "{}")
    }

    fun runSchedule(name: String) {
        settingsNotice = null
        raw("POST", "/schedules/run", id = "schedule-run", json = SettingsPayload.runSchedule(editorProject, name))
    }

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

    /**
     * How held progress is put off to the next frame: `flush` is to run once, later, on the UI thread.
     * Null (headless tools, tests) applies each progress message at once.
     */
    var scheduleProgressFlush: ((() -> Unit) -> Unit)? = null
    private val pendingProgress = ArrayList<StepProgress>()
    private var flushScheduled = false

    /** A progress message from the stream: held until the next frame, so a burst costs one sidebar publish. */
    internal fun enqueue(p: StepProgress) {
        val schedule = scheduleProgressFlush
        if (schedule == null) {
            apply(p)
            return
        }
        pendingProgress += p
        if (flushScheduled) return
        flushScheduled = true
        schedule { flushProgress() }
    }

    /** Apply what was held, in order, as one batch. Every other message flushes first, so order is kept. */
    fun flushProgress() {
        flushScheduled = false
        if (pendingProgress.isEmpty()) return
        val batch = pendingProgress.toList()
        pendingProgress.clear()
        applyAll(batch)
    }

    /** A step's progress arrived without a view: put it into the task page and the rows, and follow the log. */
    fun apply(p: StepProgress, local: Long = System.currentTimeMillis()) = applyAll(listOf(p), local)

    /** Several progress messages at once: one write to the task page, one to the sidebar, one pass over its rows. */
    private fun applyAll(ps: List<StepProgress>, local: Long = System.currentTimeMillis()) {
        ps.lastOrNull { it.now_ms > 0 }?.let { anchor(it.now_ms, local) }
        val withCell = ps.filter { it.progress != null }
        if (withCell.isEmpty()) return
        fun merged(old: List<ProgressCell>, cell: ProgressCell): List<ProgressCell>? {
            val i = old.indexOfFirst { it.step == cell.step }
            if (i < 0) return null
            // A finished or stopped run says what it ended as on the next view; until then, no "running".
            val c = if (cell.state != CellState.PART) cell.copy(label = "") else cell
            return old.toMutableList().also { it[i] = c }
        }
        taskDetail?.let { d ->
            var row = d.row
            if (row != null) {
                var changed = false
                val lines = ArrayList<String>()
                for (p in withCell) {
                    val cur = row ?: break
                    if (cur.id != p.task) continue
                    val cells = merged(cur.progress, p.progress!!)
                    if (cells != null) { row = cur.copy(progress = cells); changed = true }
                    lines += p.log
                }
                if (changed) taskDetail = d.copy(row = row)
                appendLog(lines)
            }
        }
        sidebar?.let { s ->
            val latest = HashMap<Long, MutableList<ProgressCell>>()
            for (p in withCell) latest.getOrPut(p.task) { ArrayList() }.add(p.progress!!)
            var changed = false
            val sections = s.sections.map { sec ->
                sec.copy(rows = sec.rows.map { row ->
                    val cells = latest[row.id]
                    if (cells == null) row else {
                        var progress = row.progress
                        var hit = false
                        for (c in cells) merged(progress, c)?.let { progress = it; hit = true }
                        if (hit) { changed = true; row.copy(progress = progress) } else row
                    }
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
        m.step_progress?.let { enqueue(it); return }
        flushProgress()
        m.welcome?.let { w ->
            connected = true
            reconnecting = false
            announcer.connectionRestored()
            pending = emptyMap()
            if (w.resume == ResumeKind.SNAPSHOT) { lastSeq = 0; lastViewSeq = 0; appliedViewSeq.clear() }
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
            // A view older than one already applied (out-of-order arrival) is stale: drop it.
            val kind = u.view?.let(::viewKind)
            val stale = kind != null && u.seq > 0 && u.seq < (appliedViewSeq[kind] ?: 0L)
            lastSeq = max(lastSeq, u.seq)
            if (!stale) {
                lastViewSeq = max(lastViewSeq, u.seq)
                if (kind != null) appliedViewSeq[kind] = max(appliedViewSeq[kind] ?: 0L, u.seq)
                u.view?.let(::apply)
            }
        }
        m.result?.let { r ->
            lastResult = if (r.ok) null else r.error
            settle(r)
            if (r.id.startsWith("session:")) r.id.removePrefix("session:").toLongOrNull()?.let { task ->
                when (val d = OpenSession.decide(r.ok, r.json, r.error)) {
                    OpenSession.Decision.Confirm -> sessionPrompt = task
                    is OpenSession.Decision.Launch -> launchSession(task, d.anyway)
                    is OpenSession.Decision.Failed -> sessionNotice = d.why
                }
            }
            if (r.id.startsWith("osession:") && r.ok) ownerSession = OwnerSessionTranscript.parse(r.json)
            if ((r.id.startsWith("osession:") || r.id.startsWith("osession-send:")) && !r.ok) ownerSessionError = r.error
            if (r.id.startsWith("osession-send:") && r.ok) refreshOwnerSession()
            if (r.id.startsWith("osession-end:") && r.ok) closeOwnerSession()
            if (r.id.startsWith("ssh-keys:")) { if (r.ok) sshKeys = SshKeyList.parse(r.json) else sshKeyResult = r.error }
            if (r.id.startsWith("ssh-key:")) {
                sshKeyBusy = false
                if (r.ok) { sshKeyResult = "The remote accepted the key. Landing is released."; sshKeyProject = null } else sshKeyResult = r.error
            }
            if (r.id == "announce" && r.ok) announcer.handleClaim(r.json)
            if (r.id == "update-daemon" && !r.ok) updateError = r.error
            if (r.id == "update-monitor") {
                if (r.ok) monitorAsked = true else monitorUpdateFailed(r.error)
            }
            if (r.id == "health" && r.ok) health = HealthSnapshot.parse(r.json)
            if (r.id == "introspection" && r.ok) introspection = IntrospectionSnapshot.parse(r.json)
            if (r.id == "settings" && r.ok) settings = SettingsSnapshot.parse(r.json)
            if (r.id == "lan" && r.ok) lanStatus = LanStatus.parse(r.json)
            if (r.id == "devices" && r.ok) devices = DeviceList.parse(r.json)
            if (r.id == "pair-code") {
                if (r.ok) {
                    pairOffer = PairOffer.parse(r.json, System.currentTimeMillis())
                    if (pairOffer == null) devicesNotice = "The daemon sent a pairing code this monitor could not read."
                } else devicesNotice = r.error
            }
            if (r.id.startsWith("device-revoke:")) {
                if (r.ok) { devicesNotice = "Revoked."; loadDevices() } else devicesNotice = r.error
            }
            if (r.id == "notify-settings" && r.ok) notifySettings = NotifySettings.parse(r.json)
            if (r.id == "remote" && r.ok) remoteSettings = RemoteSettings.parse(r.json)
            if (r.id in SETTINGS_RESULTS) {
                settingsNotice = if (r.ok) SettingsPayload.done(r.id) else r.error
                if (r.ok && (r.id == "settings-save" || r.id == "notify-save")) loadSettings()
                if (r.ok && r.id == "remote-save") raw("GET", "/remote", id = "remote")
            }
            if (r.id.startsWith("publish:") && r.ok) publishPreview = PublishPreview.parse(r.json)
            onResult?.invoke(r)
        }
    }

    /** Forget every number computed for the previous selection, so it never shows under the new
     *  name. The header keeps its project list and banners (the picker needs them); its counts go. */
    internal fun clearProjectScoped() {
        header = header?.copy(counts = emptyList(), attention = 0, status = "", project = projectState)
        sidebar = null
        home = null
        landing = null
    }

    /** The project a view was computed for, when it is one of the project-scoped views. */
    private fun scopeOf(v: View): String? =
        v.header_?.project ?: v.sidebar?.project ?: v.home?.project ?: v.landing?.project

    /** Chime once when a task that needs the owner appears that was not there before. */
    internal fun noteAttention(s: SidebarView) {
        val ids = s.sections.filter { it.state in WANTING }.flatMap { it.rows.map { r -> r.id } }.toSet()
        val seen = attentionSeen
        attentionSeen = ids
        if (seen != null && !muted && !announcer.heardFromDaemon && (ids - seen).isNotEmpty()) onAttention?.invoke()
    }

    fun apply(v: View) {
        // A view computed for another filter (a rebuild in flight when the picker changed, or
        // one resumed after a reconnect) is dropped.
        scopeOf(v)?.let { if (it != projectState) return }
        v.header_?.let {
            header = it
            noteMonitorBuild()
            // An open dialog follows the session: a reply arrives as a new header.
            if (ownerSessionDialog != null) refreshOwnerSession()
        }
        v.sidebar?.let { sidebar = it; noteAttention(it); settleAfterView() }
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
            settleAfterView()
        }
        v.cpu_history?.let { cpuHistory = it }
        v.disk_history?.let { noteDisk(it) }
        v.landing?.let { landing = it }
        v.usage?.let { usage = it }
        v.editor?.let { editor = it }
    }

    fun disconnected(nowMs: Long = System.currentTimeMillis()) {
        connected = false
        reconnecting = true
        markPendingNotSent()
        announcer.connectionLost(nowMs)
    }

    companion object {
        /** The priorities the approval picker offers, and the one it starts at. */
        val APPROVAL_PRIORITIES = listOf("urgent", "high", "normal", "low")
        const val DEFAULT_APPROVAL_PRIORITY = "normal"
        /** The CPU gate's range in the monitor, and the most the settle stepper asks for. */
        const val CPU_GATE_MIN = 10
        const val CPU_GATE_MAX = 100
        const val SETTLE_MAX = 600

        /** The project a `push_auth` banner is about: its text starts "<project>: ". */
        fun pushAuthProject(b: Banner): String = b.text.substringBefore(":")

        /** The protocol this monitor speaks. */
        const val PROTOCOL = 2
        const val MONITOR_KIND = "compose"
        const val LOG_KEEP = 40
        const val CLAIM_EVERY_MS = 3_000L
        const val KEY_SCALE = "textScale"
        const val KEY_MUTE = "muted"
        const val KEY_HIDDEN_VOLUMES = "disk.hidden"
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
        /** The identity attention.rs uses for a pause. */
        fun pauseKey(project: String, mode: String) = "paused:$project:$mode"
        const val PAUSE_INDEX = "paused.collapsed.keys"
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

    /** Whether the daemon must send history rows with their bodies: only while a body search runs. */
    val wantsRowBodies get() = inBody && isActive

    fun matches(r: TaskRow): Boolean {
        val q = text.trim().lowercase()
        if (q.isEmpty()) return true
        if (inName && (r.project.lowercase().contains(q) || r.key.lowercase().contains(q))) return true
        if (inTitle && r.title.lowercase().contains(q)) return true
        if (inBody && r.body.lowercase().contains(q)) return true
        return false
    }
}

/** How long the task has been in its state at daemon time [now] (ms): from `state_since_ms`, or the daemon's own `in_state_ms` when it sent none. */
fun TaskRow.inState(now: Long): Long = if (state_since_ms == 0L) in_state_ms else maxOf(0L, now - state_since_ms)
