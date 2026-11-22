// SPDX-License-Identifier: GPL-3.0-or-later

import MasonTokens
import MonitorCore
import SwiftUI

/// The sidebar, on every page. Pinned above the list: a Home row and search. In the
/// scrolling part: one collapsible section per state (the daemon's order and sorting),
/// then the other pages. `width` is the drawn width, already fitted to the window.
struct SidebarPane: View {
    @Environment(\.theme) var theme
    let model: MonitorModel
    let width: CGFloat

    #if DEBUG
    /// Body evaluations, for the offscreen test that counts them.
    nonisolated(unsafe) static var evaluations = 0
    #endif

    var body: some View {
        #if DEBUG
        let _ = { Self.evaluations += 1 }()
        #endif
        VStack(spacing: 0) {
            VStack(alignment: .leading, spacing: 0) {
                homeRow
                SearchBox(model: model).padding(theme.spacing)
            }
            Divider()
            ScrollViewReader { proxy in
                ScrollView {
                    // Lazy: only the rows near the viewport exist. Entries keep their `SidebarEntry.id`
                    // through ForEach, so rows keep identity and `scrollTo` reaches an unrealised header.
                    LazyVStack(alignment: .leading, spacing: 0) {
                        if model.searchMatchesNothing {
                            VStack(spacing: theme.spacing) {
                                MascotPicture(id: "no_results", points: MasonTokens.sizeMascotStatus)
                                Text(searchNoMatchText).font(theme.body()).foregroundStyle(theme.muted)
                                    .accessibilityIdentifier("search:no-results")
                            }.frame(maxWidth: .infinity).padding(theme.spacing)
                        }
                        ForEach(model.sidebarEntries) { e in
                            entry(e)
                        }
                    }
                    .frame(width: width, alignment: .leading)
                }
                // A new project is a new scroll view: top offset, no stale content-height estimate from the
                // old list (#926). Later sidebar updates keep the project, so they keep the realised rows.
                .id(model.project)
                .onChange(of: model.scrollTarget) { _, target in
                    guard let t = target else { return }
                    model.scrollTarget = nil
                    // Next turn: the expanded section's rows are laid out by then. The pinned
                    // area is outside the ScrollView, so the top is below it.
                    DispatchQueue.main.async { withAnimation { proxy.scrollTo(SidebarEntry.headerId(t), anchor: .top) } }
                }
            }
            ActiveSessionsPanel(model: model)
        }
        .frame(width: width)
        .background(theme.surface.opacity(0.6))
    }

    @ViewBuilder func entry(_ e: SidebarEntry) -> some View {
        switch e {
        case .header(let sec): SectionHeader(model: model, section: sec)
        case .row(let r):
            RowView(
                row: r, selected: model.page == .task(r.id), canAct: model.canAct, clock: model.live, store: model.progress,
                approving: model.pendingLabel(task: r.id, action: "approve"), busy: model.isBusy(task: r.id),
                go: { [model] p in model.go(p) }, approve: { [model] id in model.approve(id) }
            ).equatable()
        case .rule: Divider().padding(.vertical, theme.spacing)
        case .pageLink(let title, let page):
            pageLink(title, icon: Self.icon(page), page: page)
        }
    }

    static func icon(_ p: Page) -> String {
        switch p {
        case .landing: return "arrow.down.to.line"
        case .usage: return "gauge"
        case .pipeline: return "point.3.connected.trianglepath.dotted"
        case .settings: return "gearshape"
        default: return "heart.text.square"
        }
    }

    var homeRow: some View {
        Button { model.goHome() } label: {
            Label("Home", systemImage: "house")
                .font(theme.body(.semibold))
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(theme.spacing)
                .background(model.page == .home ? theme.accent.opacity(0.18) : Color.clear)
        }
        .buttonStyle(.plain).touchTarget()
        .accessibilityIdentifier("sidebar:home")
    }

    #if DEBUG
    /// Page-link titles whose body has run, for the offscreen scroll test.
    nonisolated(unsafe) static var realisedLinks = Set<String>()
    #endif

    @ViewBuilder func pageLink(_ title: String, icon: String, page: Page) -> some View {
        #if DEBUG
        let _ = { Self.realisedLinks.insert(title) }()
        #endif
        Button { model.go(page) } label: {
            Label(title, systemImage: icon)
                .font(theme.body())
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, theme.spacing).padding(.vertical, theme.spacingSmall + 2)
                .background(model.page == page ? theme.accent.opacity(0.18) : Color.clear)
        }
        .buttonStyle(.plain).touchTarget()
        .accessibilityIdentifier("sidebar:\(title.lowercased().replacingOccurrences(of: " ", with: "-"))")
    }
}

struct SearchBox: View {
    @Environment(\.theme) var theme
    @Bindable var model: MonitorModel

    var body: some View {
        VStack(alignment: .leading, spacing: theme.spacingSmall) {
            HStack {
                Image(systemName: "magnifyingglass").foregroundStyle(theme.muted)
                TextField("Search: name, title, body", text: $model.search.text)
                    .textFieldStyle(.plain)
                    .font(theme.body())
                    .accessibilityIdentifier("search")
            }
            .padding(theme.spacingSmall + 2)
            .background(RoundedRectangle(cornerRadius: theme.radius).fill(theme.background))
            .overlay(RoundedRectangle(cornerRadius: theme.radius).stroke(theme.border))
            HStack(spacing: theme.spacing) {
                Toggle("name", isOn: $model.search.inName)
                Toggle("title", isOn: $model.search.inTitle)
                Toggle("body", isOn: $model.search.inBody)
            }
            .toggleStyle(.checkbox)
            .font(theme.small())
        }
    }
}

struct SectionHeader: View {
    @Environment(\.theme) var theme
    let model: MonitorModel
    let section: SidebarSection

    #if DEBUG
    /// Section states whose header body has run, for the offscreen scroll test.
    nonisolated(unsafe) static var realised = Set<String>()
    /// Body evaluations, for the offscreen test that counts them.
    nonisolated(unsafe) static var evaluations = 0
    #endif

    var body: some View {
        #if DEBUG
        let _ = { Self.realised.insert(section.state); Self.evaluations += 1 }()
        #endif
        let open = model.isExpanded(section)
        HStack(spacing: 0) {
        Button { model.toggle(section) } label: {
            HStack(spacing: theme.spacingSmall) {
                Image(systemName: open ? "chevron.down" : "chevron.right").frame(width: theme.cell)
                StateIcon(state: section.state)
                Text(section.title).font(theme.body(.semibold)).lineLimit(1).truncationMode(.tail)
                Spacer(minLength: 0)
                Text(model.countText(section))
                    .font(theme.body(.semibold)).lineLimit(1).fixedSize().layoutPriority(1)
                    .foregroundStyle(section.state == "needs_attention" && section.count > 0 ? theme.attention : theme.muted)
            }
            .padding(.leading, theme.spacing).padding(.vertical, theme.spacingSmall + 2)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain).touchTarget()
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(section.title), \(model.countText(section)), \(open ? "expanded" : "collapsed")")
        .accessibilityIdentifier("section:\(section.state)")
        if let page = Page.sectionPage(section.state) {
            Button { model.go(page) } label: {
                Image(systemName: "chevron.right").font(theme.body(.semibold)).foregroundStyle(theme.muted)
                    .padding(.horizontal, theme.spacing).padding(.vertical, theme.spacingSmall + 2)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain).touchTarget().hint("Open the \(section.title) page")
            .accessibilityLabel("Open \(section.title)")
            .accessibilityIdentifier("section-open:\(section.state)")
        } else {
            Color.clear.frame(width: theme.spacing)
        }
        }
    }
}

/// A sidebar row: key and title, project, activity, time in state, progress
/// cells, the model when one is running, and a link for each blocker.
struct RowView: View, Equatable {
    @Environment(\.theme) var theme
    let row: TaskRow
    let selected: Bool
    let canAct: Bool
    /// Read for the clock offset only; never observed, so a change to it does not redraw the row.
    let clock: LiveState
    let store: ProgressStore
    /// Task #522: what the row's Approve says while its command is pending (nil: nothing pending).
    var approving: String? = nil
    var busy = false
    let go: (Page) -> Void
    let approve: (UInt64) -> Void

    /// Rows are redrawn only when what they show changes; the closures and the clock do not count.
    static func == (a: RowView, b: RowView) -> Bool {
        a.row == b.row && a.selected == b.selected && a.canAct == b.canAct && a.approving == b.approving && a.busy == b.busy
            && a.clock === b.clock && a.store === b.store
    }

    #if DEBUG
    /// Body evaluations, for the offscreen test that counts them.
    nonisolated(unsafe) static var evaluations = 0
    #endif

    var body: some View {
        #if DEBUG
        let _ = { Self.evaluations += 1 }()
        #endif
        VStack(alignment: .leading, spacing: 2) {
            Button { go(.task(row.id)) } label: {
                VStack(alignment: .leading, spacing: 2) {
                    HStack(alignment: .firstTextBaseline, spacing: theme.spacingSmall) {
                        Text(row.key).font(theme.body(.semibold)).linkLook().fixedSize()
                        Text(row.title).font(theme.body()).linkLook().lineLimit(2)
                    }
                    Text("\(row.project), \(activity)")
                        .font(theme.small()).foregroundStyle(theme.muted).lineLimit(2)
                    if let mark = MonitorModel.runOnMarker(row) {
                        Text(mark).font(theme.small()).foregroundStyle(theme.muted)
                            .accessibilityIdentifier("row:\(row.id):run-on")
                    }
                    if !row.proposal.isEmpty {
                        Text(row.proposal).font(theme.small()).foregroundStyle(theme.muted)
                            .accessibilityIdentifier("row:\(row.id):proposal")
                    }
                    metaLine
                    LiveProgress(live: store.entry(row.id), fallback: row.progress) { cells in
                        if let c = cells.running { ProgressLine(clock: clock, cell: c) }
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain).touchTarget()
            .pointingHand()
            if row.issue != 0 {
                HStack(spacing: theme.spacing) {
                    if let url = URL(string: row.issueURL), !row.issueURL.isEmpty {
                        WebLink(title: "issue #\(row.issue)", url: url, font: theme.small())
                            .accessibilityIdentifier("issue:\(row.id)")
                    } else {
                        Text("issue #\(row.issue)").font(theme.small()).foregroundStyle(theme.muted)
                    }
                    if row.state == "proposed" && canAct && row.allowedActions.contains("approve") {
                        Button { approve(row.id) } label: { Text(approving ?? "Approve").pill(prominent: true) }
                            .buttonStyle(.plain).touchTarget()
                            .disabled(busy)
                            .accessibilityIdentifier("row:\(row.id):approve")
                    }
                }
            }
            if !row.blockers.isEmpty {
                HStack(spacing: theme.spacingSmall) {
                    Text("blocked by").font(theme.small()).foregroundStyle(theme.muted)
                    ForEach(row.blockers, id: \.self) { b in
                        TaskLink(title: "#\(b)", font: theme.small()) { go(.task(b)) }
                    }
                }
            }
        }
        .padding(.horizontal, theme.spacing).padding(.vertical, theme.spacingSmall + 1)
        .background(selected ? theme.accent.opacity(0.18) : Color.clear)
        .accessibilityElement(children: .contain)
        .accessibilityLabel("\(row.key) \(row.title), \(row.project), \(row.state)")
        .accessibilityIdentifier("row:\(row.state):\(row.id)")
    }

    /// The time / typical / cells / model / icon line, in one `MetaLayout` (one measurement per
    /// row). Pieces give way in order as the panel narrows: the model first, then the typical
    /// range, then the cells wrap under the time.
    var metaLine: some View {
        MetaLayout(spacing: theme.spacing, lineSpacing: 2) {
            durationText.metaRole(.duration)
            if !row.typical.isEmpty {
                Text(row.typical).font(theme.small()).foregroundStyle(theme.muted).lineLimit(1)
                    .droppable().metaRole(.typical)
            }
            LiveProgress(live: store.entry(row.id), fallback: row.progress) {
                ProgressCells(clock: clock, cells: $0, slow: row.slow)
            }.fixedSize().metaRole(.cells)
            if !row.display.isEmpty || !row.model.isEmpty {
                Text(row.display.isEmpty ? row.model : row.display)
                    .font(theme.small()).foregroundStyle(theme.muted).lineLimit(1)
                    .droppable().metaRole(.model)
            }
            StateIcon(state: row.state).metaRole(.icon)
        }
    }

    var durationText: some View {
        RowDuration(row: row, clock: clock, color: row.slow ? theme.slow : theme.muted)
    }

    var activity: String { QueueWait.line(row) }

}

/// Pinned at the bottom of the sidebar, under the list: every open owner session the owner is
/// part of (a task's, the pipeline's health), each with a marker while it is replying or waits
/// for the owner. Hidden when there are none. Clicking one opens its dialog.
struct ActiveSessionsPanel: View {
    @Environment(\.theme) var theme
    let model: MonitorModel

    var body: some View {
        let sessions = model.header?.activeSessions ?? []
        if let title = ActiveSessions.title(sessions) {
            VStack(alignment: .leading, spacing: 0) {
                Divider()
                Text(title).font(theme.small(.semibold)).foregroundStyle(theme.muted)
                    .padding(.horizontal, theme.spacing).padding(.top, theme.spacing)
                    .accessibilityIdentifier("active-sessions-title")
                ForEach(sessions, id: \.id) { s in
                    Button { model.openOwnerSession(s.id) } label: {
                        HStack(spacing: theme.spacing) {
                            Text(s.label).font(theme.body()).lineLimit(1)
                            Spacer(minLength: 0)
                            let marker = ActiveSessions.marker(s)
                            if !marker.isEmpty {
                                Text(marker).font(theme.small()).foregroundStyle(s.working ? theme.accent : theme.attention)
                            }
                        }
                        .padding(.horizontal, theme.spacing).padding(.vertical, theme.spacing / 2)
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain).touchTarget()
                    .accessibilityIdentifier("active-session:\(s.id)")
                }
            }
            .padding(.bottom, theme.spacing)
        }
    }
}

/// The owner-session dialog (s364): the transcript, a field to write in and the End button.
struct OwnerSessionSheet: View {
    @Environment(\.theme) var theme
    let model: MonitorModel
    @State private var draft = ""

    var body: some View {
        VStack(alignment: .leading, spacing: theme.spacing) {
            Text(model.ownerSession?.kind == "health" ? "Pipeline health" : "Session").font(theme.title())
            ScrollView {
                VStack(alignment: .leading, spacing: theme.spacing / 2) {
                    ForEach(model.ownerSession?.messages ?? []) { m in
                        Text("\(m.role == "owner" ? "you" : (m.role == "agent" ? "agent" : "·")): \(m.text)")
                            .font(m.role == "tool" ? theme.small() : theme.body())
                            .foregroundStyle(m.role == "tool" ? theme.muted : Color.primary)
                            .textSelection(.enabled)
                    }
                }.frame(maxWidth: .infinity, alignment: .leading)
            }
            .frame(minHeight: 240)
            if let e = model.ownerSessionError { Text(e).font(theme.small()).foregroundStyle(theme.attention) }
            HStack {
                TextField("Write to the agent", text: $draft).onSubmit(send).accessibilityIdentifier("session-input")
                Button("Send", action: send).disabled(model.ownerSession?.working == true)
                Button("End session") { model.endOwnerSession() }
                Button("Close") { model.closeOwnerSession() }.keyboardShortcut(.cancelAction)
            }
        }
        .padding(theme.spacingLarge)
        .frame(minWidth: 560)
        .accessibilityIdentifier("owner-session-sheet")
    }

    private func send() {
        model.sendOwnerMessage(draft)
        draft = ""
    }
}

/// What the meta line shows at a width: the pieces give way in this order.
enum MetaFit: Int, Comparable {
    case all, noModel, noTypical, wrapped
    static func < (a: MetaFit, b: MetaFit) -> Bool { a.rawValue < b.rawValue }

    /// The first variant whose pieces fit `available` on one line; `wrapped` when none does.
    /// Widths are ideal widths; a piece that is absent has width nil.
    static func choose(
        available: CGFloat, spacing: CGFloat, duration: CGFloat, typical: CGFloat?, cells: CGFloat,
        model: CGFloat?, icon: CGFloat
    ) -> MetaFit {
        func fits(_ parts: [CGFloat?]) -> Bool {
            let w = parts.compactMap { $0 }
            return w.reduce(0, +) + spacing * CGFloat(max(0, w.count - 1)) <= available
        }
        if fits([duration, typical, cells, model, icon]) { return .all }
        if fits([duration, typical, cells, icon]) { return .noModel }
        if fits([duration, cells, icon]) { return .noTypical }
        return .wrapped
    }
}

enum MetaRole: Int { case duration, typical, cells, model, icon }

struct MetaRoleKey: LayoutValueKey { static let defaultValue = MetaRole.duration }

extension View {
    func metaRole(_ r: MetaRole) -> some View { layoutValue(key: MetaRoleKey.self, value: r) }

    /// A piece the meta line may drop: it sizes to its content when asked for its ideal size, but
    /// is clipped to whatever width it is given, so a zero width shows nothing.
    func droppable() -> some View {
        fixedSize().frame(minWidth: 0, maxWidth: .greatestFiniteMagnitude, alignment: .leading).clipped()
    }
}

/// One layout for the row's meta line, replacing four alternative variants.
struct MetaLayout: Layout {
    let spacing: CGFloat
    let lineSpacing: CGFloat

    struct Cache {
        var ideal: [CGSize] = []
        var roles: [MetaRole] = []
        func width(_ r: MetaRole) -> CGFloat? { roles.firstIndex(of: r).map { ideal[$0].width } }
    }

    func makeCache(subviews: Subviews) -> Cache {
        Cache(ideal: subviews.map { $0.sizeThatFits(.unspecified) }, roles: subviews.map { $0[MetaRoleKey.self] })
    }

    func fit(_ c: Cache, width: CGFloat) -> MetaFit {
        MetaFit.choose(
            available: width, spacing: spacing, duration: c.width(.duration) ?? 0, typical: c.width(.typical),
            cells: c.width(.cells) ?? 0, model: c.width(.model), icon: c.width(.icon) ?? 0)
    }

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout Cache) -> CGSize {
        let ideals = cache.ideal
        let full = ideals.map(\.width).reduce(0, +) + spacing * CGFloat(max(0, ideals.count - 1))
        let w: CGFloat
        if let pw = proposal.width, pw.isFinite { w = pw } else { w = full }
        let mode = fit(cache, width: w)
        return CGSize(width: w, height: height(cache, mode))
    }

    private func height(_ c: Cache, _ mode: MetaFit) -> CGFloat {
        func h(_ r: MetaRole) -> CGFloat { c.roles.firstIndex(of: r).map { c.ideal[$0].height } ?? 0 }
        let top = max(h(.duration), h(.icon))
        switch mode {
        case .wrapped: return top + lineSpacing + h(.cells)
        case .noTypical: return max(top, h(.cells))
        case .noModel: return max(top, h(.cells), h(.typical))
        case .all: return max(top, h(.cells), h(.typical), h(.model))
        }
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout Cache) {
        let mode = fit(cache, width: bounds.width)
        let rowH = mode == .wrapped
            ? max(cache.roles.firstIndex(of: .duration).map { cache.ideal[$0].height } ?? 0,
                  cache.roles.firstIndex(of: .icon).map { cache.ideal[$0].height } ?? 0)
            : bounds.height
        var x = bounds.minX
        for (i, sv) in subviews.enumerated() {
            let role = cache.roles[i]
            let size = cache.ideal[i]
            let shown: Bool
            switch role {
            case .typical: shown = mode <= .noModel
            case .model: shown = mode == .all
            default: shown = true
            }
            if !shown {
                sv.place(at: CGPoint(x: bounds.minX, y: bounds.minY), anchor: .topLeading, proposal: ProposedViewSize(width: 0, height: size.height))
                continue
            }
            let one = ProposedViewSize(size)
            switch role {
            case .icon:
                sv.place(at: CGPoint(x: bounds.maxX, y: bounds.minY + rowH / 2), anchor: .trailing, proposal: one)
            case .cells where mode == .wrapped:
                sv.place(at: CGPoint(x: bounds.minX, y: bounds.minY + rowH + lineSpacing), anchor: .topLeading, proposal: one)
            default:
                sv.place(at: CGPoint(x: x, y: bounds.minY + rowH / 2), anchor: .leading, proposal: one)
                x += size.width + spacing
            }
        }
    }
}

/// The time in state, counted from the row's `stateSinceMs` on the shared one-second tick.
struct RowDuration: View {
    @Environment(\.theme) var theme
    let row: TaskRow
    let clock: LiveState
    let color: Color
    @ObservedObject private var ticker = SharedTick.shared

    var body: some View {
        Text(Format.duration(ms: row.inState(now: clock.daemonNow(ticker.now))))
            .font(theme.small()).foregroundStyle(color)
            .lineLimit(1).fixedSize()
            .onAppear { SharedTick.shared.join() }
            .onDisappear { SharedTick.shared.leave() }
    }
}
