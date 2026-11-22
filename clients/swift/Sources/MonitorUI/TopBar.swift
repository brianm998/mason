// SPDX-License-Identifier: GPL-3.0-or-later

import MasonMascot
import MasonProtocol
import MonitorCore
import SwiftUI

/// The top bar, on every page: home over back and forward; the attention indicator
/// (only while something needs a look) over the project filter; LLM usage (5-hour
/// over weekly); one count tile per state; a 2x2 grid of add task, improve, mute, voice.
struct TopBar: View {
    @Environment(\.theme) var theme
    @Bindable var model: MonitorModel
    @State private var adding = false
    @State private var improving = false
    @State private var voices = false

    #if DEBUG
    /// Body evaluations, for the offscreen test that counts them.
    nonisolated(unsafe) static var evaluations = 0
    #endif

    var body: some View {
        #if DEBUG
        let _ = { Self.evaluations += 1 }()
        #endif
        HStack(spacing: theme.spacing) {
            topBarMascot
            // Home on top; back and forward in a row under it.
            VStack(alignment: .leading, spacing: theme.spacingSmall) {
                Button { model.goHome() } label: { Label("Home", systemImage: "house") }
                    .touchTarget().hint("Home").accessibilityIdentifier("nav:home")
                HStack(spacing: theme.spacingSmall) {
                    Button { model.goBack() } label: { Image(systemName: "chevron.left") }
                        .keyboardShortcut("[", modifiers: .command).disabled(!model.canGoBack).touchTarget().hint("Back (⌘[)")
                        .accessibilityIdentifier("nav:back")
                    Button { model.goForward() } label: { Image(systemName: "chevron.right") }
                        .keyboardShortcut("]", modifiers: .command).disabled(!model.canGoForward).touchTarget().hint("Forward (⌘])")
                        .accessibilityIdentifier("nav:forward")
                }
            }
            .buttonStyle(.bordered)

            // The indicator (only while something needs a look) over the project filter; the
            // picker stands alone, centred, when nothing does.
            VStack(alignment: .leading, spacing: theme.spacingSmall) {
                if model.attention > 0 {
                    // Clicking it opens the first task that needs the owner.
                    Button { model.openFirstNeedingYou() } label: {
                        HStack(spacing: theme.spacingSmall) {
                            StateIcon(state: "needs_attention")
                            Text("\(model.attention) need\(model.attention == 1 ? "s" : "") you")
                                .font(theme.body(.semibold)).foregroundStyle(theme.attention)
                                .lineLimit(1).fixedSize()
                        }
                    }
                    .buttonStyle(.plain)
                    .touchTarget().hint("Open the first task that needs you")
                    .accessibilityIdentifier("attention")
                }

                Picker("", selection: $model.project) {
                    Text("All projects").tag("")
                    ForEach(model.header?.projects ?? [], id: \.self) { Text($0).tag($0) }
                }
                .labelsHidden()
                .frame(width: 120 * min(CGFloat(theme.scale), 1.4))
                .onChange(of: model.project) { _, _ in model.subscribe?() }
            }

            TopBarUsage(model: model)
            .layoutPriority(-1)
            .frame(minWidth: 0)

            Spacer(minLength: theme.spacing)

            // Tiles are fixed size: a narrow window squeezes the text above, not these.
            HStack(spacing: theme.spacingSmall) {
                ForEach(model.header?.counts ?? [], id: \.state) { CountTile(model: model, count: $0) }
                if let n = model.header?.pendingIssues.count, n > 0 { PendingTile(count: n) }
            }

            // A 2x2 grid: add task and improve on top, mute and voice under. With no self
            // project the wand's cell stays empty so the others keep their places.
            Grid(horizontalSpacing: theme.spacingSmall, verticalSpacing: theme.spacingSmall) {
                GridRow {
                    Button { adding = true } label: { Image(systemName: "plus") }
                        .touchTarget().hint("Add task").accessibilityIdentifier("add-task")
                    if !model.selfProject.isEmpty {
                        Button { improving = true } label: { Image(systemName: "wand.and.stars") }
                            .touchTarget().hint("Improve the pipeline").accessibilityIdentifier("improve-pipeline")
                    } else {
                        Color.clear.frame(width: 1, height: 1)
                    }
                }
                GridRow {
                    Button { model.muted.toggle() } label: { Image(systemName: model.muted ? "speaker.slash" : "speaker.wave.2") }
                        .touchTarget().hint(model.muted ? "Unmute" : "Mute").accessibilityIdentifier("mute")
                    Button { voices = true } label: { Image(systemName: "waveform") }
                        .touchTarget().hint("Sounds and voice").accessibilityIdentifier("voice-settings")
                        .popover(isPresented: $voices) { SpeechSettingsView(model: model) }
                }
            }
            .buttonStyle(.bordered)
        }
        .font(theme.body())
        .padding(theme.spacing)
        .fixedSize(horizontal: false, vertical: true)
        .background(theme.surface)
        .sheet(isPresented: $adding) { AddTaskSheet(model: model, isPresented: $adding) }
        .sheet(isPresented: $improving) { ImproveSheet(model: model, isPresented: $improving) }
    }
}

extension TopBar {
    /// The picture for the state holding the most tasks: the task page's art at 85% of its
    /// size (never taller than the bar's tile row pair), aspect kept. Decorative, but a button.
    @ViewBuilder var topBarMascot: some View {
        if let choice = model.topBarMascot, let img = Mascot.illustration(choice.id) {
            let side = topBarMascotSide(illustration: theme.illustration, bar: theme.tileHeight * 2 + theme.spacingSmall)
            Button { model.topBarMascotClicked() } label: {
                img.resizable().interpolation(.high).aspectRatio(contentMode: .fit)
                    .frame(minWidth: 0, idealWidth: side, maxWidth: side, minHeight: 0, idealHeight: side, maxHeight: side)
            }
            .layoutPriority(-2)
            .buttonStyle(.plain)
            .touchTarget().hint(model.topBarMascotTooltip)
            .accessibilityLabel(model.topBarMascotTooltip)
            .accessibilityIdentifier("topbar-mascot:\(choice.id)")
        }
    }
}

/// 85% of the illustration size, or the bar's height if that is less.
func topBarMascotSide(illustration: CGFloat, bar: CGFloat) -> CGFloat { min(illustration * 0.85, bar) }

/// The measured heights of the rows (by key) and of the show-hidden button.
private struct RowHeights: PreferenceKey {
    static var defaultValue: [String: CGFloat] = [:]
    static func reduce(value: inout [String: CGFloat], nextValue: () -> [String: CGFloat]) { value.merge(nextValue()) { $1 } }
}
private struct ButtonWidth: PreferenceKey {
    static var defaultValue: CGFloat = 0
    static func reduce(value: inout CGFloat, nextValue: () -> CGFloat) { value = max(value, nextValue()) }
}

/// Where the parts of the bar are, by accessibility id, for tests that render it offscreen.
private struct Probe: PreferenceKey {
    static var defaultValue: [String: CGRect] = [:]
    static func reduce(value: inout [String: CGRect], nextValue: () -> [String: CGRect]) { value.merge(nextValue()) { $1 } }
}

private extension View {
    func probe(_ id: String) -> some View {
        background(GeometryReader { g in Color.clear.preference(key: Probe.self, value: [id: g.frame(in: .global)]) })
    }
}

struct BannersView: View {
    @Environment(\.theme) var theme
    let model: MonitorModel
    /// The window's height, for the cap on the scrolling bar.
    var windowHeight: CGFloat = 800
    /// Called with the frames of the bar's parts (tests only).
    var onProbe: (([String: CGRect]) -> Void)?
    @State private var heights: [String: CGFloat] = [:]
    @State private var buttonWidth: CGFloat = 0
    @State private var dragStart: CGFloat?

    var body: some View {
        bar
            .sheet(isPresented: Binding(get: { model.sshKeyProject != nil }, set: { if !$0 { model.closeSSHKeys() } })) {
                SSHKeySheet(model: model)
            }
            .sheet(isPresented: Binding(get: { model.ownerSessionDialog != nil }, set: { if !$0 { model.closeOwnerSession() } })) {
                OwnerSessionSheet(model: model)
            }
            .alert("Update masond now?", isPresented: Binding(get: { model.confirmingDaemonUpdate }, set: { if !$0 { model.cancelDaemonUpdate() } })) {
                Button("Update now") { model.confirmDaemonUpdate() }
                Button("Keep waiting", role: .cancel) { model.cancelDaemonUpdate() }
            } message: {
                Text(model.daemonUpdateConfirm)
            }
    }

    private func rowHeight(_ r: NotificationRow) -> CGFloat { heights[r.key] ?? 0 }

    /// The chosen height, kept between one row and all rows (and the window's share).
    private func scrollHeight(_ rows: [NotificationRow]) -> CGFloat {
        // The 1 pt dividers between rows count toward the heights.
        let all = rows.map(rowHeight)
        let shown = rows.prefix(MonitorModel.visibleNotificationRows).map(rowHeight)
        let first = shown.reduce(0, +) + CGFloat(max(shown.count - 1, 0))
        let total = all.reduce(0, +) + CGFloat(max(all.count - 1, 0))
        let want = model.notificationHeight.map { CGFloat($0) } ?? first
        return CGFloat(MonitorModel.clampNotificationHeight(Double(want), oneRow: Double(all.first ?? 0),
                                                            total: Double(total), window: Double(windowHeight)))
    }

    var bar: some View {
        let rows = model.notificationRows()
        let hidden = model.hiddenNotificationCount()
        return ZStack(alignment: .topTrailing) {
            Group {
                if rows.count > MonitorModel.visibleNotificationRows {
                    VStack(spacing: 0) {
                        ScrollView(.vertical) { stack(rows, hidden: hidden) }
                            .frame(height: scrollHeight(rows))
                            .accessibilityIdentifier("notifications-scroll")
                            .probe("notifications-scroll")
                        resizeHandle(rows)
                    }
                } else {
                    stack(rows, hidden: hidden)
                }
            }
            if hidden > 0 {
                Button("Show hidden notifications (\(hidden))") { model.showHiddenNotifications() }
                    .buttonStyle(.borderless)
                    .font(theme.small())
                    .padding(.horizontal, theme.spacing).padding(.vertical, theme.spacingSmall)
                    .fixedSize()
                    .background(GeometryReader { Color.clear.preference(key: ButtonWidth.self, value: $0.size.width) })
                    .accessibilityIdentifier("show-hidden-notifications")
                    .probe("show-hidden-notifications")
            }
        }
        .onPreferenceChange(RowHeights.self) { heights = $0 }
        .onPreferenceChange(ButtonWidth.self) { buttonWidth = $0 }
        .onPreferenceChange(Probe.self) { onProbe?($0) }
    }

    private func stack(_ rows: [NotificationRow], hidden: Int) -> some View {
        VStack(spacing: 0) {
            ForEach(Array(rows.enumerated()), id: \.element.key) { i, row in
                // A 1 pt line between neighbouring rows, none above the first or below the last.
                if i > 0 {
                    Rectangle().fill(theme.border).frame(height: 1)
                        .accessibilityHidden(true)
                        .probe("divider:\(i)")
                }
                rowView(row, inset: i == 0 && hidden > 0 ? buttonWidth : 0)
                    .background(GeometryReader { Color.clear.preference(key: RowHeights.self, value: [row.key: $0.size.height]) })
            }
        }
    }

    private func resizeHandle(_ rows: [NotificationRow]) -> some View {
        let step = rows.first.map(rowHeight) ?? 20
        return Capsule().fill(theme.muted.opacity(0.6)).frame(width: 36, height: 3)
            .frame(maxWidth: .infinity).frame(height: 10)
            .background(theme.surface)
            .contentShape(Rectangle())
            #if canImport(AppKit)
            .onHover { inside in if inside { NSCursor.resizeUpDown.push() } else { NSCursor.pop() } }
            #endif
            .gesture(DragGesture(minimumDistance: 1)
                .onChanged { g in
                    if dragStart == nil { dragStart = scrollHeight(rows) }
                    set(height: (dragStart ?? 0) + g.translation.height, rows: rows)
                }
                .onEnded { _ in dragStart = nil })
            .accessibilityElement()
            .accessibilityLabel("Resize notifications")
            .accessibilityValue("\(Int(scrollHeight(rows))) points")
            .accessibilityAdjustableAction { direction in
                switch direction {
                case .increment: set(height: scrollHeight(rows) + step, rows: rows)
                case .decrement: set(height: scrollHeight(rows) - step, rows: rows)
                @unknown default: break
                }
            }
            .accessibilityIdentifier("notifications-resize")
            .probe("notifications-resize")
    }

    private func set(height: CGFloat, rows: [NotificationRow]) {
        let all = rows.map(rowHeight)
        model.setNotificationHeight(MonitorModel.clampNotificationHeight(
            Double(height), oneRow: Double(all.first ?? 0), total: Double(all.reduce(0, +)) + Double(max(all.count - 1, 0)), window: Double(windowHeight)))
    }

    /// One row: its content, then the dismiss X; `inset` keeps it clear of the show-hidden button.
    func rowView(_ row: NotificationRow, inset: CGFloat) -> some View {
        let pending = row.kind == .pending
        return HStack(spacing: theme.spacing) {
            content(row).frame(maxWidth: .infinity, alignment: .leading)
            Button { model.dismissNotification(row.key) } label: { Image(systemName: "xmark") }
                .buttonStyle(.borderless)
                .touchTarget().hint("Dismiss")
                .accessibilityLabel("Dismiss")
                .accessibilityIdentifier("dismiss:\(row.key)")
                .probe("dismiss:\(row.key)")
        }
        .padding(.leading, theme.spacingLarge).padding(.trailing, theme.spacingLarge + inset)
        .padding(.vertical, pending ? theme.spacingSmall : theme.spacing)
        .background(theme.slow.opacity(pending ? 0.12 : 0.18))
    }

    @ViewBuilder func content(_ row: NotificationRow) -> some View {
        switch row.kind {
        case .pending:
            if let p = (model.header?.pendingIssues ?? []).first(where: { Int($0.number) == row.issue }) {
                PendingIssueRow(issue: p)
            }
        case .paused:
            pauseContent(row)
        default:
            textRow(row)
        }
    }

    @ViewBuilder func textRow(_ row: NotificationRow) -> some View {
        HStack(spacing: theme.spacing) {
            bannerImage(row.daemonKind.flatMap { BannerIcons.id(forDaemonKind: $0) } ?? row.icon)
            Text(row.text).font(theme.body(.medium)).frame(maxWidth: .infinity, alignment: .leading)
                .accessibilityIdentifier(row.uiID + "-text")
                .probe(row.uiID + "-text")
            if let button = row.button { Button(button) { press(row) }.accessibilityIdentifier(row.uiID) }
        }
        .accessibilityIdentifier(row.button == nil ? row.uiID : row.uiID + "-banner")
    }

    private func press(_ row: NotificationRow) {
        let b = row.bannerIndex.flatMap { i in (model.header?.banners ?? []).indices.contains(i) ? model.header?.banners[i] : nil }
        switch row.kind {
        case .push: if let b { model.chooseSSHKey(project: MonitorModel.pushAuthProject(b)) }
        case .health: if let b { model.openOwnerSession(b.session) }
        default:
            if row.clientID == "daemon-update" {
                if model.downloadURL.isEmpty { model.pressUpdateDaemon() } else { model.openDownloadPage() }
            } else if row.clientID == "monitor-update" {
                if model.canCancelMonitorUpdate { model.cancelMonitorUpdate() } else { model.updateMonitor() }
            }
        }
    }

    /// A pause row: collapsed ones are icons, the expanded one a full banner with Resume and Collapse.
    func pauseContent(_ row: NotificationRow) -> some View {
        let banners = model.header?.banners ?? []
        return HStack(spacing: theme.spacing) {
            ForEach(row.collapsed.filter { banners.indices.contains($0) }, id: \.self) { i in collapsedPauseIcon(banners[i], index: i) }
            if let e = row.bannerIndex, banners.indices.contains(e) { expandedPause(banners[e], index: e) } else { Spacer(minLength: 0) }
        }
        .accessibilityIdentifier(row.uiID)
    }

    func collapsedPauseIcon(_ b: Mason_V1_Banner, index: Int) -> some View {
        bannerImage(BannerIcons.id(forDaemonKind: b.kind), hidden: false)
            .touchTarget().hint(b.text)
            .accessibilityLabel(b.text)
            .accessibilityAddTraits(.isButton)
            .accessibilityAction(named: "Expand") { model.setPauseCollapsed(project: b.project, mode: b.mode, false) }
            // A double click (double tap on touch) expands; one click does nothing, so the layout stays put.
            .onTapGesture(count: 2) { model.setPauseCollapsed(project: b.project, mode: b.mode, false) }
            .accessibilityIdentifier("paused-icon:\(index)")
    }

    func expandedPause(_ b: Mason_V1_Banner, index: Int) -> some View {
        HStack(spacing: theme.spacing) {
            bannerImage(BannerIcons.id(forDaemonKind: b.kind))
            Text(b.text).font(theme.body(.medium))
            Spacer()
            if model.canAct {
                Button("Resume") { model.resumePause(project: b.project, mode: b.mode) }
                    .accessibilityIdentifier("resume-pause:\(index)")
            }
            Button { model.setPauseCollapsed(project: b.project, mode: b.mode, true) } label: { Image(systemName: "chevron.up") }
                .buttonStyle(.borderless)
                .touchTarget().hint("Collapse")
                .accessibilityLabel("Collapse")
                .accessibilityIdentifier("collapse-pause:\(index)")
                .probe("collapse-pause:\(index)")
        }
        .accessibilityIdentifier("banner:\(index)")
    }

    /// The decorative image at the start of a banner; nothing for an unknown id.
    @ViewBuilder func bannerImage(_ id: String?, hidden: Bool = true) -> some View {
        if let id, let img = Mascot.bannerIcon(id) {
            img.resizable().interpolation(.high)
                .frame(width: theme.mascotBannerIcon, height: theme.mascotBannerIcon)
                .accessibilityHidden(hidden)
        }
    }
}

struct AddTaskSheet: View {
    @Environment(\.theme) var theme
    let model: MonitorModel
    @Binding var isPresented: Bool
    @State private var project = ""
    @State private var title = ""
    @State private var text = ""

    var body: some View {
        VStack(alignment: .leading, spacing: theme.spacing) {
            Text("Add task").font(theme.title())
            Picker("Project", selection: $project) {
                ForEach(model.header?.projects ?? [], id: \.self) { Text($0).tag($0) }
            }
            TextField("Title", text: $title).textFieldStyle(.roundedBorder)
            TextEditor(text: $text)
                .font(theme.body())
                .frame(minHeight: 120)
                .overlay(RoundedRectangle(cornerRadius: theme.radius).stroke(theme.border))
            HStack {
                Spacer()
                Button("Cancel") { isPresented = false }
                Button("Add") {
                    model.addTask(project: project.isEmpty ? (model.header?.projects.first ?? "") : project, title: title, body: text)
                    isPresented = false
                }
                .keyboardShortcut(.defaultAction)
                .disabled(title.trimmingCharacters(in: .whitespaces).isEmpty)
            }
        }
        .padding(theme.spacingLarge)
        .sheetWidth(460)
    }
}

/// How many open issues the tracker has not imported; the list is below the top bar.
struct PendingTile: View {
    @Environment(\.theme) var theme
    let count: Int

    var body: some View {
        VStack(spacing: 0) {
            Text(String(count)).font(theme.count()).foregroundStyle(theme.attention)
            Text("not imported").font(theme.small()).foregroundStyle(theme.muted).lineLimit(1).fixedSize()
        }
        .padding(.horizontal, theme.spacing)
        .frame(minWidth: theme.tileMinWidth, minHeight: theme.tileHeight, maxHeight: theme.tileHeight)
        .fixedSize(horizontal: true, vertical: false)
        .background(RoundedRectangle(cornerRadius: theme.radius).fill(theme.attentionTint))
        .overlay(RoundedRectangle(cornerRadius: theme.radius).stroke(theme.attention, lineWidth: 1))
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(count) not imported")
        .accessibilityIdentifier("tile:not_imported")
    }
}

/// One issue the tracker did not import, with the reason.
struct PendingIssueRow: View {
    @Environment(\.theme) var theme
    let issue: Mason_V1_PendingIssue

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: theme.spacing) {
            if let url = URL(string: issue.url), !issue.url.isEmpty {
                WebLink(title: "#\(issue.number)", url: url)
            } else {
                Text("#\(issue.number)")
            }
            Text(issue.title).lineLimit(1)
            Text(issue.reason).foregroundStyle(theme.muted)
            Spacer()
        }
        .font(theme.small())
        .accessibilityIdentifier("pending:\(issue.project):\(issue.number)")
    }
}

/// The top-bar lines for an account, in the order received (5-hour, then weekly:
/// "claude 5-hour 82%, resets 1 h 10 m"); its sessions when it has reported none.
func topBarUsageLines(_ u: Mason_V1_UsageView?, _ p: Mason_V1_ProviderStatus) -> [String] {
    let lines = (u?.topBar ?? []).filter { $0.hasPrefix("\(p.name) ") }
    return lines.isEmpty ? ["\(p.name): \(p.sessionsUsed) of \(p.sessionsLimit) sessions"] : lines
}

/// The usage lines in the top bar, a view of their own so a new usage view redraws these and not the bar.
struct TopBarUsage: View {
    @Environment(\.theme) var theme
    let model: MonitorModel

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            ForEach(model.usage?.providers ?? [], id: \.name) { p in
                // 5-hour above weekly; the first line keeps the account's id, the rest add a number.
                ForEach(Array(topBarUsageLines(model.usage, p).enumerated()), id: \.offset) { i, line in
                    Text(line)
                        .font(theme.small()).foregroundStyle(theme.muted).lineLimit(1)
                        .accessibilityIdentifier(i == 0 ? "topbar-usage:\(p.name)" : "topbar-usage:\(p.name):\(i)")
                }
            }
            if let note = model.usage?.note, (model.usage?.providers.isEmpty ?? true) {
                Text(note).font(theme.small()).foregroundStyle(theme.muted).lineLimit(1)
            }
        }
    }
}
