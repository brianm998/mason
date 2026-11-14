// SPDX-License-Identifier: GPL-3.0-or-later

import Foundation
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

/// What the monitor knows and what the user has done to it. The client draws
/// this and sends commands; it never re-derives pipeline rules: counts,
/// sorting, allowed actions and waiting text all arrive from the daemon.
/// State that changes on nearly every message. Views that show it observe this object; the rest
/// of the model, and every sidebar row, do not. `clockOffsetMs` is not published: the progress
/// lines read it on their shared one-second tick.
@MainActor
public final class LiveState: ObservableObject {
    @Published public var lastSeq: UInt64 = 0
    @Published public var liveLog: [String] = []
    public var clockOffsetMs: Int64 = 0
    public init() {}

    /// The daemon's time now, ms since the epoch, by this device's clock and the offset.
    public func daemonNow(_ local: Date = Date()) -> UInt64 {
        let ms = Int64(local.timeIntervalSince1970 * 1000) + clockOffsetMs
        return ms > 0 ? UInt64(ms) : 0
    }
}

@MainActor
public final class MonitorModel: ObservableObject {
    // From the daemon.
    @Published public var header: HeaderView?
    @Published public var sidebar: SidebarView?
    @Published public var home: HomeView?
    /// The CPU shares, observed apart from the whole model: the CPU section watches only this.
    public let cpu = CpuModel()
    @Published public var taskDetail: TaskDetailView?
    @Published public var landing: LandingView?
    @Published public var usage: UsageView?
    @Published public var editor: EditorView?
    @Published public var health: HealthSnapshot?
    @Published public var introspection: IntrospectionSnapshot?
    /// The high-frequency state (last sequence number, live log, clock offset) lives in its own
    /// object, so a change to it redraws only the views that show it, never the sidebar rows.
    public let live = LiveState()
    public var lastSeq: UInt64 {
        get { live.lastSeq }
        set { live.lastSeq = newValue }
    }
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
    @Published public var connected = false
    /// The connection is down and being retried: the daemon is restarting or has
    /// not answered. Shown as "reconnecting", never as a hard error.
    @Published public var reconnecting = false
    /// Whether the daemon took this connection's identity for the owner's (its welcome says).
    /// Only then are the answer, approve and reject buttons drawn: anyone else sees the card.
    @Published public var isOwner = false
    public var canAct: Bool { isOwner && readOnlyReason == nil }

    // About versions (Part 15).
    /// The protocol this monitor speaks.
    public static let protocolVersion: UInt32 = 1
    /// This monitor's own build: a commit, or "dev" for a build nobody versioned.
    public var ownVersion: String = ProcessInfo.processInfo.environment["MASON_MONITOR_VERSION"] ?? "dev"
    /// The daemon's build, from its welcome.
    @Published public var daemonVersion = ""
    /// Set when the daemon needs a newer protocol than this monitor speaks: nothing
    /// can be sent, and this says why.
    @Published public var readOnlyReason: String?
    /// What this monitor's update is doing, while the user has asked for one.
    @Published public var updating: String?
    /// Why the last update of this monitor did not happen.
    @Published public var updateError: String?
    /// Puts the finished build (a path on this machine) in place, launches it and quits.
    /// Only the desktop app sets this; a phone has no way to replace itself.
    public var onInstall: ((String) -> Void)?
    /// The daemon said this monitor's build is on its way.
    private var monitorAsked = false
    /// The connection is to a daemon on this machine, so a path it gives is one we can open.
    @Published public private(set) var localDaemon = true
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

    /// What to say about the daemon's own version; empty when there is nothing to say.
    public var daemonUpdateText: String { updateInfo?.daemonText ?? "" }
    public var canUpdateDaemon: Bool { canAct && updateInfo?.daemonCanUpdate == true }

    /// On the `release` channel (a packaged install) there is no source to build from: the
    /// button opens the download page instead.
    public var downloadURL: String { updateInfo?.downloadURL ?? "" }
    public var onOpenURL: ((URL) -> Void)?
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
    @Published public var confirmingDaemonUpdate = false

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
        c.raw = .with { $0.method = "POST"; $0.path = "/update/monitor"; $0.json = "{\"kind\":\"\(Self.monitorKind)\"}" }
        send(c)
    }

    /// The update of this monitor stopped; this one keeps running.
    public func monitorUpdateFailed(_ why: String) {
        monitorAsked = false
        updating = nil
        updateError = why
    }

    /// Follow the build the owner asked for, from each new header.
    private func noteMonitorBuild() {
        guard monitorAsked else { return }
        switch monitorBuild?.phase {
        case "building":
            break
        case "ready":
            monitorAsked = false
            updating = "Starting the new monitor…"
            onInstall?(monitorBuild?.path ?? "")
        case "failed":
            monitorUpdateFailed("The new monitor did not build: \(monitorBuild?.error ?? "")")
        default:
            monitorUpdateFailed("The daemon restarted during the build. Press Update to start it again.")
        }
    }

    // The user's.
    @Published public private(set) var history = NavHistory()
    @Published public var search = SearchState()
    @Published public var textScale: Double { didSet { prefs.setDouble(textScale, for: Keys.scale) } }
    /// The panel's width before text-zoom scaling; always inside the token limits.
    @Published public private(set) var sidebarWidth: Double
    @Published public var muted: Bool {
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
    @Published public private(set) var daemonDown = false
    /// This monitor's name when it asks to speak; one per launch.
    public let clientId = UUID().uuidString
    @Published public var project = "" { didSet { attentionSeen = nil } }
    /// Called (on the main actor) when something new needs the owner and the monitor is not
    /// muted: a task asked a question, waits for approval, was proposed, or failed. The app
    /// plays its sound here; the model only decides when.
    public var onAttention: (() -> Void)?
    /// The tasks that needed the owner at the last update; nil until the first one, so what
    /// was already waiting when the monitor opened (or the project changed) is silent.
    private var attentionSeen: Set<UInt64>?
    /// Sections the user opened or closed, remembered per device.
    @Published private var overrides: [String: Bool] = [:]
    /// Where the sidebar should scroll to (a tile was clicked).
    @Published public var scrollTarget: String?
    @Published public var lastResult: String?
    /// Open session found the step running: the task to ask about. Answer with `answerSessionPrompt`.
    @Published public var sessionPrompt: UInt64?
    /// What Open session did or could not do, in words, until the next click.
    @Published public var sessionNotice: String?
    /// Starts a terminal running this argument vector; false when it could not. The app sets it.
    public var launcher: (([String]) -> Bool)?
    /// Puts text on the pasteboard. The app sets it.
    public var copier: ((String) -> Void)?
    /// The CLI by its full path.
    public var cliPath = OpenSession.defaultCLI()
    /// What `mason proposal publish --preview` would send, in the daemon's words, once asked for.
    @Published public var publishPreview: PublishPreview?
    /// The SSH key sheet after a refused push: the project it is for, the keys offered, and what
    /// choosing one said. nil project: the sheet is closed.
    @Published public var sshKeyProject: String?
    @Published public var sshKeys: SSHKeyList?
    @Published public var sshKeyResult: String?
    @Published public var sshKeyBusy = false
    /// The step selected in the pipeline editor (for its inspector).
    @Published public private(set) var editorStep = ""
    /// Called with every command result (the tools read them; the app shows errors).
    public var onResult: ((Mason_V1_CommandResult) -> Void)?

    private let prefs: Preferences
    public var sender: ((Mason_V1_Command) -> Void)?
    public var subscribe: (() -> Void)?

    public enum Keys {
        static let scale = "textScale"
        static let mute = "muted"
        static let sidebarWidth = "sidebarWidth"
        static func section(_ s: String) -> String { "section.\(s).expanded" }
    }

    public static let scaleRange: ClosedRange<Double> = 0.8...2.4
    public static let scaleStep = 0.1

    public init(preferences: Preferences = MemoryPreferences()) {
        prefs = preferences
        textScale = preferences.double(Keys.scale) ?? 1.0
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
    private var lastClaim = Date.distantPast
    static let claimEvery: TimeInterval = 3

    // MARK: navigation

    public var page: Page { history.current }
    public var canGoBack: Bool { history.canGoBack }
    public var canGoForward: Bool { history.canGoForward }

    public func go(_ p: Page) { history.go(p); subscribe?() }
    public func goBack() { history.goBack(); subscribe?() }
    public func goForward() { history.goForward(); subscribe?() }
    public func goHome() { history.goHome(); subscribe?() }

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
    public var attention: UInt32 { connected ? (header?.attention ?? 0) : 0 }

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

    public func toggle(_ s: SidebarSection) { setExpanded(s.state, !isExpanded(s)) }

    /// Clicking a tile expands its section and scrolls to it.
    public func reveal(state: String) {
        setExpanded(state, true)
        scrollTarget = state
    }

    /// What a tile click does. Every click reveals at once (never waits to tell
    /// a single click from a double); the second click of a double also opens
    /// the lone task.
    public func tileClicked(state: String, clickCount: Int) {
        reveal(state: state)
        if clickCount == 2 { revealAndOpenIfOne(state: state) }
    }

    /// Reveal the state's section and, if it holds exactly one task right now
    /// (rows as they are at this moment, search ignored), open that task.
    /// Does nothing while disconnected.
    public func revealAndOpenIfOne(state: String) {
        guard connected else { return }
        reveal(state: state)
        guard let sec = sidebar?.sections.first(where: { $0.state == state }), sec.rows.count == 1 else { return }
        go(.task(sec.rows[0].id))
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
    public var identity: String = ProcessInfo.processInfo.environment["MASON_AS"] ?? NSUserName()

    public func send(_ c: Mason_V1_Command) {
        if let why = readOnlyReason {
            lastResult = "Read-only: \(why)"
            return
        }
        var c = c
        if c.actor.isEmpty { c.actor = identity }
        sender?(c)
    }

    private func command(_ build: (inout Mason_V1_Command) -> Void) {
        var c = Mason_V1_Command()
        build(&c)
        send(c)
    }

    public func cancel(_ task: UInt64) { command { $0.cancel = .with { $0.task = task } } }
    public func retry(_ task: UInt64, from: String = "", seen: UInt64 = 0) { command { $0.retry = .with { $0.task = task; $0.from = from; $0.seen = seen } } }
    public func approve(_ task: UInt64) { command { $0.approve = .with { $0.task = task } } }
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
    public func force(_ task: UInt64) { command { $0.force = .with { $0.task = task } } }
    public func skipTest(_ task: UInt64, step: String) {
        command { $0.skip = .with { $0.task = task; $0.step = step } }
    }
    public func answer(task: UInt64, _ text: String, cardHash: String) {
        command { $0.answer = .with { $0.task = task; $0.answer = text; $0.cardHash = cardHash } }
    }
    /// Approve a held self-change: the exact diff the owner was shown.
    public func approveHold(_ task: UInt64, diffHash: String) {
        command { $0.approveHold = .with { $0.task = task; $0.diffHash = diffHash } }
    }

    /// What Approve sends for an approval: the held diff's hash when there is one, else a plain approve.
    public func approve(_ task: UInt64, _ a: Mason_V1_Approval) {
        if a.diffHash.isEmpty { approve(task) } else { approveHold(task, diffHash: a.diffHash) }
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
    /// How many compiles may run at once (the daemon refuses 0).
    public func setCompileLimit(_ units: UInt32) {
        command { $0.setLimit = .with { $0.resource = "compile"; $0.units = max(1, units) } }
    }
    public func pause(mode: String, seconds: UInt64) {
        command { $0.pause = .with { $0.project = project; $0.mode = mode; $0.forSeconds = seconds } }
    }
    public func resume() { command { $0.resume = .with { $0.project = project } } }
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

    public func raw(_ method: String, _ path: String, json: String = "") {
        command { $0.raw = .with { $0.method = method; $0.path = path; $0.json = json } }
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

    /// A step's progress arrived without a view: put it into the task page and the rows, and follow the log.
    public func apply(progress p: Mason_V1_StepProgress, local: Date = Date()) {
        anchor(daemonNow: p.nowMs, local: local)
        guard p.hasProgress else { return }
        let cell = p.progress
        func merged(_ old: [ProgressCell]) -> [ProgressCell]? {
            guard let i = old.firstIndex(where: { $0.step == cell.step }) else { return nil }
            var c = cell
            // A finished or stopped run says what it ended as on the next view; until then, no "running".
            if c.state != .part { c.label = "" }
            var out = old
            out[i] = c
            return out
        }
        if var d = taskDetail, d.hasRow, d.row.id == p.task {
            if let cells = merged(d.row.progress) {
                d.row.progress = cells
                taskDetail = d
            }
            appendLog(p.log)
        }
        if var s = sidebar {
            var changed = false
            for si in s.sections.indices {
                for ri in s.sections[si].rows.indices where s.sections[si].rows[ri].id == p.task {
                    if let cells = merged(s.sections[si].rows[ri].progress) {
                        s.sections[si].rows[ri].progress = cells
                        changed = true
                    }
                }
            }
            if changed { sidebar = s }
        }
    }

    func appendLog(_ lines: [String]) {
        guard !lines.isEmpty else { return }
        liveLog = Array((liveLog + lines).suffix(Self.logKeep))
    }

    // MARK: updates from the daemon

    public func apply(_ m: Mason_V1_ServerMessage) {
        switch m.body {
        case .welcome(let w)?:
            connected = true
            reconnecting = false
            announcer.connectionRestored()
            if w.resume == .snapshot { lastSeq = 0 }
            daemonVersion = w.version
            localDaemon = w.role.isEmpty
            // A build asked for on the connection that just went has no answer coming.
            if !monitorAsked { updating = nil }
            anchor(daemonNow: w.nowMs)
            isOwner = w.owner
            readOnlyReason = w.minProtocol > Self.protocolVersion
                ? "This monitor speaks protocol \(Self.protocolVersion), but the daemon needs \(w.minProtocol) or newer. Update the monitor; until then it can only look."
                : nil
        case .snapshot(let s)?:
            lastSeq = max(lastSeq, s.seq)
            s.views.forEach(apply)
        case .events(let e)?:
            if let last = e.events.last?.seq { lastSeq = max(lastSeq, last) }
        case .view(let u)?:
            lastSeq = max(lastSeq, u.seq)
            apply(u.view)
        case .stepProgress(let p)?:
            apply(progress: p)
        case .result(let r)?:
            lastResult = r.ok ? nil : r.error
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

    func apply(_ v: Mason_V1_View) {
        switch v.body {
        case .header(let x)?:
            header = x
            noteMonitorBuild()
        case .sidebar(let x)?:
            sidebar = x
            noteAttention(x)
        case .home(let x)?:
            home = x
            cpu.update(x)
        case .taskDetail(let x)?:
            // A different task's page starts its log over; the same task's view carries the tail again.
            taskDetail = x
            liveLog = x.logTail.split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
            if liveLog.last == "" { liveLog.removeLast() }
            liveLog = Array(liveLog.suffix(Self.logKeep))
        case .landing(let x)?: landing = x
        case .usage(let x)?: usage = x
        case .editor(let x)?: editor = x
        default: break
        }
    }

    func handle(result r: Mason_V1_CommandResult) {
        if r.id.hasPrefix("publish:"), r.ok { publishPreview = PublishPreview(json: r.json) }
        if r.id.hasPrefix("session:"), let task = UInt64(r.id.dropFirst("session:".count)) {
            switch OpenSession.decide(ok: r.ok, json: r.json, error: r.error) {
            case .confirm: sessionPrompt = task
            case .launch(let anyway): launchSession(task, anyway: anyway)
            case .failed(let why): sessionNotice = why
            }
        }
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
    }

    public func disconnected(now: Date = Date()) {
        connected = false
        reconnecting = true
        announcer.connectionLost(at: now)
    }
}

extension Double {
    func rounded(toPlaces n: Int) -> Double {
        let p = pow(10.0, Double(n))
        return (self * p).rounded() / p
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
