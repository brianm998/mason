// SPDX-License-Identifier: GPL-3.0-or-later

import Foundation
import Observation
import MasonProtocol
import MasonTokens

public typealias HeaderView = Mason_V1_HeaderView
public typealias SidebarView = Mason_V1_SidebarView
public typealias SidebarSection = Mason_V1_SidebarSection
public typealias HomeView = Mason_V1_HomeView
public typealias TaskDetailView = Mason_V1_TaskDetailView
public typealias LandingView = Mason_V1_LandingView
public typealias UsageView = Mason_V1_UsageView
public typealias EditorView = Mason_V1_EditorView
public typealias StateCount = Mason_V1_StateCount
public typealias CiLink = Mason_V1_CiLink

/// What the monitor knows and what the user has done to it. The client draws
/// this and sends commands; it never re-derives pipeline rules: counts,
/// sorting, allowed actions and waiting text all arrive from the daemon.
/// State that changes on nearly every message. Views that show it observe this object; the rest
/// of the model, and every sidebar row, do not. `clockOffsetMs` is not published: the progress
/// lines read it on their shared one-second tick.
@MainActor
@Observable
public final class LiveState {
    public var lastSeq: UInt64 = 0
    public var liveLog: [String] = []
    @ObservationIgnored public var clockOffsetMs: Int64 = 0
    public init() {}

    /// The daemon's time now, ms since the epoch, by this device's clock and the offset.
    public func daemonNow(_ local: Date = Date()) -> UInt64 {
        let ms = Int64(local.timeIntervalSince1970 * 1000) + clockOffsetMs
        return ms > 0 ? UInt64(ms) : 0
    }
}

@MainActor
@Observable
public final class MonitorModel {
    // From the daemon.
    public var header: HeaderView? { didSet { prunePauseCollapse(); pruneDismissed() } }
    public var sidebar: SidebarView? { didSet { sidebarRevision &+= 1; linkRevision &+= 1 } }
    /// Bumped on every new sidebar, so derived layouts know when to be rebuilt.
    @ObservationIgnored var sidebarRevision: UInt64 = 0
    /// Bumped when the sidebar or the open task changes: what `isKnownTask` and `linkify` depend on.
    @ObservationIgnored var linkRevision: UInt64 = 0
    @ObservationIgnored private var knownTaskCache: (revision: UInt64, ids: Set<UInt64>)?
    @ObservationIgnored private var linkCache: (revision: UInt64, segments: [String: [LinkSegment]]) = (0, [:])
    /// How many times `linkify` actually ran `Linkify.segments` (for tests and the exit test).
    @ObservationIgnored public private(set) var linkifyPasses = 0
    /// How many task-id lookups `linkify` made.
    @ObservationIgnored public private(set) var isKnownTaskCalls = 0
    @ObservationIgnored var sidebarEntriesCache: (key: SidebarEntriesKey, entries: [SidebarEntry])?
    public var home: HomeView?
    /// The CPU shares, observed apart from the whole model: the CPU section watches only this.
    public let cpu = CpuModel()
    /// The disk history, observed apart from the whole model like `cpu`.
    public let disk: DiskModel
    public var taskDetail: TaskDetailView? { didSet { linkRevision &+= 1 } }
    public var landing: LandingView?
    public var usage: UsageView?
    public var editor: EditorView?
    public var health: HealthSnapshot?
    public var introspection: IntrospectionSnapshot?
    /// The Settings page (s430): the picked project's settings, the global notification routes,
    /// and what the last change or test said.
    public var settings: SettingsSnapshot?
    public var notifySettings: NotifySettings?
    public var settingsNotice: String?
    public var remoteSettings: RemoteSettings?
    /// Running work on other Macs: this Mac's own switch (a node), and the nodes (a coordinator).
    public var nodeSelf: NodeSelf?
    public var nodes: NodesSnapshot?
    /// The Devices section (s433): LAN state, paired devices, the code on show, and what the last action said.
    public var lanStatus: LanStatus?
    public var devices: DeviceList?
    public var pairOffer: PairOffer?
    public var devicesNotice: String?
    /// The high-frequency state (last sequence number, live log, clock offset) lives in its own
    /// object, so a change to it redraws only the views that show it, never the sidebar rows.
    public let live = LiveState()
    /// Live progress cells by task id, apart from `sidebar` and `taskDetail` (task #517).
    public let progress = ProgressStore()
    /// A task's cells as shown now: live ones if a progress message set them, else its last view's.
    public func progressCells(task id: UInt64) -> [ProgressCell] {
        if let c = progress.cells(id) { return c }
        for sec in sidebar?.sections ?? [] { if let r = sec.rows.first(where: { $0.id == id }) { return r.progress } }
        return []
    }
    public var lastSeq: UInt64 {
        get { live.lastSeq }
        set { live.lastSeq = newValue }
    }
    /// The highest seq a view or snapshot has carried. Events arrive before the views of the same
    /// seq, so `lastSeq` alone does not say the screen has caught up.
    @ObservationIgnored public private(set) var lastViewSeq: UInt64 = 0
    /// The daemon's clock minus this device's, ms, as of the last message that carried the daemon's
    /// time (the welcome, each StepProgress): "updated 4 s ago" counts on the local clock, corrected by this.
    public private(set) var clockOffsetMs: Int64 {
        get { live.clockOffsetMs }
        set { live.clockOffsetMs = newValue }
    }
    /// The open task's live log: its last 40 lines, followed from the stream.
    public private(set) var liveLog: [String] {
        get { live.liveLog }
        set { live.liveLog = newValue }
    }
    public var connected = false
    /// The Add Project wizard while it is open (desktop only; see `canAddProject`).
    public var addProject: AddProjectWizard?
    /// The connection is down and being retried: the daemon is restarting or has
    /// not answered. Shown as "reconnecting", never as a hard error.
    public var reconnecting = false
    /// What the remote link is doing ("Looking for MASON on studio…", "Reached through studio.local"),
    /// or nil for the loopback connection. The start and banner views say it when set.
    public var linkStatus: String?
    /// Whether the daemon took this connection's identity for the owner's (its welcome says).
    /// Only then are the answer, approve and reject buttons drawn: anyone else sees the card.
    public var isOwner = false
    public var canAct: Bool { isOwner && readOnlyReason == nil }

    // About versions (Part 15).
    /// The protocol this monitor speaks.
    public static let protocolVersion: UInt32 = 2
    /// This monitor's own build: a commit, or "dev" for a build nobody versioned.
    /// The commit comes from the environment, else from `BuildStamp`, which every build made by
    /// the daemon or `make` stamps; a build nobody stamped stays "dev" and does not nag.
    @ObservationIgnored public var ownVersion: String = ProcessInfo.processInfo.environment["MASON_MONITOR_VERSION"]
        ?? (BuildStamp.commit.isEmpty ? "dev" : BuildStamp.commit)
    /// Where this monitor's executable is, so the daemon can rebuild a bare binary in place.
    @ObservationIgnored public var ownPath: String = ""
    /// The daemon's build, from its welcome.
    public var daemonVersion = ""
    /// Set when the daemon needs a newer protocol than this monitor speaks: nothing
    /// can be sent, and this says why.
    public var readOnlyReason: String?
    /// What this monitor's update is doing, while the user has asked for one.
    public var updating: String?
    /// Why the last update of this monitor did not happen.
    public var updateError: String?
    /// Puts the finished build (a path on this machine) in place, launches it and quits.
    /// Only the desktop app sets this; a phone has no way to replace itself.
    @ObservationIgnored public var onInstall: ((String) -> Void)?
    /// The daemon said this monitor's build is on its way.
    @ObservationIgnored private var monitorAsked = false
    /// The connection is to a daemon on this machine, so a path it gives is one we can open.
    public private(set) var localDaemon = true
    public static let monitorKind = "swift"

    var updateInfo: Mason_V1_UpdateInfo? { header?.hasUpdate == true ? header?.update : nil }
    var monitorBuild: Mason_V1_MonitorBuild? { updateInfo?.monitors.first { $0.kind == Self.monitorKind } }

    /// Has this monitor's own source changed on the update channel? Never for a build
    /// nobody stamped, so development builds do not nag.
    public var updateAvailable: Bool {
        guard canAct, localDaemon, onInstall != nil, ownVersion != "dev", let b = monitorBuild else { return false }
        return !b.latest.isEmpty && b.latest != ownVersion
    }
    /// The commit a monitor update would go to.
    public var latestMonitor: String { monitorBuild?.latest ?? "" }

    /// How many commits behind develop the daemon says it is (parsed from the daemon's own
    /// sentence, "masond is 3 commits behind develop"); nil when it does not say.
    public var daemonCommitsBehind: Int? {
        let t = daemonUpdateText
        guard let r = t.range(of: #"(\d+) commits? behind"#, options: .regularExpression) else { return nil }
        return Int(t[r].prefix { $0.isNumber })
    }
    private static func shortCommit(_ v: String) -> String { String(v.prefix(8)) }
    /// Home's version line: the running masond's short commit (or "unversioned build"), how far
    /// behind develop it is, and this monitor's own commit. Empty before a welcome.
    public var versionLine: String {
        if daemonVersion.isEmpty { return "" }
        var d = daemonVersion == "dev" ? "masond (unversioned build)" : "masond \(Self.shortCommit(daemonVersion))"
        if let n = daemonCommitsBehind { d += " (\(n) commit\(n == 1 ? "" : "s") behind develop)" }
        let o = ownVersion == "dev" ? "monitor (unversioned build)" : "monitor \(Self.shortCommit(ownVersion))"
        return "\(d) · \(o)"
    }
    /// The full hashes, for the tooltip.
    public var versionTip: String {
        if daemonVersion.isEmpty { return "" }
        return "masond \(daemonVersion)\nmonitor \(ownVersion)"
    }

    /// What to say about the daemon's own version; empty when there is nothing to say.
    public var daemonUpdateText: String { updateInfo?.daemonText ?? "" }
    public var canUpdateDaemon: Bool { canAct && updateInfo?.daemonCanUpdate == true }

    /// On the `release` channel (a packaged install) there is no source to build from: the
    /// button opens the download page instead.
    public var downloadURL: String { updateInfo?.downloadURL ?? "" }
    @ObservationIgnored public var onOpenURL: ((URL) -> Void)?
    public func openDownloadPage() {
        if let u = URL(string: downloadURL), ["https", "http"].contains(u.scheme) { onOpenURL?(u) }
    }

    /// The button's label, as the daemon words it ("Update masond", or "Update now" while a
    /// finished update waits for landing batches).
    public var daemonUpdateButton: String {
        let b = updateInfo?.daemonButton ?? ""
        return b.isEmpty ? "Update masond" : b
    }
    /// What to ask before the button acts; empty when it acts at once.
    public var daemonUpdateConfirm: String { updateInfo?.daemonConfirm ?? "" }
    /// The confirmation dialog is open.
    public var confirmingDaemonUpdate = false
    /// The key of the notification row whose "?" dialog is open (#951).
    public var notificationDetailKey: String?
    /// The Land now the owner pressed and has not yet confirmed (the landing queue's one-line confirm).
    public var landNowAsk: LandNowTarget?
    /// The placement change the owner chose and has not yet confirmed (#834).
    public var placementAsk: PlacementAsk?

    /// The button was pressed: ask first if the daemon says to, else act.
    public func pressUpdateDaemon() {
        if daemonUpdateConfirm.isEmpty { updateDaemon() } else { confirmingDaemonUpdate = true }
    }

    /// "Keep waiting".
    public func cancelDaemonUpdate() { confirmingDaemonUpdate = false }

    /// "Update now" in the dialog.
    public func confirmDaemonUpdate() {
        confirmingDaemonUpdate = false
        updateDaemon()
    }

    /// The owner's button: the daemon builds its newest version, proves it and takes over; while
    /// an update waits for landing batches it takes over now. A refusal lands in `updateError`.
    public func updateDaemon() {
        updateError = nil
        var c = Mason_V1_Command()
        c.id = "update-daemon"
        let now = daemonUpdateButton == "Update now"
        c.raw = .with { $0.method = "POST"; $0.path = now ? "/update/now" : "/update"; $0.json = "{}" }
        send(c)
    }

    /// The owner's button: the daemon builds the newest monitor; `noteMonitorBuild` takes it from there.
    public func updateMonitor() {
        updateError = nil
        updating = "Building the new monitor…"
        var c = Mason_V1_Command()
        c.id = "update-monitor"
        let body: [String: String] = ["kind": Self.monitorKind, "own_path": ownPath, "own_version": ownVersion]
        let json = (try? JSONSerialization.data(withJSONObject: body, options: [.sortedKeys])).flatMap { String(data: $0, encoding: .utf8) } ?? "{}"
        c.raw = .with { $0.method = "POST"; $0.path = "/update/monitor"; $0.json = json }
        send(c)
    }

    /// The build can be stopped: it is running and the daemon is the one doing it.
    public var canCancelMonitorUpdate: Bool { monitorAsked && monitorBuild?.phase == "building" }

    /// The owner's Cancel while the new monitor builds; this one keeps running.
    public func cancelMonitorUpdate() {
        var c = Mason_V1_Command()
        c.id = "update-monitor-cancel"
        c.raw = .with { $0.method = "POST"; $0.path = "/update/monitor/cancel"; $0.json = "{\"kind\":\"\(Self.monitorKind)\"}" }
        send(c)
    }

    /// What the build is doing, in words: "Compiling 12 of 48", then the build's last line.
    public static func buildText(_ b: Mason_V1_MonitorBuild) -> String {
        var head: String
        switch b.step {
        case "fetching": head = "Fetching the new source…"
        case "waiting": head = "Waiting for a build slot…"
        case "compiling": head = b.total > 0 ? "Compiling \(b.done) of \(b.total)…" : "Compiling…"
        case "linking": head = "Linking…"
        case "installing": head = "Installing the new monitor…"
        default: return "Building the new monitor…"
        }
        if !b.lastLine.isEmpty, b.step == "compiling" || b.step == "linking" { head += " " + String(b.lastLine.prefix(90)) }
        return head
    }

    /// The update of this monitor stopped; this one keeps running.
    public func monitorUpdateFailed(_ why: String) {
        monitorAsked = false
        updating = nil
        updateError = why
    }

    /// Why the build stopped, with where its log is.
    public static func failureText(_ b: Mason_V1_MonitorBuild?) -> String {
        var t = "The new monitor did not build: \(b?.error ?? "")"
        if let log = b?.log, !log.isEmpty { t += " (log: \(log))" }
        return t
    }

    /// Follow the build the owner asked for, from each new header.
    private func noteMonitorBuild() {
        guard monitorAsked else { return }
        switch monitorBuild?.phase {
        case "building":
            if let b = monitorBuild { updating = Self.buildText(b) }
        case "ready":
            monitorAsked = false
            updating = "Starting the new monitor…"
            onInstall?(monitorBuild?.path ?? "")
        case "failed":
            monitorUpdateFailed(Self.failureText(monitorBuild))
        default:
            monitorUpdateFailed("The daemon restarted during the build. Press Update to start it again.")
        }
    }

    // The user's.
    public private(set) var history = NavHistory()
    public var search = SearchState() {
        // The daemon leaves history-row bodies out unless a body search needs them.
        didSet { if oldValue.wantsRowBodies != search.wantsRowBodies { subscribe?() } }
    }
    public var textScale: Double { didSet { prefs.setDouble(textScale, for: Keys.scale) } }
    /// The panel's width before text-zoom scaling; always inside the token limits.
    public private(set) var sidebarWidth: Double
    public var muted: Bool {
        didSet {
            prefs.setBool(muted, for: Keys.mute)
            // Muting silences sound and speech at once. Banners and the attention count are
            // drawn from the views, not from here, so they stay.
            muteBox.value = muted
            if muted { announcer.silence() }
        }
    }
    /// The queue of sounds and sentences (Part 19). The app swaps in a sink that plays them.
    public let announcer: Announcer
    /// The connection has been down long enough to call the daemon down: a banner, muted or not.
    public private(set) var daemonDown = false
    /// This monitor's name when it asks to speak; one per launch.
    public let clientId = UUID().uuidString
    /// The selected project filter ("" is all projects). Everything the daemon computes for a
    /// project is stamped with it; a change clears what was computed for the old one, and
    /// `apply` drops any view stamped for another.
    public var project = "" {
        didSet {
            attentionSeen = nil
            if oldValue != project { clearProjectScoped() }
        }
    }

    /// Forget every number computed for the previous selection, so it never shows under the new
    /// name. The header keeps its project list and banners (the picker needs them); its counts go.
    func clearProjectScoped() {
        if var h = header {
            h.counts = []
            h.attention = 0
            h.status = ""
            h.project = project
            header = h
        }
        progress.resetAll()
        sidebar = nil
        heldSidebar = nil
        heldHeader = nil
        home = nil
        landing = nil
    }
    /// Called (on the main actor) when something new needs the owner and the monitor is not
    /// muted: a task asked a question, waits for approval, was proposed, or failed. The app
    /// plays its sound here; the model only decides when.
    @ObservationIgnored public var onAttention: (() -> Void)?
    /// The tasks that needed the owner at the last update; nil until the first one, so what
    /// was already waiting when the monitor opened (or the project changed) is silent.
    @ObservationIgnored private var attentionSeen: Set<UInt64>?
    /// Sections the user opened or closed, remembered per device.
    private var overrides: [String: Bool] = [:]
    /// Where the sidebar should scroll to (a tile was clicked).
    public var scrollTarget: String?
    public var lastResult: String?
    /// What the last Skip testing click did ("Skipped testing for 4 tasks", "Nothing to skip: ..."),
    /// kept apart from `lastResult` so the daemon's next ok result does not wipe it.
    public var skipNotice: String?
    /// What the last Land now for all click did ("Land now sent for 3 tasks (1 batch)"), apart from `lastResult`.
    public var landNotice: String?
    /// Commands sent and not yet settled, by id (task #522).
    public internal(set) var pending: [String: PendingCommand] = [:]
    /// The daemon's refusal of the last command about a task, by task id (0: not about a task).
    public internal(set) var actionErrors: [UInt64: String] = [:]
    @ObservationIgnored var nextActionId = 0
    /// A long command opened from a Compilation row; nil: the dialog is closed.
    public var commandDialog: CommandDialog?
    /// Open session found the step running: the task to ask about. Answer with `answerSessionPrompt`.
    public var sessionPrompt: UInt64?
    /// What Open session did or could not do, in words, until the next click.
    public var sessionNotice: String?
    /// Starts a terminal running this argument vector; false when it could not. The app sets it.
    @ObservationIgnored public var launcher: (([String]) -> Bool)?
    /// Puts text on the pasteboard. The app sets it.
    @ObservationIgnored public var copier: ((String) -> Void)?
    /// The CLI by its full path.
    @ObservationIgnored public var cliPath = OpenSession.defaultCLI()
    /// What `mason proposal publish --preview` would send, in the daemon's words, once asked for.
    public var publishPreview: PublishPreview?
    /// The SSH key sheet after a refused push: the project it is for, the keys offered, and what
    /// choosing one said. nil project: the sheet is closed.
    public var sshKeyProject: String?
    public var sshKeys: SSHKeyList?
    public var sshKeyResult: String?
    public var sshKeyBusy = false
    /// The owner session whose dialog is open, and what the daemon last said of it (s364, s432).
    public var ownerSessionDialog: UInt64?
    public var ownerSession: OwnerSessionTranscript?
    public var ownerSessionError: String?
    /// The step selected in the pipeline editor (for its inspector).
    public private(set) var editorStep = ""
    /// Called with every command result (the tools read them; the app shows errors).
    @ObservationIgnored public var onResult: ((Mason_V1_CommandResult) -> Void)?

    let prefs: Preferences
    @ObservationIgnored public var sender: ((Mason_V1_Command) -> Void)?
    @ObservationIgnored public var subscribe: (() -> Void)?

    public enum Keys {
        static let scale = "textScale"
        static let mute = "muted"
        static let sidebarWidth = "sidebarWidth"
        static func section(_ s: String) -> String { "section.\(s).expanded" }
        static func pause(project: String, mode: String) -> String { "paused:\(project):\(mode)" }
        static let pauseIndex = "paused.collapsed.keys"
    }

    public static let scaleRange: ClosedRange<Double> = 0.8...2.4
    public static let scaleStep = 0.1

    public init(preferences: Preferences = MemoryPreferences(), defaultTextScale: Double = 1.0) {
        prefs = preferences
        disk = DiskModel(preferences: preferences)
        textScale = preferences.double(Keys.scale) ?? defaultTextScale
        muted = preferences.bool(Keys.mute) ?? false
        sidebarWidth = Self.clampSidebar(preferences.double(Keys.sidebarWidth) ?? Double(MasonTokens.sizeSidebarWidth))
        let box = MuteBox()
        announcer = Announcer(sink: SilentSink(), isMuted: { box.value }, preferences: preferences)
        muteBox = box
        box.value = muted
    }

    /// Lets the announcer read `muted` without holding the model.
    private let muteBox: MuteBox

    /// Called about once a second by the app: the lost-connection clock, and every few seconds
    /// a claim to speak, whose answer carries what the daemon wants said.
    public func pulse(now: Date = Date()) {
        muteBox.value = muted
        announcer.check(now: now)
        if daemonDown != announcer.daemonDown { daemonDown = announcer.daemonDown }
        if connected, now.timeIntervalSince(lastClaim) >= Self.claimEvery {
            lastClaim = now
            raw("POST", "/announce/claim", json: announcer.claimBody(client: clientId))
        }
    }
    @ObservationIgnored private var lastClaim = Date.distantPast
    static let claimEvery: TimeInterval = 3

    // MARK: navigation

    public var page: Page { history.current }
    public var canGoBack: Bool { history.canGoBack }
    public var canGoForward: Bool { history.canGoForward }

    /// Whether `n` is a task this monitor has heard of (in the sidebar, or named by the open
    /// task's page), so a `#n` in text can be a link to its page.
    public func isKnownTask(_ n: UInt64) -> Bool {
        isKnownTaskCalls += 1
        if let c = knownTaskCache, c.revision == linkRevision { return c.ids.contains(n) }
        var ids = Set<UInt64>()
        for sec in liveSidebar?.sections ?? [] { for r in sec.rows { ids.insert(r.id) } }
        if let d = taskDetail, d.hasRow {
            let r = d.row
            ids.insert(r.id)
            ids.formUnion(r.blockers)
            ids.formUnion(r.blocks)
            for debt in d.debts { ids.insert(debt.task) }
            if d.hasProposal { ids.insert(d.proposal.filedBy) }
        }
        knownTaskCache = (linkRevision, ids)
        return ids.contains(n)
    }

    /// `text` as plain text, task links and URL links, for the open task's page. The result is
    /// kept per text until the sidebar or the open task changes.
    public func linkify(_ text: String) -> [LinkSegment] {
        if linkCache.revision != linkRevision { linkCache = (linkRevision, [:]) }
        if let hit = linkCache.segments[text] { return hit }
        linkifyPasses += 1
        let made = Linkify.segments(text, isKnownTask: { [unowned self] in self.isKnownTask($0) },
                                    ciLinks: taskDetail?.ciLinks ?? [])
        if linkCache.segments.count > 512 { linkCache.segments.removeAll() }
        linkCache.segments[text] = made
        return made
    }

    /// Where the shown page is scrolled now. Not published: it changes every frame of a scroll,
    /// and is saved into the history only when the page is left.
    @ObservationIgnored public private(set) var liveScroll: Double = 0
    public func reportScroll(_ y: Double) { liveScroll = max(0, y) }

    private func navigate(_ f: (inout NavHistory, Double) -> Void) {
        let before = history.visit
        f(&history, liveScroll)
        if history.visit != before { liveScroll = history.restore }
        subscribe?()
    }

    public func go(_ p: Page) { navigate { $0.go(p, scroll: $1) } }
    public func goBack() { navigate { $0.goBack(scroll: $1) } }
    public func goForward() { navigate { $0.goForward(scroll: $1) } }
    public func goHome() { navigate { $0.goHome(scroll: $1) } }
    /// Pick the CPU graph's span and ask the daemon for that range.
    public func setCpuSpan(_ s: CpuSpan) { cpu.setSpan(s); subscribe?() }
    /// Pick the disk graph's span and ask the daemon for that range.
    public func setDiskSpan(_ s: DiskSpan) { disk.setSpan(s); subscribe?() }
    /// Show or hide a volume's line; remembered.
    public func toggleVolume(_ id: String) { disk.toggleVolume(id) }

    public func open(url: URL) {
        if let p = Page.from(url: url) { go(p) }
    }

    // MARK: text scale

    public func zoomIn() { textScale = min(Self.scaleRange.upperBound, (textScale + Self.scaleStep).rounded(toPlaces: 1)) }
    public func zoomOut() { textScale = max(Self.scaleRange.lowerBound, (textScale - Self.scaleStep).rounded(toPlaces: 1)) }
    public func zoomReset() { textScale = 1.0 }

    // MARK: counts and tiles

    /// The tile text: the daemon's label, or (older daemon) the state with spaces.
    public func tileLabel(_ c: StateCount) -> String {
        c.label.isEmpty ? c.state.replacingOccurrences(of: "_", with: " ") : c.label
    }

    /// A dash while disconnected: never a stale count, never a false zero.
    public func tileCount(_ c: StateCount) -> String {
        connected ? String(c.count) : "–"
    }

    /// The attention indicator is shown only while something needs a look.
    public var attention: UInt32 { connected ? ((heldHeader ?? header)?.attention ?? 0) : 0 }

    // MARK: sidebar width

    /// Text zoom stops scaling the panel here, as before.
    public static let sidebarScaleCap = 1.5

    public static func clampSidebar(_ w: Double) -> Double {
        guard w.isFinite else { return Double(MasonTokens.sizeSidebarWidth) }
        return min(Double(MasonTokens.sizeSidebarWidthMax), max(Double(MasonTokens.sizeSidebarWidthMin), w))
    }

    /// Sets and stores the unscaled width, clamped to the limits.
    public func setSidebarWidth(_ w: Double) {
        let c = Self.clampSidebar(w)
        guard c != sidebarWidth else { return }
        sidebarWidth = c
        prefs.setDouble(c, for: Keys.sidebarWidth)
    }

    public static func sidebarFactor(scale: Double) -> Double { min(scale, sidebarScaleCap) }

    /// The stored width as drawn at this text zoom.
    public func panelWidth(scale: Double) -> Double { sidebarWidth * Self.sidebarFactor(scale: scale) }

    /// The width to draw: the panel gives way first, so the main view keeps its minimum
    /// unless the panel is already at its (scaled) floor.
    public static func fit(desired: Double, available: Double, scale: Double) -> Double {
        let f = sidebarFactor(scale: scale)
        let floor = Double(MasonTokens.sizeSidebarWidthMin) * f
        let room = available - Double(MasonTokens.sizeMainWidthMin) - Double(MasonTokens.sizeSidebarHandle)
        return max(floor, min(desired, room))
    }

    // MARK: sidebar sections

    public func isExpanded(_ s: SidebarSection) -> Bool {
        if let o = prefs.bool(Keys.section(s.state)) { return o }
        if let o = overrides[s.state] { return o }
        return !s.collapsed
    }

    public func setExpanded(_ state: String, _ open: Bool) {
        overrides[state] = open
        prefs.setBool(open, for: Keys.section(state))
    }

    // MARK: paused banners

    private var collapsedPauseKeys: [String] {
        (prefs.string(Keys.pauseIndex) ?? "").split(separator: "\n").map(String.init)
    }

    /// Whether this pause's banner is collapsed to its icon (kept per device, across launches).
    public func isPauseCollapsed(project: String, mode: String) -> Bool {
        _ = prefsRevision  // observed: a change made by `setPauseCollapsed` redraws the readers
        return prefs.bool(Keys.pause(project: project, mode: mode)) ?? false
    }
    /// Bumped when a preference the views read through a method (not a property) changes.
    public internal(set) var prefsRevision = 0
    /// Bumped when the announcer's settings (kept in the announcer, not here) change.
    public private(set) var announcerRevision = 0
    public func announcerSettingsChanged() { announcerRevision &+= 1 }

    public func setPauseCollapsed(project: String, mode: String, _ collapsed: Bool) {
        let key = Keys.pause(project: project, mode: mode)
        var keys = collapsedPauseKeys.filter { $0 != key }
        if collapsed { keys.append(key) }
        prefs.setBool(collapsed, for: key)
        prefs.setString(keys.joined(separator: "\n"), for: Keys.pauseIndex)
        prefsRevision &+= 1
    }

    /// One row of the pause group: banner indices. `collapsed` icons sit at the leading edge of the
    /// row; `expanded` is the full banner in that row (nil: an icon-only row).
    public struct PauseRow: Equatable {
        public let collapsed: [Int]
        public let expanded: Int?
    }

    /// The pause group's rows, in banner order: collapsed icons ride in the first expanded pause's
    /// row, or share one icon-only row when every pause is collapsed.
    public func pauseRows() -> [PauseRow] {
        let all = (header?.banners ?? []).enumerated().filter { $0.element.kind == "paused" }
        let isCollapsed = { (b: Mason_V1_Banner) in self.isPauseCollapsed(project: b.project, mode: b.mode) }
        let collapsed = all.filter { isCollapsed($0.element) }.map(\.offset)
        let expanded = all.filter { !isCollapsed($0.element) }.map(\.offset)
        if expanded.isEmpty { return collapsed.isEmpty ? [] : [PauseRow(collapsed: collapsed, expanded: nil)] }
        return expanded.enumerated().map { PauseRow(collapsed: $0.offset == 0 ? collapsed : [], expanded: $0.element) }
    }

    /// A pause that is no longer in the header (resumed or expired) forgets its collapse, so a
    /// later pause on the same project and mode starts expanded and is noticed.
    private func prunePauseCollapse() {
        guard let h = header else { return }
        let live = Set(h.banners.filter { $0.kind == "paused" }.map { Keys.pause(project: $0.project, mode: $0.mode) })
        let keys = collapsedPauseKeys
        let kept = keys.filter { live.contains($0) }
        guard kept.count != keys.count else { return }
        for k in keys where !live.contains(k) { prefs.setBool(false, for: k) }
        prefs.setString(kept.joined(separator: "\n"), for: Keys.pauseIndex)
    }

    public func toggle(_ s: SidebarSection) { setExpanded(s.state, !isExpanded(s)) }

    /// Clicking a tile expands its sections (proposed: its imports too) and scrolls to the first
    /// that has tasks.
    public func reveal(state: String) {
        let secs = tileSections(state)
        for s in secs where s.state != state { setExpanded(s.state, true) }
        setExpanded(state, true)
        scrollTarget = secs.first { !$0.rows.isEmpty }?.state ?? state
    }

    /// What a tile click does. Every click reveals at once (never waits to tell
    /// a single click from a double); the second click of a double also opens
    /// the lone task.
    public func tileClicked(state: String, clickCount: Int) {
        reveal(state: state)
        if clickCount == 2 { tileDoubleClicked(state: state) }
    }

    /// A double click: the lone task when the stage holds exactly one, else the stage's page
    /// (for a state that has one). Nothing while disconnected.
    public func tileDoubleClicked(state: String) {
        guard connected else { return }
        let n = sectionRows(state).count
        if n == 0 && state == "failed" { return }
        if n == 1 { revealAndOpenIfOne(state: state) }
        else if let p = Page.sectionPage(state) { go(p) }
    }

    /// Reveal the state's section and, if it holds exactly one task right now
    /// (rows as they are at this moment, search ignored), open that task.
    /// Does nothing while disconnected.
    public func revealAndOpenIfOne(state: String) {
        guard connected else { return }
        reveal(state: state)
        let rows = sectionRows(state)
        guard rows.count == 1 else { return }
        go(.task(rows[0].id))
    }

    /// Whether the main view shows one of the tasks behind this tile: the page is a task page and
    /// its id is a row of the sidebar section with the tile's state.
    public func tileShowsCurrentTask(_ c: StateCount) -> Bool {
        guard case .task(let id) = page else { return false }
        return sectionRows(c.state).contains { $0.id == id }
    }

    /// The tile's tooltip; a count of exactly one says a double-click opens it.
    public func tileTooltip(_ c: StateCount) -> String {
        connected && c.count == 1 ? c.tooltip + "\nDouble-click to open it" : c.tooltip
    }

    // MARK: search

    public func filteredRows(_ s: SidebarSection) -> [TaskRow] {
        s.rows.filter { search.matches($0) }
    }

    public func visibleCount(_ s: SidebarSection) -> Int {
        search.isActive ? filteredRows(s).count : Int(s.count)
    }

    /// The daemon sends only the newest rows of a long history section; `count` stays the true total.
    public func isCapped(_ s: SidebarSection) -> Bool { Int(s.count) > s.rows.count }

    /// The number in a section header: "500 of 2000" when capped; while searching, "3 in newest 500".
    public func countText(_ s: SidebarSection) -> String {
        if !isCapped(s) { return String(visibleCount(s)) }
        if search.isActive { return "\(filteredRows(s).count) in newest \(s.rows.count)" }
        return "\(s.rows.count) of \(s.count)"
    }

    // MARK: commands

    /// Who is acting. Only the owner's identity counts as an answer (Part 10),
    /// so every command says who sent it: `MASON_AS`, else the macOS user name.
    @ObservationIgnored public var identity: String = ProcessInfo.processInfo.environment["MASON_AS"] ?? NSUserName()

    public func send(_ c: Mason_V1_Command) {
        if let why = readOnlyReason {
            lastResult = "Read-only: \(why)"
            return
        }
        var c = c
        if c.actor.isEmpty { c.actor = identity }
        // Recorded before the write, so the pressed control redraws at once; a second click
        // while one is in flight sends nothing.
        guard beginPending(&c) else { return }
        sender?(c)
    }

    private func command(_ build: (inout Mason_V1_Command) -> Void) {
        var c = Mason_V1_Command()
        build(&c)
        send(c)
    }

    public func cancel(_ task: UInt64) { command { $0.cancel = .with { $0.task = task } } }
    public func retry(_ task: UInt64, from: String = "", seen: UInt64 = 0) { command { $0.retry = .with { $0.task = task; $0.from = from; $0.seen = seen } } }
    public func approve(_ task: UInt64, priority: String = "") { command { $0.approve = .with { $0.task = task; $0.priority = priority } } }
    /// Put a proposed task aside; it comes back only by hand.
    public func deferTask(_ task: UInt64) { command { $0.defer = .with { $0.task = task } } }
    /// Move a deferred task back to proposed.
    public func undefer(_ task: UInt64) { command { $0.undefer = .with { $0.task = task } } }
    /// The priorities the approval picker offers; it starts at `defaultApprovalPriority`.
    nonisolated public static let approvalPriorities = ["urgent", "high", "normal", "low"]
    nonisolated public static let defaultApprovalPriority = "normal"
    /// The tasks the Add blocker picker offers for `row`: any task in the sidebar that is not
    /// itself, not already a blocker, and could still land (not landed, canceled or failed).
    public func blockerCandidates(for row: TaskRow) -> [TaskRow] {
        var seen = Set<UInt64>()
        var out: [TaskRow] = []
        for s in sidebar?.sections ?? [] {
            for r in s.rows where r.id != row.id && !row.blockers.contains(r.id)
                && !["landed", "canceled", "failed"].contains(r.state) && seen.insert(r.id).inserted {
                out.append(r)
            }
        }
        return out.sorted { $0.id < $1.id }
    }
    /// Release a task from one blocker (owner only; the daemon refuses anyone else).
    public func release(_ task: UInt64, blocker: UInt64) {
        command { $0.unblock = .with { $0.task = task; $0.from = [blocker] } }
    }
    /// Make a task wait for another one as well (owner only).
    public func addBlocker(_ task: UInt64, after: UInt64) {
        command { $0.block = .with { $0.task = task; $0.after = [after] } }
    }
    /// Put a canceled task whose branch was kept back in the queue (owner only).
    public func revive(_ task: UInt64) { command { $0.revive = .with { $0.task = task } } }
    public func reject(_ task: UInt64, reason: String = "") { command { $0.reject = .with { $0.task = task; $0.reason = reason } } }
    public func exploreAnyway(_ task: UInt64) { command { $0.exploreAnyway = .with { $0.task = task } } }
    public func landNow(_ task: UInt64) { command { $0.landNow = .with { $0.task = task } } }
    /// Land now for a whole running batch: every member lands past its remaining tests, which
    /// run on the base after the merge.
    public func landNowBatch(_ batch: UInt64) { command { $0.landNowBatch = .with { $0.batch = batch } } }
    public func force(_ task: UInt64) { command { $0.force = .with { $0.task = task } } }
    public func skipTest(_ task: UInt64, step: String) {
        command { $0.skip = .with { $0.task = task; $0.step = step } }
    }
    /// Make a task immune to testing (a fast track): no test runs for it from develop's checks
    /// through landing, and the full run is owed as a debt.
    public func makeTestImmune(_ task: UInt64) {
        command { $0.skip = .with { $0.task = task; $0.allTests = true } }
    }
    public func answer(task: UInt64, _ text: String, cardHash: String) {
        command { $0.answer = .with { $0.task = task; $0.answer = text; $0.cardHash = cardHash } }
    }
    /// Approve a held self-change: the exact diff the owner was shown.
    public func approveHold(_ task: UInt64, diffHash: String) {
        command { $0.approveHold = .with { $0.task = task; $0.diffHash = diffHash } }
    }

    /// What Approve sends for an approval: the held diff's hash when there is one, else a plain approve.
    public func approve(_ task: UInt64, _ a: Mason_V1_Approval, priority: String = MonitorModel.defaultApprovalPriority) {
        if a.diffHash.isEmpty { approve(task, priority: priority) } else { approveHold(task, diffHash: a.diffHash) }
    }

    /// The first task that needs the owner, in the Home queue's order.
    public var firstNeedingYou: UInt64? { connected ? home?.attention.first?.task.id : nil }

    /// What clicking the attention count does: open the first task that needs the owner.
    public func openFirstNeedingYou() {
        if let t = firstNeedingYou { go(.task(t)) } else { goHome() }
    }

    /// MASON's own repository's project name, when one is registered ("Improve the pipeline" files there).
    public var selfProject: String { header?.selfProject ?? "" }

    public func addTask(project: String, title: String, body: String) {
        command { $0.addTask = .with { $0.project = project; $0.title = title; $0.body = body } }
    }
    public func setSessionLimit(_ units: UInt32) {
        let name = usage?.providers.first?.name ?? "claude"
        command { $0.setLimit = .with { $0.resource = "llm:\(name)"; $0.units = units } }
    }
    /// The session stepper's minus (`by: -1`) and plus (`by: 1`), the one rule for the Mac and the
    /// phone: the limit moves by `delta` and never goes below 0.
    public func stepSessionLimit(by delta: Int) {
        let limit = Int(home?.sessionsLimit ?? 0)
        setSessionLimit(UInt32(max(0, limit + delta)))
    }
    /// How many compiles may run at once (the daemon refuses 0).
    public func setCompileLimit(_ units: UInt32) {
        command { $0.setLimit = .with { $0.resource = "compile"; $0.units = max(1, units) } }
    }
    /// The CPU gate's range in the monitor: a further compile starts below this percent.
    public static let cpuGateRange = 10...100
    /// The most seconds the settle stepper asks for.
    public static let settleMax = 600
    /// Start another compile only below `pct` percent CPU (kept to 10-100; the daemon holds 1-100
    /// and refuses the rest, and says so in `lastResult`).
    public func setCompileCpuGate(_ pct: Int) {
        let v = min(max(pct, Self.cpuGateRange.lowerBound), Self.cpuGateRange.upperBound)
        command { $0.setGate = .with { $0.name = "compile_cpu_max"; $0.value = Int64(v) } }
    }
    /// Seconds after one compile starts before the next may (0: no wait).
    public func setCompileSettle(_ seconds: Int) {
        let v = min(max(seconds, 0), Self.settleMax)
        command { $0.setGate = .with { $0.name = "compile_settle_s"; $0.value = Int64(v) } }
    }
    public func pause(mode: String, seconds: UInt64) {
        command { $0.pause = .with { $0.project = project; $0.mode = mode; $0.forSeconds = seconds } }
    }
    public func resume() { command { $0.resume = .with { $0.project = project } } }
    /// Lift exactly one pause: its project (empty for everything) and its mode.
    public func resumePause(project: String, mode: String) {
        command { $0.resume = .with { $0.project = project; $0.mode = mode } }
    }
    /// Change a proposal's title, body or evidence while it waits for the owner (nil: unchanged).
    public func editProposal(_ task: UInt64, title: String?, body: String?, evidence: String?) {
        var o: [String: String] = [:]
        if let title { o["title"] = title }
        if let body { o["body"] = body }
        if let evidence { o["evidence"] = evidence }
        guard let data = try? JSONSerialization.data(withJSONObject: o), let text = String(data: data, encoding: .utf8) else { return }
        raw("POST", "/proposals/\(task)/edit", json: text)
    }
    /// "Improve the pipeline": ask MASON to change its own code. The daemon files it in MASON's own project.
    public func improve(project: String, text: String) {
        guard let data = try? JSONSerialization.data(withJSONObject: ["text": text]), let body = String(data: data, encoding: .utf8) else { return }
        raw("POST", "/projects/\(project)/improve", json: body)
    }
    /// Show the redacted version that would go upstream (`confirm: false`), or send it (`true`).
    public func publish(_ task: UInt64, confirm: Bool) {
        var c = Mason_V1_Command()
        c.id = "publish:\(task)"
        c.raw = .with { $0.method = "POST"; $0.path = "/proposals/\(task)/publish"; $0.json = "{\"confirm\":\(confirm)}" }
        send(c)
    }
    /// Open session: ask the daemon about the task's session first (`handle(result:)` carries on).
    public func openSession(_ task: UInt64) {
        sessionNotice = nil
        var c = Mason_V1_Command()
        c.id = "session:\(task)"
        c.raw = .with { $0.method = "GET"; $0.path = "/tasks/\(task)/session" }
        send(c)
    }

    public func answerSessionPrompt(_ yes: Bool) {
        guard let task = sessionPrompt else { return }
        sessionPrompt = nil
        if yes { launchSession(task, anyway: true) }
    }

    func launchSession(_ task: UInt64, anyway: Bool) {
        let argv = OpenSession.argv(cli: cliPath, task: task, anyway: anyway)
        if launcher?(argv) == true { return }
        let line = OpenSession.commandLine(cli: cliPath, task: task, anyway: anyway)
        copier?(line)
        sessionNotice = OpenSession.copiedNotice(line)
    }

    /// The project a `push_auth` banner is about: its text starts "<project>: ".
    public static func pushAuthProject(_ b: Mason_V1_Banner) -> String {
        String(b.text.split(separator: ":", maxSplits: 1, omittingEmptySubsequences: false).first ?? "")
    }

    /// Open the key sheet for `project` and ask the daemon which keys it could use.
    public func chooseSSHKey(project: String) {
        sshKeyProject = project
        sshKeys = nil
        sshKeyResult = nil
        var c = Mason_V1_Command()
        c.id = "ssh-keys:\(project)"
        c.raw = .with { $0.method = "GET"; $0.path = "/projects/\(project)/ssh-keys" }
        send(c)
    }

    /// Use this key for the project's remote. A key with a passphrase asks for it in a dialog on
    /// the daemon's Mac; the passphrase never passes through here.
    public func useSSHKey(project: String, path: String) {
        sshKeyBusy = true
        sshKeyResult = nil
        var c = Mason_V1_Command()
        c.id = "ssh-key:\(project)"
        c.raw = .with { $0.method = "POST"; $0.path = "/projects/\(project)/ssh-key"; $0.json = "{\"path\":\(Self.jsonString(path))}" }
        send(c)
    }

    /// Open the dialog for a row's command (nothing when the row has none).
    public func openCommand(label: String, command: String) {
        if let d = Format.commandDialog(label: label, command: command) { commandDialog = d }
    }

    public func closeCommand() { commandDialog = nil }

    /// Open the owner-session dialog on a session (the health banner's and the sidebar's button).
    public func openOwnerSession(_ id: UInt64) {
        ownerSessionDialog = id
        ownerSession = nil
        ownerSessionError = nil
        refreshOwnerSession()
    }

    public func closeOwnerSession() {
        ownerSessionDialog = nil
        ownerSession = nil
        ownerSessionError = nil
    }

    func refreshOwnerSession() {
        guard let id = ownerSessionDialog else { return }
        var c = Mason_V1_Command()
        c.id = "osession:\(id)"
        c.raw = .with { $0.method = "GET"; $0.path = "/sessions/\(id)" }
        send(c)
    }

    public func sendOwnerMessage(_ text: String) {
        guard let id = ownerSessionDialog, !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return }
        var c = Mason_V1_Command()
        c.id = "osession-send:\(id)"
        c.raw = .with { $0.method = "POST"; $0.path = "/sessions/\(id)/messages"; $0.json = "{\"text\":\(Self.jsonString(text))}" }
        send(c)
    }

    public func endOwnerSession() {
        guard let id = ownerSessionDialog else { return }
        var c = Mason_V1_Command()
        c.id = "osession-end:\(id)"
        c.raw = .with { $0.method = "POST"; $0.path = "/sessions/\(id)/end"; $0.json = "{}" }
        send(c)
    }

    public func closeSSHKeys() {
        sshKeyProject = nil
        sshKeys = nil
        sshKeyResult = nil
        sshKeyBusy = false
    }

    static func jsonString(_ s: String) -> String {
        guard let d = try? JSONSerialization.data(withJSONObject: [s]), let t = String(data: d, encoding: .utf8) else { return "\"\"" }
        return String(t.dropFirst().dropLast())
    }

    /// Ask for the Settings page's data: the project's settings and the notification routes.
    public func loadSettings() {
        let p = editorProject
        if !p.isEmpty { raw("GET", SettingsPayload.settingsPath(project: p), id: "settings") }
        raw("GET", "/notify/settings", id: "notify-settings")
        raw("GET", "/remote", id: "remote")
    }
    /// Change the global placement mode; the page reloads `/remote` on the answer.
    public func changeGlobalMode(_ mode: String) {
        settingsNotice = nil
        raw("POST", "/remote", json: SettingsPayload.remoteMode(mode), id: "remote-save")
    }
    /// Change the selected project's placement mode.
    public func changeProjectMode(_ mode: String) {
        settingsNotice = nil
        raw("POST", "/remote", json: SettingsPayload.remoteMode(mode, project: editorProject), id: "remote-save")
    }
    /// Change one project setting (a body from `SettingsPayload`); the page reloads on the answer.
    public func changeProjectSetting(_ body: String) {
        settingsNotice = nil
        raw("POST", SettingsPayload.settingsPath(project: editorProject), json: body, id: "settings-save")
    }
    public func changeNotifySetting(_ body: String) {
        settingsNotice = nil
        raw("POST", "/notify/settings", json: body, id: "notify-save")
    }
    public func testNotification(channel: String) {
        settingsNotice = nil
        raw("POST", "/notify/test", json: SettingsPayload.testNotification(channel: channel), id: "notify-test")
    }
    public func runSchedule(_ name: String) {
        settingsNotice = nil
        raw("POST", "/schedules/run", json: SettingsPayload.runSchedule(project: editorProject, name: name), id: "schedule-run")
    }

    /// Ask for the LAN state and the paired devices.
    public func loadDevices() {
        raw("GET", "/lan", id: "lan")
        raw("GET", "/devices", id: "devices")
    }
    /// Make a pairing code for a new device; `role` is "owner" or "viewer".
    public func issuePairCode(role: String) {
        devicesNotice = nil
        raw("POST", "/pair/codes", json: "{\"role\":\"\(role)\"}", id: "pair-code")
    }
    public func closePairing() { pairOffer = nil }
    public func revokeDevice(_ id: UInt64) {
        devicesNotice = nil
        raw("POST", "/devices/\(id)/revoke", json: "{}", id: "device-revoke:\(id)")
    }

    // MARK: Add Project (the daemon's /onboarding endpoints)

    /// Add Project is offered on the desktop monitor only, to the owner, never read-only.
    public var canAddProject: Bool { canAct }

    /// Open the wizard and ask for the candidate repositories.
    public func openAddProject() {
        guard canAddProject else { return }
        addProject = AddProjectWizard()
        raw("GET", "/onboarding/candidates", id: "onb-candidates")
    }

    public func closeAddProject() { addProject = nil }

    /// Ask what the daemon sees at the wizard's path, with the answers so far.
    public func planAddProject() {
        guard var w = addProject, w.canPlan || w.fillingRest else { return }
        w.busy = true
        addProject = w
        raw("POST", "/onboarding/plan", json: w.body(forApply: false), id: "onb-plan")
    }

    /// Answer the current question and ask for the next.
    public func answerAddProject(_ value: String) {
        addProject?.answer(value)
        planAddProject()
    }

    public func addProjectUseRecommendedForRest() {
        addProject?.useRecommendedForRest()
        planAddProject()
    }

    public func applyAddProject() {
        guard var w = addProject, !w.busy else { return }
        w.startApply()
        addProject = w
        raw("POST", "/onboarding/apply", json: w.body(forApply: true), id: "onb-apply")
    }

    /// Done: select the project that was added (the picker names it once the header lists it).
    public func finishAddProject(openProofTask: Bool) {
        guard let w = addProject else { return }
        if let id = w.appliedId { project = id; subscribe?() }
        if openProofTask, let t = w.proofTask { go(.task(t)) }
        addProject = nil
    }

    func handleOnboarding(_ r: Mason_V1_CommandResult) {
        guard addProject != nil else { return }
        switch r.id {
        case "onb-candidates":
            if r.ok {
                let l = OnboardingCandidate.list(json: r.json)
                addProject?.candidates = l.candidates
                addProject?.candidateErrors = l.errors
            } else { addProject?.refusal = r.error }
        case "onb-plan":
            if r.ok, let p = OnboardingPlan(json: r.json) {
                if addProject?.received(plan: p) == true { planAddProject() }
            } else { addProject?.planRefused(r.ok ? "The daemon sent a plan this monitor could not read." : r.error) }
        case "onb-apply":
            addProject?.applied(ok: r.ok, json: r.json, error: r.error)
            if r.ok, let id = addProject?.appliedId { project = id; subscribe?() }
        default: break
        }
    }

    public func raw(_ method: String, _ path: String, json: String = "", id: String = "") {
        command {
            if !id.isEmpty { $0.id = id }
            $0.raw = .with { $0.method = method; $0.path = path; $0.json = json }
        }
    }

    // MARK: the pipeline editor (Part 18)

    /// The project the editor is about: the one picked, else the first there is.
    public var editorProject: String { project.isEmpty ? (header?.projects.first ?? "") : project }

    public func selectStep(_ name: String) {
        editorStep = name
        subscribe?()
    }

    /// One operation on the draft, as the daemon's editor API takes it.
    public func editorOp(_ op: [String: Any]) {
        guard let data = try? JSONSerialization.data(withJSONObject: op), let text = String(data: data, encoding: .utf8) else { return }
        raw("POST", "/projects/\(editorProject)/editor/op", json: text)
    }

    /// `undo`, `redo`, `discard`, `simulate` or `apply`.
    public func editorCall(_ what: String, _ body: [String: Any] = [:]) {
        let data = (try? JSONSerialization.data(withJSONObject: body)) ?? Data("{}".utf8)
        raw("POST", "/projects/\(editorProject)/editor/\(what)", json: String(data: data, encoding: .utf8) ?? "{}")
    }

    // MARK: live progress

    public static let logKeep = 40

    /// The daemon's time now, ms since the epoch, by this device's clock and the offset.
    public func daemonNow(_ local: Date = Date()) -> UInt64 { live.daemonNow(local) }

    func anchor(daemonNow ms: UInt64, local: Date = Date()) {
        guard ms > 0 else { return }
        clockOffsetMs = Int64(ms) - Int64(local.timeIntervalSince1970 * 1000)
    }

    /// How progress messages are put off to the next frame: `flush` is run once, later, on the main actor.
    /// The app's default waits one frame (1/30 s); tests replace it to hold the model "busy".
    @ObservationIgnored public var scheduleProgressFlush: (@escaping @MainActor () -> Void) -> Void = { flush in
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.0 / 30) { MainActor.assumeIsolated { flush() } }
    }
    @ObservationIgnored private var pendingProgress: [Mason_V1_StepProgress] = []
    @ObservationIgnored private var flushScheduled = false

    // MARK: visibility (task #973)

    /// Whether the window can be seen. While it cannot, `header` and `sidebar` are held in
    /// `held*` (side effects still run), progress is held, and the subscription names only those
    /// two views. Becoming visible publishes the held values in one go and resubscribes.
    @ObservationIgnored public var visible = true {
        didSet {
            guard visible != oldValue else { return }
            if visible { publishHeld() }
            subscribe?()
        }
    }
    @ObservationIgnored var heldHeader: HeaderView?
    @ObservationIgnored var heldSidebar: SidebarView?
    /// Newest progress message per task and step, held while hidden.
    @ObservationIgnored private var heldProgress: [UInt64: [String: Mason_V1_StepProgress]] = [:]
    @ObservationIgnored private var heldProgressOrder: [Mason_V1_StepProgress] = []

    /// The newest sidebar, published or held: what logic (not drawing) must read.
    var liveSidebar: SidebarView? { heldSidebar ?? sidebar }

    private func publishHeld() {
        if let h = heldHeader { heldHeader = nil; if header != h { header = h } }
        if let x = heldSidebar { heldSidebar = nil; if sidebar != x { sidebar = x } }
        if !heldProgressOrder.isEmpty {
            let batch = heldProgressOrder
            heldProgressOrder = []
            heldProgress = [:]
            apply(progresses: batch)
        }
    }

    /// A progress message from the stream: held until the next frame, so a burst costs one sidebar publish.
    func enqueue(progress p: Mason_V1_StepProgress) {
        if !visible {
            holdProgress(p)
            return
        }
        pendingProgress.append(p)
        guard !flushScheduled else { return }
        flushScheduled = true
        scheduleProgressFlush { [weak self] in self?.flushProgress() }
    }

    private func holdProgress(_ p: Mason_V1_StepProgress) {
        guard p.hasProgress else {
            if p.nowMs > 0 { anchor(daemonNow: p.nowMs) }
            return
        }
        // Only the newest message for each task and step is kept.
        let step = p.progress.step
        if let old = heldProgress[p.task]?[step], let i = heldProgressOrder.firstIndex(where: { $0.task == old.task && $0.progress.step == step }) {
            heldProgressOrder.remove(at: i)
        }
        heldProgress[p.task, default: [:]][step] = p
        heldProgressOrder.append(p)
        if p.nowMs > 0 { anchor(daemonNow: p.nowMs) }
    }

    /// Apply what was held, in order, as one batch. Every other message flushes first, so order is kept.
    public func flushProgress() {
        flushScheduled = false
        guard !pendingProgress.isEmpty else { return }
        let batch = pendingProgress
        pendingProgress = []
        apply(progresses: batch)
    }

    /// A step's progress arrived without a view: put it into the task page and the rows, and follow the log.
    public func apply(progress p: Mason_V1_StepProgress, local: Date = Date()) {
        apply(progresses: [p], local: local)
    }

    /// Several progress messages at once: one write to the task page, one to the sidebar, one pass over its rows.
    func apply(progresses ps: [Mason_V1_StepProgress], local: Date = Date()) {
        if let last = ps.last(where: { $0.nowMs > 0 }) { anchor(daemonNow: last.nowMs, local: local) }
        let withCell = ps.filter { $0.hasProgress }
        guard !withCell.isEmpty else { return }
        if let d = taskDetail, d.hasRow {
            var cells = progress.detail.cells ?? d.row.progress
            var changed = false
            var lines: [String] = []
            for p in withCell where d.row.id == p.task {
                if let next = ProgressStore.merged(cells, p.progress) {
                    cells = next
                    changed = true
                }
                lines += p.log
            }
            if changed { progress.detail.cells = cells }
            appendLog(lines)
        }
        var latest: [UInt64: [Mason_V1_ProgressCell]] = [:]
        for p in withCell { latest[p.task, default: []].append(p.progress) }
        // Base cells: the live entry if there is one, else the row of the last sidebar (read, never copied).
        var base: [UInt64: [ProgressCell]] = [:]
        for id in latest.keys { if let c = progress.cells(id) { base[id] = c } }
        if base.count < latest.count, let s = liveSidebar {
            for sec in s.sections {
                for r in sec.rows where latest[r.id] != nil && base[r.id] == nil { base[r.id] = r.progress }
            }
        }
        for (id, cells) in latest {
            guard var current = base[id] else { continue }
            var hit = false
            for cell in cells {
                if let next = ProgressStore.merged(current, cell) { current = next; hit = true }
            }
            if hit { progress.entry(id).cells = current }
        }
    }

    func appendLog(_ lines: [String]) {
        guard !lines.isEmpty else { return }
        liveLog = Array((liveLog + lines).suffix(Self.logKeep))
    }

    // MARK: updates from the daemon

    public func apply(_ m: Mason_V1_ServerMessage) {
        if case .stepProgress(let p)? = m.body {
            enqueue(progress: p)
            return
        }
        flushProgress()
        switch m.body {
        case .welcome(let w)?:
            if !connected { connected = true }
            if reconnecting { reconnecting = false }
            announcer.connectionRestored()
            if !pending.isEmpty { pending = [:] }
            if w.resume == .snapshot, lastSeq != 0 { lastSeq = 0 }
            if w.resume == .snapshot { lastViewSeq = 0 }
            if daemonVersion != w.version { daemonVersion = w.version }
            if localDaemon != w.role.isEmpty { localDaemon = w.role.isEmpty }
            // A build asked for on the connection that just went has no answer coming.
            if !monitorAsked { updating = nil }
            anchor(daemonNow: w.nowMs)
            if isOwner != w.owner { isOwner = w.owner }
            let why: String? = w.minProtocol > Self.protocolVersion
                ? "This monitor speaks protocol \(Self.protocolVersion), but the daemon needs \(w.minProtocol) or newer. Update the monitor; until then it can only look."
                : nil
            if readOnlyReason != why { readOnlyReason = why }
        case .snapshot(let s)?:
            setSeq(max(lastSeq, s.seq))
            lastViewSeq = max(lastViewSeq, s.seq)
            s.views.forEach(apply)
        case .events(let e)?:
            if let last = e.events.last?.seq { setSeq(max(lastSeq, last)) }
        case .view(let u)?:
            setSeq(max(lastSeq, u.seq))
            lastViewSeq = max(lastViewSeq, u.seq)
            apply(u.view)
        case .result(let r)?:
            let shown: String? = r.ok ? nil : r.error
            if lastResult != shown { lastResult = shown }
            settle(result: r)
            handle(result: r)
            onResult?(r)
        default:
            break
        }
    }

    /// Chime once when a task that needs the owner appears that was not there before.
    func noteAttention(_ s: Mason_V1_SidebarView) {
        let wanting: Set<String> = ["needs_attention", "failed", "proposed"]
        let ids = Set(s.sections.filter { wanting.contains($0.state) }.flatMap { $0.rows.map(\.id) })
        defer { attentionSeen = ids }
        guard let seen = attentionSeen, !muted, !announcer.heardFromDaemon else { return }
        if !ids.subtracting(seen).isEmpty { onAttention?() }
    }

    /// The project a view was computed for, when it is one of the project-scoped views.
    static func scope(of v: Mason_V1_View) -> String? {
        switch v.body {
        case .header(let x)?: return x.project
        case .sidebar(let x)?: return x.project
        case .home(let x)?: return x.project
        case .landing(let x)?: return x.project
        default: return nil
        }
    }

    /// Writes that carry the same value are skipped: an `@Observable` property notifies on every
    /// assignment, so an equal one would redraw every view that read it.
    private func setSeq(_ n: UInt64) { if lastSeq != n { lastSeq = n } }

    func apply(_ v: Mason_V1_View) {
        // A view computed for another filter (a rebuild that was in flight when the picker
        // changed, or one resumed after a reconnect) is dropped.
        if let p = Self.scope(of: v), p != project { return }
        switch v.body {
        case .header(let x)?:
            if visible {
                heldHeader = nil
                if header != x { header = x }
            } else {
                heldHeader = x
            }
            noteMonitorBuild()
            // An open dialog follows the session: a reply arrives as a new header.
            if ownerSessionDialog != nil { refreshOwnerSession() }
        case .sidebar(let x)?:
            progress.reset(tasks: x.sections.flatMap { $0.rows.map(\.id) })
            if visible {
                heldSidebar = nil
                if sidebar != x { sidebar = x }
            } else {
                heldSidebar = x
            }
            noteAttention(x)
            settleAfterView()
        case .home(let x)?:
            if home != x { home = x }
            cpu.update(x)
        case .cpuHistory(let x)?:
            cpu.update(history: x)
        case .diskHistory(let x)?:
            disk.update(history: x)
        case .taskDetail(let x)?:
            // A different task's page starts its log over; the same task's view carries the tail again.
            progress.resetDetail()
            if taskDetail != x { taskDetail = x }
            var lines = x.logTail.split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
            if lines.last == "" { lines.removeLast() }
            lines = Array(lines.suffix(Self.logKeep))
            if liveLog != lines { liveLog = lines }
            settleAfterView()
        case .landing(let x)?: if landing != x { landing = x }
        case .usage(let x)?: if usage != x { usage = x }
        case .editor(let x)?: if editor != x { editor = x }
        default: break
        }
    }

    static func settingsDone(_ id: String) -> String {
        switch id {
        case "notify-test": return "Test notification sent."
        case "schedule-run": return "Started."
        default: return "Saved. It applies now, with no restart."
        }
    }

    func handle(result r: Mason_V1_CommandResult) {
        if r.id.hasPrefix("onb-") { handleOnboarding(r) }
        if r.id.hasPrefix("publish:"), r.ok { publishPreview = PublishPreview(json: r.json) }
        if r.id.hasPrefix("session:"), let task = UInt64(r.id.dropFirst("session:".count)) {
            switch OpenSession.decide(ok: r.ok, json: r.json, error: r.error) {
            case .confirm: sessionPrompt = task
            case .launch(let anyway): launchSession(task, anyway: anyway)
            case .failed(let why): sessionNotice = why
            }
        }
        if r.id.hasPrefix("osession:"), r.ok { ownerSession = OwnerSessionTranscript(json: r.json) }
        if r.id.hasPrefix("osession:") || r.id.hasPrefix("osession-send:"), !r.ok { ownerSessionError = r.error }
        if r.id.hasPrefix("osession-send:"), r.ok { refreshOwnerSession() }
        if r.id.hasPrefix("osession-end:"), r.ok { closeOwnerSession() }
        if r.id.hasPrefix("ssh-keys:"), r.ok { sshKeys = SSHKeyList(json: r.json) }
        if r.id.hasPrefix("ssh-keys:"), !r.ok { sshKeyResult = r.error }
        if r.id.hasPrefix("ssh-key:") {
            sshKeyBusy = false
            if r.ok { sshKeyResult = "The remote accepted the key. Landing is released."; sshKeyProject = nil }
            else { sshKeyResult = r.error }
        }
        if r.id == "announce", r.ok { announcer.handle(claim: r.json) }
        if r.id == "update-daemon", !r.ok { updateError = r.error }
        if r.id == "update-monitor" {
            if r.ok { monitorAsked = true } else { monitorUpdateFailed(r.error) }
        }
        if r.id == "health", r.ok { health = HealthSnapshot(json: r.json) }
        if r.id == "introspection", r.ok { introspection = IntrospectionSnapshot(json: r.json) }
        if r.id == "settings", r.ok { settings = SettingsSnapshot(json: r.json) }
        if r.id == "lan", r.ok { lanStatus = LanStatus(json: r.json) }
        if r.id == "devices", r.ok { devices = DeviceList(json: r.json) }
        if r.id == "pair-code" {
            if r.ok {
                pairOffer = PairOffer(json: r.json, nowMs: UInt64(Date().timeIntervalSince1970 * 1000))
                if pairOffer == nil { devicesNotice = "The daemon sent a pairing code this monitor could not read." }
            } else { devicesNotice = r.error }
        }
        if r.id.hasPrefix("device-revoke:") {
            if r.ok { devicesNotice = "Revoked."; loadDevices() } else { devicesNotice = r.error }
        }
        if r.id == "notify-settings", r.ok { notifySettings = NotifySettings(json: r.json) }
        if r.id == "remote", r.ok { remoteSettings = RemoteSettings(json: r.json) }
        if r.id == "node-self", r.ok { nodeSelf = NodeSelf(json: r.json) }
        if r.id == "nodes", r.ok { nodes = NodesSnapshot(json: r.json) }
        if r.id == "node-accept" || r.id == "nodes-use" {
            if r.ok { loadNodes() } else { settingsNotice = r.error }
        }
        if r.id == "remote-save" {
            if r.ok { settingsNotice = Self.settingsDone(r.id); raw("GET", "/remote", id: "remote") } else { settingsNotice = r.error }
        }
        if ["settings-save", "notify-save", "notify-test", "schedule-run"].contains(r.id) {
            settingsNotice = r.ok ? Self.settingsDone(r.id) : r.error
            if r.ok, r.id == "settings-save" || r.id == "notify-save" { loadSettings() }
        }
    }

    public func disconnected(now: Date = Date()) {
        if connected { connected = false }
        if !reconnecting { reconnecting = true }
        markPendingNotSent()
        announcer.connectionLost(at: now)
    }
}

extension Double {
    func rounded(toPlaces n: Int) -> Double {
        let p = pow(10.0, Double(n))
        return (self * p).rounded() / p
    }
}

extension Mason_V1_TaskRow {
    /// How long the task has been in its state at daemon time `now` (ms): from `stateSinceMs`,
    /// or the daemon's own figure (`inStateMs`) when it did not send one.
    public func inState(now: UInt64) -> UInt64 {
        if stateSinceMs == 0 { return inStateMs }
        return now > stateSinceMs ? now - stateSinceMs : 0
    }
}

/// The search box and its three checkboxes: name, title, body.
public struct SearchState: Equatable, Sendable {
    public var text = ""
    public var inName = true
    public var inTitle = true
    public var inBody = false

    public init() {}

    public var isActive: Bool { !text.trimmingCharacters(in: .whitespaces).isEmpty }

    /// Whether the daemon must send history rows with their bodies: only while a body search runs.
    public var wantsRowBodies: Bool { inBody && isActive }

    public func matches(_ r: TaskRow) -> Bool {
        let q = text.trimmingCharacters(in: .whitespaces).lowercased()
        if q.isEmpty { return true }
        if inName && (r.project.lowercased().contains(q) || r.key.lowercased().contains(q)) { return true }
        if inTitle && r.title.lowercased().contains(q) { return true }
        if inBody && r.body.lowercased().contains(q) { return true }
        return false
    }
}

/// The Health page: resources and who holds them, from the daemon's own report.
public struct HealthSnapshot: Equatable, Sendable {
    public struct Resource: Equatable, Sendable {
        public var name: String
        public var kind: String
        public var used: Int
        public var limit: Int
    }
    public var resources: [Resource] = []
    public var waiting: [String] = []
    public var holders: [String] = []
    /// Overall CPU use now, if the daemon could read it.
    public var cpuBusy: Int? = nil
    /// A compile starts only while CPU use is below this percent.
    public var compileCpuMax: Int = 50
    /// Where the reading comes from (`mach`, `proc`, `top`, `loadavg`, `assumed-busy`); empty if not said.
    public var cpuSource: String = ""
    /// syspolicyd's CPU use and the limit on it, when the daemon gates on it.
    public var syspolicydPct: Int? = nil
    public var syspolicydMaxPct: Int = 0

    /// The syspolicyd line of the Health page's CPU section; nil with no gate.
    public var policyLine: String? { policyReadingLine(busy: syspolicydPct, max: syspolicydMaxPct) }

    /// The CPU line of the Health page.
    public var cpuLine: String {
        cpuReadingLine(busy: cpuBusy, source: cpuSource, gate: "a compile starts below \(compileCpuMax)%")
    }

    public init(json: String) {
        guard let data = json.data(using: .utf8),
              let v = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return }
        resources = (v["resources"] as? [[String: Any]] ?? []).map {
            Resource(
                name: $0["name"] as? String ?? "?",
                kind: $0["kind"] as? String ?? "?",
                used: $0["used"] as? Int ?? 0,
                limit: $0["limit"] as? Int ?? 0
            )
        }
        waiting = (v["waiting"] as? [[String: Any]] ?? []).map {
            "\($0["label"] as? String ?? "?"): \($0["explain"] as? String ?? "")"
        }
        holders = (v["holders"] as? [[String: Any]] ?? []).map { $0["label"] as? String ?? "?" }
        if let c = v["cpu"] as? [String: Any] {
            cpuBusy = c["busy_pct"] as? Int
            compileCpuMax = c["compile_max_pct"] as? Int ?? 50
            cpuSource = c["source"] as? String ?? ""
            syspolicydPct = c["syspolicyd_pct"] as? Int
            syspolicydMaxPct = c["syspolicyd_max_pct"] as? Int ?? 0
        }
    }
}

/// The Health page's introspection half (Part 14): what the detectors and the analyst found,
/// what came of the fixes, the recurring jobs, the triage review queue and the digest.
public struct IntrospectionSnapshot: Equatable, Sendable {
    public struct Finding: Equatable, Sendable {
        public var id: Int
        public var title: String
        public var metric: String
        public var task: Int
        public var verdict: String
        public var text: String
    }
    public struct Job: Equatable, Sendable {
        public var name: String
        public var kind: String
        public var runs: Int
        public var status: String
    }
    public var findings: [Finding] = []
    public var jobs: [Job] = []
    public var triageOn = false
    public var triageWaiting = 0
    /// Lines of the digest, each a figure computed from recorded data.
    public var digest: [String] = []

    public init(json: String) {
        guard let d = json.data(using: .utf8),
              let v = try? JSONSerialization.jsonObject(with: d) as? [String: Any] else { return }
        findings = (v["findings"] as? [[String: Any]] ?? []).map {
            Finding(
                id: $0["id"] as? Int ?? 0,
                title: $0["title"] as? String ?? "?",
                metric: $0["metric"] as? String ?? "?",
                task: $0["task"] as? Int ?? 0,
                verdict: $0["verdict"] as? String ?? "open",
                text: $0["text"] as? String ?? ""
            )
        }
        jobs = (v["schedules"] as? [[String: Any]] ?? []).map {
            Job(
                name: $0["name"] as? String ?? "?",
                kind: $0["kind"] as? String ?? "?",
                runs: $0["runs"] as? Int ?? 0,
                status: $0["last_status"] as? String ?? ""
            )
        }
        let t = v["triage"] as? [String: Any] ?? [:]
        triageOn = t["enabled"] as? Bool ?? false
        triageWaiting = (t["queue"] as? [Any])?.count ?? 0
        let g = v["digest"] as? [String: Any] ?? [:]
        digest.append("Landed: \(g["landed"] as? Int ?? 0)")
        if let m = g["median_claim_to_land_ms"] as? Int {
            digest.append("Median time from claim to land: \(m / 1000) s")
        }
        if let p = g["interventions_per_landed"] as? Double {
            digest.append("Interventions per landed task: \(String(format: "%.2f", p))")
        }
        digest.append("Tokens per landed task: not recorded")
        let ex = g["exploring"] as? [String: Any] ?? [:]
        digest.append("Exploring skipped for \(ex["skipped"] as? Int ?? 0) task(s); \(ex["came_back"] as? Int ?? 0) came back")
        for (route, r) in (ex["by_route"] as? [String: Any] ?? [:]).sorted(by: { $0.key < $1.key }) {
            let r = r as? [String: Any] ?? [:]
            digest.append("  \(route): \(r["tasks"] as? Int ?? 0) task(s), \(r["came_back"] as? Int ?? 0) came back")
        }
        for f in g["forced_landings"] as? [[String: Any]] ?? [] {
            digest.append("Forced landing, task #\(f["task"] as? Int ?? 0): \(f["reason"] as? String ?? "")")
        }
    }
}

/// The daemon's answer to a publish preview: the redacted text, and what it would touch.
public struct PublishPreview: Equatable, Sendable {
    public var title = ""
    public var body = ""
    public var head = ""
    public var base = ""
    public var files: [String] = []
    public var redactions = 0
    public var blocked: String?
    public var pr: Int?

    public init(json: String) {
        guard let d = json.data(using: .utf8), let o = try? JSONSerialization.jsonObject(with: d) as? [String: Any] else { return }
        title = o["title"] as? String ?? ""
        body = o["body"] as? String ?? ""
        head = o["head"] as? String ?? ""
        base = o["base"] as? String ?? ""
        files = o["files"] as? [String] ?? []
        redactions = o["redactions"] as? Int ?? 0
        blocked = o["blocked"] as? String
        pr = o["pr"] as? Int
    }
}

/// The keys the daemon offers for a project's remote.
public struct SSHKeyList: Equatable, Sendable {
    public struct Key: Equatable, Sendable, Identifiable {
        public var id: String { path.isEmpty ? fingerprint : path }
        public var path = ""
        public var fingerprint = ""
        public var comment = ""
        public var inAgent = false
        public var encrypted = false
        public var onOwnerAccount = false
        public var seenAs: String?
        /// A key with no file cannot be named to ssh: only the agent has it.
        public var usable: Bool { !path.isEmpty }
    }
    public var keys: [Key] = []
    /// The daemon's host: where the passphrase dialog appears.
    public var host = ""

    public init(json: String) {
        guard let d = json.data(using: .utf8), let o = try? JSONSerialization.jsonObject(with: d) as? [String: Any] else { return }
        host = o["host"] as? String ?? ""
        keys = (o["keys"] as? [[String: Any]] ?? []).map { k in
            var x = Key()
            x.path = k["path"] as? String ?? ""
            x.fingerprint = k["fingerprint"] as? String ?? ""
            x.comment = k["comment"] as? String ?? ""
            x.inAgent = k["in_agent"] as? Bool ?? false
            x.encrypted = k["encrypted"] as? Bool ?? false
            x.onOwnerAccount = k["on_owner_account"] as? Bool ?? false
            x.seenAs = k["seen_as"] as? String
            return x
        }
    }
}

final class MuteBox { var value = false }
