// SPDX-License-Identifier: GPL-3.0-or-later

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
                Section("\(sec.title) (\(model.countText(sec)))") {
                    ForEach(model.filteredRows(sec), id: \.id) { r in
                        Button { choose(.task(r.id)) } label: {
                            VStack(alignment: .leading) {
                                Text("\(r.key) \(r.title)")
                                if !r.activity.isEmpty { Text(r.activity).font(.caption).foregroundStyle(.secondary) }
                            }
                        }
                        .accessibilityIdentifier("row:\(r.id)")
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
