// SPDX-License-Identifier: GPL-3.0-or-later

import Foundation
import MasonProtocol

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
@MainActor
public final class MonitorModel: ObservableObject {
    // From the daemon.
    @Published public var header: HeaderView?
    @Published public var sidebar: SidebarView?
    @Published public var home: HomeView?
    @Published public var taskDetail: TaskDetailView?
    @Published public var landing: LandingView?
    @Published public var usage: UsageView?
    @Published public var editor: EditorView?
    @Published public var health: HealthSnapshot?
    @Published public var lastSeq: UInt64 = 0
    @Published public var connected = false
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
    /// What the update is doing (or why it cannot), while the user has asked for one.
    @Published public var updating: String?
    /// Run when the user presses the update button (the app does the work).
    public var onUpdate: (() -> Void)?

    /// Is there a newer monitor? Only when both builds are real versions and differ.
    public var updateAvailable: Bool {
        !daemonVersion.isEmpty && daemonVersion != "dev" && ownVersion != "dev" && daemonVersion != ownVersion
    }

    // The user's.
    @Published public private(set) var history = NavHistory()
    @Published public var search = SearchState()
    @Published public var textScale: Double { didSet { prefs.setDouble(textScale, for: Keys.scale) } }
    @Published public var muted: Bool { didSet { prefs.setBool(muted, for: Keys.mute) } }
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
        static func section(_ s: String) -> String { "section.\(s).expanded" }
    }

    public static let scaleRange: ClosedRange<Double> = 0.8...2.4
    public static let scaleStep = 0.1

    public init(preferences: Preferences = MemoryPreferences()) {
        prefs = preferences
        textScale = preferences.double(Keys.scale) ?? 1.0
        muted = preferences.bool(Keys.mute) ?? false
    }

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

    /// A dash while disconnected: never a stale count, never a false zero.
    public func tileCount(_ c: StateCount) -> String {
        connected ? String(c.count) : "–"
    }

    /// The attention indicator is shown only while something needs a look.
    public var attention: UInt32 { connected ? (header?.attention ?? 0) : 0 }

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
    public func retry(_ task: UInt64, from: String = "") { command { $0.retry = .with { $0.task = task; $0.from = from } } }
    public func approve(_ task: UInt64) { command { $0.approve = .with { $0.task = task } } }
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

    public func addTask(project: String, title: String, body: String) {
        command { $0.addTask = .with { $0.project = project; $0.title = title; $0.body = body } }
    }
    public func setSessionLimit(_ units: UInt32) {
        let name = usage?.providers.first?.name ?? "claude"
        command { $0.setLimit = .with { $0.resource = "llm:\(name)"; $0.units = units } }
    }
    public func pause(mode: String, seconds: UInt64) {
        command { $0.pause = .with { $0.project = project; $0.mode = mode; $0.forSeconds = seconds } }
    }
    public func resume() { command { $0.resume = .with { $0.project = project } } }
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

    // MARK: updates from the daemon

    public func apply(_ m: Mason_V1_ServerMessage) {
        switch m.body {
        case .welcome(let w)?:
            connected = true
            if w.resume == .snapshot { lastSeq = 0 }
            daemonVersion = w.version
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
        guard let seen = attentionSeen, !muted else { return }
        if !ids.subtracting(seen).isEmpty { onAttention?() }
    }

    func apply(_ v: Mason_V1_View) {
        switch v.body {
        case .header(let x)?: header = x
        case .sidebar(let x)?:
            sidebar = x
            noteAttention(x)
        case .home(let x)?: home = x
        case .taskDetail(let x)?: taskDetail = x
        case .landing(let x)?: landing = x
        case .usage(let x)?: usage = x
        case .editor(let x)?: editor = x
        default: break
        }
    }

    func handle(result r: Mason_V1_CommandResult) {
        if r.id == "health", r.ok { health = HealthSnapshot(json: r.json) }
    }

    public func disconnected() {
        connected = false
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
    }
}
