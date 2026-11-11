// SPDX-License-Identifier: GPL-3.0-or-later

import MasonMascot
import MasonTokens
import MonitorCore
import SwiftUI

/// The phone's screens (Part 17). The sidebar is the first screen: count tiles scroll sideways
/// above a Home row, then the state sections, Landing and Usage. A row pushes its detail.
/// This target has no AppKit, so the iOS app builds it; the Mac monitor uses `MonitorUI`.
public struct PhoneRootView: View {
    @ObservedObject var model: MonitorModel
    @State private var path: [Page] = []

    public init(model: MonitorModel) { self.model = model }

    public var body: some View {
        if model.header == nil {
            PhoneStartView(model: model)
        } else {
            stack
        }
    }

    private var stack: some View {
        NavigationStack(path: $path) {
            SidebarScreen(model: model) { page in
                model.go(page)
                path.append(page)
            }
            .navigationDestination(for: Page.self) { page in
                DetailScreen(model: model, page: page)
            }
            .navigationTitle("MASON")
        }
    }
}

<<<<<<< HEAD
/// Shown until the first header arrives: the whole start image fitted to the screen width on a
/// plain deep-blue ground (the robot is left, the word right, so no crop), the real connection
/// status beneath it, and no progress bar. Later disconnects never bring it back.
struct PhoneStartView: View {
    @ObservedObject var model: MonitorModel
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
        if model.connected { return "Connecting…" }
        return model.reconnecting ? "Daemon restarting, reconnecting…" : "Looking for the daemon…"
=======
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
>>>>>>> origin/develop
    }
}

struct SidebarScreen: View {
    @ObservedObject var model: MonitorModel
    let choose: (Page) -> Void

    var body: some View {
        List {
            // PhoneLayout.sidebarOrder: tiles, Home, sections, Landing, Usage.
            Section {
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack {
                        ForEach(model.header?.counts ?? [], id: \.state) { c in
                            VStack {
                                PhoneStateIcon(state: c.state)
                                Text(model.tileCount(c)).font(.title2.bold())
                                Text(model.tileLabel(c)).font(.caption)
                            }
                            .padding(8)
                            .accessibilityIdentifier("tile:\(c.state)")
                        }
                    }
                }
            }
            Button { choose(.home) } label: { Label("Home", systemImage: "house") }
                .accessibilityIdentifier("sidebar:home")
            ForEach(model.sidebar?.sections ?? [], id: \.state) { sec in
                Section {
                    ForEach(model.filteredRows(sec), id: \.id) { r in
                        Button { choose(.task(r.id)) } label: {
                            HStack {
                                VStack(alignment: .leading) {
                                    Text("\(r.key) \(r.title)")
                                    if !r.activity.isEmpty { Text(r.activity).font(.caption).foregroundStyle(.secondary) }
                                }
                                Spacer(minLength: 0)
                                PhoneStateIcon(state: r.state, trailing: true)
                            }
                        }
                        .accessibilityIdentifier("row:\(r.id)")
                    }
                } header: {
                    HStack(spacing: 4) {
                        PhoneStateIcon(state: sec.state)
                        Text("\(sec.title) (\(model.visibleCount(sec)))")
                    }
                }
            }
            Button { choose(.landing) } label: { Label("Landing", systemImage: "arrow.down.to.line") }
            Button { choose(.usage) } label: { Label("LLM usage", systemImage: "gauge") }
            Toggle("Mute", isOn: $model.muted)
        }
    }
}

struct DetailScreen: View {
    @ObservedObject var model: MonitorModel
    let page: Page

    var body: some View {
        switch page {
        case .task(let id): TaskScreen(model: model, id: id)
        case .landing: LandingScreen(model: model)
        case .usage: UsageScreen(model: model)
        default: HomeScreen(model: model)
        }
    }
}

struct HomeScreen: View {
    @ObservedObject var model: MonitorModel
    var body: some View {
        List {
            Section("Needs you") {
                ForEach(model.home?.attention ?? [], id: \.task.id) { a in
                    Text("\(a.task.key) \(a.task.title)")
                }
            }
        }
        .navigationTitle("Home")
    }
}

struct TaskScreen: View {
    @ObservedObject var model: MonitorModel
    let id: UInt64

    var body: some View {
        let t = model.taskDetail
        let row = t?.row
        let allowed = row?.allowedActions ?? []
        List {
            if let r = row {
                Section { Text(r.title).font(.headline); Text(r.activity) }
            }
            if let card = t?.card, !card.ask.isEmpty {
                Section("Decision") {
                    Text(card.ask)
                    ForEach(card.options, id: \.label) { o in
                        Button(o.label) { model.answer(task: id, o.label, cardHash: card.hash) }
                            .disabled(!model.canAct)
                    }
                }
            }
            Section("Actions") {
                if allowed.contains("cancel") { Button("Cancel", role: .destructive) { model.cancel(id) } }
                if allowed.contains("land_now") { Button("Land now") { model.landNow(id) } }
                if allowed.contains("skip") { Button("Skip test") { model.skipTest(id, step: row?.step ?? "") } }
            }
            .disabled(!model.canAct)
            if !model.canAct {
                Text("Viewing only: this device is paired as a viewer.").font(.footnote)
            }
        }
        .navigationTitle(row?.key ?? "Task")
    }
}

struct LandingScreen: View {
    @ObservedObject var model: MonitorModel
    var body: some View {
        List {
            ForEach(model.landing?.queue ?? [], id: \.id) { r in Text("\(r.key) \(r.title)") }
            ForEach(model.landing?.modes ?? [], id: \.self) { Text($0) }
        }
        .navigationTitle("Landing")
    }
}

struct UsageScreen: View {
    @ObservedObject var model: MonitorModel
    var body: some View {
        List { ForEach(model.usage?.topBar ?? [], id: \.self) { Text($0) } }
            .navigationTitle("LLM usage")
    }
}
