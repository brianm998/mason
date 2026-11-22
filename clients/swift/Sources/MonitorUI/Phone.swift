// SPDX-License-Identifier: GPL-3.0-or-later

import MasonMascot
import MasonTokens
import MonitorCore
import SwiftUI

/// The phone's root (Part 17, portrait): the sidebar is the first screen and choosing a row
/// pushes its page, with Back to return. Count tiles scroll sideways above a Home row, then the
/// state sections, Landing, Usage, Health and Settings. The navigation bar carries a compact
/// status line (connection, paused, the attention count); banners sit above the list.
/// Every page the Mac monitor has is here but the pipeline editor (`PhoneLayout.phoneHasNo`).
/// Speech is off on the phone: this app never installs a speech sink or claims to speak.
public struct PhoneRootView: View {
    let model: MonitorModel
    @Environment(\.colorScheme) private var scheme
    @State private var nav = PhoneNav()
    @State private var adding = false
    @State private var windowHeight: CGFloat = 800
    @State private var improving = false
    #if !canImport(AppKit)
    @Environment(\.scenePhase) private var scenePhase
    #endif

    public init(model: MonitorModel, pushed: [Page] = []) {
        self.model = model
        _nav = State(initialValue: PhoneNav(stack: pushed))
    }

    public var body: some View {
        let theme = Theme(dark: scheme == .dark, scale: model.textScale)
        Group {
            if model.header == nil {
                PhoneStartView(model: model)
            } else {
                stack
            }
        }
        .environment(\.theme, theme)
        .foregroundStyle(theme.text)
        #if !canImport(AppKit)
        .onChange(of: scenePhase, initial: true) { _, phase in SharedTick.shared.setVisible(phase == .active) }
        #endif
        .onAppear { SharedTick.shared.model = model }
        .onOpenURL { url in if let p = Page.from(url: url) { model.open(url: url); nav.follow(p) } }
    }

    private var stack: some View {
        NavigationStack(path: Binding(get: { nav.stack }, set: { nav.replace($0) })) {
            VStack(spacing: 0) {
                BannersView(model: model, windowHeight: windowHeight)
                PhoneSidebarScreen(model: model) { nav.select($0) }
            }
            .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { windowHeight = $0 }
            .phoneChrome(model: model, title: "MASON", open: openAttention, add: { adding = true }, improve: { improving = true })
            .navigationDestination(for: Page.self) { page in
                PhoneDetailScreen(model: model, page: page)
                    .phoneChrome(model: model, title: phoneTitle(page, model: model), open: openAttention, add: { adding = true }, improve: { improving = true })
            }
        }
        // The model's page follows the stack (it picks what the daemon sends), and a page the
        // model opens itself (a link inside a page) is pushed.
        .onChange(of: nav.stack) { _, s in
            let page = s.last ?? .home
            if model.page != page { model.go(page) }
        }
        .onChange(of: model.page) { _, page in nav.follow(page) }
        .sheet(isPresented: $adding) { AddTaskSheet(model: model, isPresented: $adding) }
        .sheet(isPresented: $improving) { ImproveSheet(model: model, isPresented: $improving) }
    }

    /// The attention count in the status line opens the first task that needs the owner.
    private func openAttention() {
        if let t = model.firstNeedingYou { nav.select(.task(t)) } else { nav.select(.home) }
    }
}

/// The title of a pushed page.
@MainActor
func phoneTitle(_ page: Page, model: MonitorModel) -> String {
    switch page {
    case .home: return "Home"
    case .task(let id):
        if let r = model.taskDetail?.row, r.id == id, !r.key.isEmpty { return r.key }
        return "#\(id)"
    case .landing: return "Landing"
    case .usage: return "LLM usage"
    case .health: return "Health"
    case .settings: return "Settings"
    case .pipeline: return "Pipeline"
    case .section(let name): return MonitorModel.sectionTitle(name)
    }
}

extension MonitorModel {
    /// A pause banner is up.
    var isPaused: Bool { (header?.banners ?? []).contains { $0.kind == "paused" } }
}

/// The compact status line: the connection, paused, and the attention count. A tap on it opens
/// the first task that needs the owner while any does.
struct PhoneStatusLine: View {
    @Environment(\.theme) var theme
    let model: MonitorModel
    let open: () -> Void

    var text: String {
        PhoneLayout.statusLine(connected: model.connected, reconnecting: model.reconnecting,
                               paused: model.isPaused, attention: model.attention)
    }

    var body: some View {
        Button(action: open) {
            Text(text).font(.footnote)
                .foregroundStyle(model.attention > 0 ? theme.attention : theme.muted)
                .lineLimit(1).minimumScaleFactor(0.7)
        }
        .buttonStyle(.plain)
        .allowsHitTesting(model.attention > 0)
        .accessibilityIdentifier("phone-status")
    }
}

extension View {
    /// The navigation bar every phone screen has: a title, the status line, and a menu with the
    /// project filter, add task, improve and mute.
    func phoneChrome(model: MonitorModel, title: String, open: @escaping () -> Void,
                     add: @escaping () -> Void, improve: @escaping () -> Void) -> some View {
        modifier(PhoneChrome(model: model, title: title, open: open, add: add, improve: improve))
    }
}

struct PhoneChrome: ViewModifier {
    @Bindable var model: MonitorModel
    let title: String
    let open: () -> Void
    let add: () -> Void
    let improve: () -> Void

    func body(content: Content) -> some View {
        content
            .navigationTitle(title)
            #if os(iOS)
            .navigationBarTitleDisplayMode(.inline)
            #endif
            .toolbar {
                ToolbarItem(placement: .principal) { PhoneStatusLine(model: model, open: open) }
                ToolbarItem(placement: .primaryAction) {
                    Menu {
                        Picker("Project", selection: $model.project) {
                            Text("All projects").tag("")
                            ForEach(model.header?.projects ?? [], id: \.self) { Text($0).tag($0) }
                        }
                        .onChange(of: model.project) { _, _ in model.subscribe?() }
                        Button("Add task", action: add)
                        if !model.selfProject.isEmpty { Button("Improve the pipeline", action: improve) }
                        Toggle("Mute", isOn: $model.muted)
                    } label: { Image(systemName: "ellipsis.circle") }
                    .accessibilityLabel("Menu")
                    .accessibilityIdentifier("phone-menu")
                }
            }
    }
}

/// Shown until the first header arrives: the whole start image fitted to the screen width on a
/// plain deep-blue ground (the robot is left, the word right, so no crop), the real connection
/// status beneath it, and no progress bar. Later disconnects never bring it back.
struct PhoneStartView: View {
    let model: MonitorModel
    @ScaledMetric(relativeTo: .body) private var gap = CGFloat(MasonTokens.sizeSpacingLarge)

    var body: some View {
        VStack(spacing: gap) {
            if let img = Mascot.brand("start_image") {
                img.resizable().scaledToFit().frame(maxWidth: .infinity).accessibilityHidden(true)
            }
            Text(status).foregroundStyle(.white).accessibilityIdentifier("start-status")
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Color(red: 2 / 255, green: 46 / 255, blue: 134 / 255).ignoresSafeArea())
    }

    private var status: String {
        if let link = model.linkStatus { return link }
        if model.connected { return "Connecting…" }
        return model.reconnecting ? "Daemon restarting, reconnecting…" : "Looking for the daemon…"
    }
}

/// A task state's mascot image on the phone: 20 pt in tiles and section headers, 24 pt trailing
/// on rows, growing with Dynamic Type. Decorative, never tinted; an unknown state draws nothing.
struct PhoneStateIcon: View {
    let state: String
    @ScaledMetric private var small = CGFloat(MasonTokens.sizeMascotIconSmall)
    @ScaledMetric private var large = CGFloat(MasonTokens.sizeMascotIcon)
    var trailing = false

    init(state: String, trailing: Bool = false) {
        self.state = state
        self.trailing = trailing
    }

    var body: some View {
        if let image = Mascot.icon(state, size: trailing ? MasonTokens.sizeMascotIcon : MasonTokens.sizeMascotIconSmall) {
            let side = trailing ? large : small
            image.resizable().frame(width: side, height: side).accessibilityHidden(true)
        }
    }
}

/// A row of the sidebar that pushes a page: a label, its icon and a chevron.
struct PhoneNavRow: View {
    let title: String
    let icon: String
    let id: String
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack {
                Label(title, systemImage: icon)
                Spacer(minLength: 0)
                Image(systemName: "chevron.right").font(.footnote).foregroundStyle(.secondary).accessibilityHidden(true)
            }
        }
        .accessibilityIdentifier(id)
    }
}

/// One task in a section: the key and title (wrapping, never cut), its activity and why it
/// waits beneath, and the state icon at the end.
struct PhoneTaskRow: View {
    let row: TaskRow
    /// When set, the wait line's task numbers are links (the row then is not a Button: a link
    /// inside a button's label cannot be clicked on its own).
    var model: MonitorModel? = nil

    var body: some View {
        HStack(alignment: .top) {
            VStack(alignment: .leading, spacing: 2) {
                Text("\(row.key) \(row.title)").fixedSize(horizontal: false, vertical: true)
                if !row.activity.isEmpty {
                    if let model {
                        LinkedText(model: model, text: row.activity, font: .caption, color: .secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    } else {
                        Text(row.activity).font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                    }
                }
                if row.hasWait {
                    Text(QueueWait.badge(row.wait.cause)).font(.caption.weight(.semibold)).foregroundStyle(.orange)
                        .accessibilityIdentifier("wait:badge")
                }
                if !row.whyWaiting.isEmpty {
                    if let model {
                        LinkedText(model: model, text: row.whyWaiting, font: .caption, color: .secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    } else {
                        // A bare row (no model): previews and layout tests.
                        Text(row.whyWaiting).font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                    }
                }
            }
            Spacer(minLength: 0)
            PhoneStateIcon(state: row.state, trailing: true)
        }
    }
}

struct PhoneSidebarScreen: View {
    @Environment(\.theme) var theme
    @Bindable var model: MonitorModel
    let choose: (Page) -> Void

    var body: some View {
        ScrollViewReader { proxy in
            List {
                // PhoneLayout.sidebarOrder: tiles, Home, sections, Landing, Usage; then morePages.
                Section {
                    ScrollView(.horizontal, showsIndicators: false) {
                        HStack(spacing: 8) {
                            ForEach(model.header?.counts ?? [], id: \.state) { c in tile(c, proxy: proxy) }
                        }
                        .padding(.vertical, 4)
                    }
                    .listRowInsets(EdgeInsets(top: 4, leading: 12, bottom: 4, trailing: 12))
                }
                PhoneNavRow(title: "Home", icon: "house", id: "sidebar:home") { choose(.home) }
                ForEach(model.sidebar?.sections ?? [], id: \.state) { sec in
                    Section {
                        ForEach(model.filteredRows(sec), id: \.id) { r in
                            PhoneTaskRow(row: r, model: model)
                                .contentShape(Rectangle())
                                .onTapGesture { choose(.task(r.id)) }
                                .accessibilityAddTraits(.isButton)
                                .accessibilityIdentifier("row:\(r.id)")
                        }
                    } header: {
                        HStack(spacing: 4) {
                            PhoneStateIcon(state: sec.state)
                            Text("\(sec.title) (\(model.countText(sec)))").fixedSize(horizontal: false, vertical: true)
                            Spacer(minLength: 0)
                            if let page = Page.sectionPage(sec.state) {
                                Button { choose(page) } label: { Image(systemName: "chevron.right") }
                                    .accessibilityLabel("Open \(sec.title)")
                                    .accessibilityIdentifier("section-open:\(sec.state)")
                            }
                        }
                        .id(sec.state)
                    }
                }
                PhoneNavRow(title: "Landing", icon: "arrow.down.to.line", id: "sidebar:landing") { choose(.landing) }
                PhoneNavRow(title: "LLM usage", icon: "gauge", id: "sidebar:usage") { choose(.usage) }
                ForEach(PhoneLayout.morePages, id: \.self) { page in
                    PhoneNavRow(title: phoneTitle(page, model: model), icon: page == .health ? "heart.text.square" : "gearshape",
                                id: "sidebar:\(page == .health ? "health" : "settings")") { choose(page) }
                }
                Toggle("Mute", isOn: $model.muted).accessibilityIdentifier("sidebar:mute")
            }
        }
    }

    /// A count tile: a tap pushes the state's page (or its one task); a state without a page
    /// scrolls to its section.
    private func tile(_ c: StateCount, proxy: ScrollViewProxy) -> some View {
        let wants = c.wantsYou && model.connected
        return Button {
            let rows = model.sectionRows(c.state).map(\.id)
            if let page = PhoneLayout.tileTarget(state: c.state, rows: rows, connected: model.connected) {
                choose(page)
            } else {
                model.reveal(state: c.state)
                proxy.scrollTo(c.state, anchor: .top)
            }
        } label: {
            VStack(spacing: 2) {
                PhoneStateIcon(state: c.state)
                Text(model.tileCount(c)).font(.title2.bold())
                    .foregroundStyle(wants ? theme.attention : theme.text)
                Text(model.tileLabel(c)).font(.caption).foregroundStyle(.secondary).lineLimit(1).fixedSize()
            }
            .padding(.horizontal, 10).padding(.vertical, 6)
            .background(RoundedRectangle(cornerRadius: 8).fill(wants ? theme.attentionTint : theme.surface))
            .overlay(RoundedRectangle(cornerRadius: 8).stroke(wants ? theme.attention : theme.border, lineWidth: 1))
        }
        .buttonStyle(.plain)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(model.tileCount(c)) \(model.tileLabel(c))")
        .accessibilityAddTraits(.isButton)
        .accessibilityIdentifier("tile:\(c.state)")
    }
}
