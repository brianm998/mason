// SPDX-License-Identifier: GPL-3.0-or-later

import MonitorCore
import SwiftUI

/// The top bar, on every page: back, forward, home; the attention indicator
/// (only while something needs a look); the project filter; LLM usage; one
/// count tile per state; add task; mute.
struct TopBar: View {
    @Environment(\.theme) var theme
    @ObservedObject var model: MonitorModel
    @State private var adding = false

    var body: some View {
        HStack(spacing: theme.spacing) {
            HStack(spacing: theme.spacingSmall) {
                Button { model.goBack() } label: { Image(systemName: "chevron.left") }
                    .keyboardShortcut("[", modifiers: .command).disabled(!model.canGoBack).help("Back (⌘[)")
                    .accessibilityIdentifier("nav:back")
                Button { model.goForward() } label: { Image(systemName: "chevron.right") }
                    .keyboardShortcut("]", modifiers: .command).disabled(!model.canGoForward).help("Forward (⌘])")
                    .accessibilityIdentifier("nav:forward")
                Button { model.goHome() } label: { Label("Home", systemImage: "house") }
                    .help("Home").accessibilityIdentifier("nav:home")
            }
            .buttonStyle(.bordered)

            if model.attention > 0 {
                // Clicking it opens the first task that needs the owner.
                Button { model.openFirstNeedingYou() } label: {
                    HStack(spacing: theme.spacingSmall) {
                        Circle().fill(theme.attention).frame(width: theme.cell * 0.7, height: theme.cell * 0.7)
                        Text("\(model.attention) need\(model.attention == 1 ? "s" : "") you")
                            .font(theme.body(.semibold)).foregroundStyle(theme.attention)
                            .lineLimit(1).fixedSize()
                    }
                }
                .buttonStyle(.plain)
                .help("Open the first task that needs you")
                .accessibilityIdentifier("attention")
            }

            Picker("", selection: $model.project) {
                Text("All projects").tag("")
                ForEach(model.header?.projects ?? [], id: \.self) { Text($0).tag($0) }
            }
            .labelsHidden()
            .frame(width: 150 * min(CGFloat(theme.scale), 1.4))
            .onChange(of: model.project) { _, _ in model.subscribe?() }

            VStack(alignment: .leading, spacing: 0) {
                ForEach(model.usage?.providers ?? [], id: \.name) { p in
                    Text("\(p.name): \(p.sessionsUsed) of \(p.sessionsLimit) sessions")
                        .font(theme.small()).foregroundStyle(theme.muted).lineLimit(1)
                }
                if let note = model.usage?.note, (model.usage?.providers.isEmpty ?? true) {
                    Text(note).font(theme.small()).foregroundStyle(theme.muted).lineLimit(1)
                }
            }
            .layoutPriority(-1)
            .frame(minWidth: 0)

            Spacer(minLength: theme.spacing)

            // Tiles are fixed size: a narrow window squeezes the text above, not these.
            HStack(spacing: theme.spacingSmall) {
                ForEach(model.header?.counts ?? [], id: \.state) { CountTile(model: model, count: $0) }
            }

            Button { adding = true } label: { Image(systemName: "plus") }
                .buttonStyle(.bordered).help("Add task").accessibilityIdentifier("add-task")
            Button { model.muted.toggle() } label: { Image(systemName: model.muted ? "speaker.slash" : "speaker.wave.2") }
                .buttonStyle(.bordered).help(model.muted ? "Unmute" : "Mute").accessibilityIdentifier("mute")
        }
        .font(theme.body())
        .padding(theme.spacing)
        .fixedSize(horizontal: false, vertical: true)
        .background(theme.surface)
        .sheet(isPresented: $adding) { AddTaskSheet(model: model, isPresented: $adding) }
    }
}

struct BannersView: View {
    @Environment(\.theme) var theme
    @ObservedObject var model: MonitorModel

    var body: some View {
        VStack(spacing: 0) {
            if !model.connected {
                banner("Connection lost. Reconnecting…", id: "connection")
            }
            if let why = model.readOnlyReason {
                banner("Read-only: \(why)", id: "read-only")
            }
            if model.updateAvailable || model.updating != nil {
                HStack(spacing: theme.spacing) {
                    Text(model.updating ?? "A newer monitor is available (\(String(model.daemonVersion.prefix(8)))).")
                        .font(theme.body(.medium))
                    Spacer()
                    if model.updating == nil {
                        Button("Update") { model.onUpdate?() }.accessibilityIdentifier("monitor-update")
                    }
                }
                .padding(.horizontal, theme.spacingLarge).padding(.vertical, theme.spacing)
                .background(theme.slow.opacity(0.18))
                .accessibilityIdentifier("monitor-update-banner")
            }
            ForEach(Array((model.header?.banners ?? []).enumerated()), id: \.offset) { i, b in
                banner(b.text, id: "banner:\(i)")
            }
        }
    }

    func banner(_ text: String, id: String) -> some View {
        Text(text)
            .font(theme.body(.medium))
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, theme.spacingLarge).padding(.vertical, theme.spacing)
            .background(theme.slow.opacity(0.18))
            .accessibilityIdentifier(id)
    }
}

struct AddTaskSheet: View {
    @Environment(\.theme) var theme
    @ObservedObject var model: MonitorModel
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
        .frame(width: 460)
    }
}
