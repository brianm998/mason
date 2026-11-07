// SPDX-License-Identifier: GPL-3.0-or-later

import MasonProtocol
import MonitorCore
import SwiftUI

/// The pipeline editor (Part 18). The daemon validates, lays out and computes everything
/// (`EditorView`); this draws it and sends small operations back. Nothing here knows a
/// pipeline rule: a step's form is built from the JSON Schema the daemon sends.
struct EditorPage: View {
    @Environment(\.theme) var theme
    @ObservedObject var model: MonitorModel
    @State private var tab = Tab.graph
    @State private var message = ""

    enum Tab: String, CaseIterable { case graph = "Graph", tests = "Tests", landing = "Landing", preview = "Preview" }

    var body: some View {
        VStack(alignment: .leading, spacing: theme.spacing) {
            if let e = model.editor, !e.project.isEmpty {
                toolbar(e)
                Picker("", selection: $tab) {
                    ForEach(Tab.allCases, id: \.self) { Text($0.rawValue).tag($0) }
                }.pickerStyle(.segmented).frame(maxWidth: 420).accessibilityIdentifier("editor-tabs")
                switch tab {
                case .graph: GraphTab(model: model, e: e)
                case .tests: TestsTab(model: model, e: e)
                case .landing: LandingTab(model: model, e: e)
                case .preview: PreviewTab(model: model, e: e)
                }
            } else {
                Text("No project to edit.").font(theme.body()).foregroundStyle(theme.muted)
            }
        }
        .padding(theme.spacingLarge)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    }

    func toolbar(_ e: Mason_V1_EditorView) -> some View {
        VStack(alignment: .leading, spacing: theme.spacingSmall) {
            HStack(spacing: theme.spacing) {
                Text("Pipeline: \(e.project)").font(theme.title())
                if e.protected { Text("MASON's own: .mason/ is protected").font(theme.body()).foregroundStyle(theme.muted) }
                Spacer()
                Button("Undo") { model.editorCall("undo") }.disabled(!e.canUndo).accessibilityIdentifier("undo")
                Button("Redo") { model.editorCall("redo") }.disabled(!e.canRedo).accessibilityIdentifier("redo")
                Button("Discard") { model.editorCall("discard") }.disabled(!e.dirty).accessibilityIdentifier("discard")
                Button("Simulate") { model.editorCall("simulate"); tab = .preview }.accessibilityIdentifier("simulate")
                TextField("Message", text: $message).frame(width: 220)
                Button("Apply") { model.editorCall("apply", ["message": message]); message = "" }
                    .disabled(!e.dirty).accessibilityIdentifier("apply")
            }
            Text(e.dirty ? "Draft changed" : "Draft is the base").font(theme.body()).foregroundStyle(theme.muted)
            ForEach(e.lastError, id: \.self) { Text("Refused: \($0)").font(theme.body()).foregroundStyle(theme.attention) }
            if let r = model.lastResult { Text(r).font(theme.body()).foregroundStyle(theme.attention) }
            ForEach(Array(e.problems.enumerated()), id: \.offset) { i, p in
                Text(p).font(theme.body()).foregroundStyle(theme.attention).accessibilityIdentifier("problem:\(i)")
            }
        }
    }
}

// MARK: the graph

private struct GraphTab: View {
    @Environment(\.theme) var theme
    @ObservedObject var model: MonitorModel
    let e: Mason_V1_EditorView
    static let nodeW: CGFloat = 170
    static let nodeH: CGFloat = 70
    static let laneH: CGFloat = 140

    var body: some View {
        HStack(alignment: .top, spacing: theme.spacingLarge) {
            GeometryReader { geo in
                ScrollView([.horizontal, .vertical]) { canvas(viewport: geo.size.width) }
            }
            .frame(minWidth: 520, maxWidth: .infinity, minHeight: 360, maxHeight: .infinity)
            .background(theme.surface)
            ScrollView {
                VStack(alignment: .leading, spacing: theme.spacingLarge) {
                    Palette(model: model, e: e)
                    if e.hasInspector { Inspector(model: model, e: e) }
                    else { Text("Select a step to edit it.").font(theme.body()).foregroundStyle(theme.muted) }
                }
            }.frame(width: 340)
        }
    }

    func horizontal(_ viewport: CGFloat) -> GraphLayout.Horizontal {
        let x = Dictionary(e.nodes.map { ($0.name, Double($0.x)) }, uniquingKeysWith: { a, _ in a })
        let back = e.edges.filter { $0.kind == "on_red" }.compactMap { ed -> GraphLayout.BackEdge? in
            guard let f = x[ed.from], let t = x[ed.to] else { return nil }
            return .init(fromX: f, toX: t, labeled: ed.bound > 0)
        }
        return GraphLayout.horizontal(nodeXs: Array(x.values), backEdges: back, viewport: Double(viewport))
    }

    func canvasSize(_ hz: GraphLayout.Horizontal) -> CGSize {
        let w = CGFloat(hz.width)
        let h = max(CGFloat(e.lanes.count) * Self.laneH, (e.nodes.map { CGFloat($0.y) }.max() ?? 0) + Self.nodeH + 40)
        return CGSize(width: w, height: h)
    }

    func center(_ n: Mason_V1_EditorNode) -> CGPoint { CGPoint(x: CGFloat(n.x) + Self.nodeW / 2, y: CGFloat(n.y) + Self.nodeH / 2) }

    func canvas(viewport: CGFloat) -> some View {
        let hz = horizontal(viewport)
        let dx = CGFloat(hz.dx)
        let canvasSize = canvasSize(hz)
        return ZStack(alignment: .topLeading) {
            ForEach(Array(e.lanes.enumerated()), id: \.offset) { i, l in
                Rectangle().fill(i % 2 == 0 ? theme.background.opacity(0.5) : Color.clear)
                    .frame(width: canvasSize.width, height: Self.laneH).offset(y: CGFloat(i) * Self.laneH)
                Text(l.title).font(theme.body(.semibold)).foregroundStyle(theme.muted)
                    .offset(x: 8, y: CGFloat(i) * Self.laneH + 4).accessibilityIdentifier("lane:\(l.category)")
            }
            Canvas { ctx, _ in
                for edge in e.edges {
                    guard let a = e.nodes.first(where: { $0.name == edge.from }), let b = e.nodes.first(where: { $0.name == edge.to }) else { continue }
                    var p = Path()
                    let back = edge.kind == "on_red"
                    let (s, t) = back
                        ? (CGPoint(x: CGFloat(a.x) + dx, y: center(a).y), CGPoint(x: CGFloat(b.x) + dx, y: center(b).y))
                        : (CGPoint(x: center(a).x + dx, y: CGFloat(a.y) + Self.nodeH), CGPoint(x: center(b).x + dx, y: CGFloat(b.y)))
                    p.move(to: s)
                    if back { p.addCurve(to: t, control1: CGPoint(x: s.x - 70, y: s.y), control2: CGPoint(x: t.x - 70, y: t.y)) }
                    else { p.addCurve(to: t, control1: CGPoint(x: s.x, y: s.y + 40), control2: CGPoint(x: t.x, y: t.y - 40)) }
                    ctx.stroke(p, with: .color(back ? theme.attention : theme.muted), style: StrokeStyle(lineWidth: 1.5, dash: back ? [5, 4] : []))
                    var arrow = Path()
                    arrow.move(to: t)
                    arrow.addLine(to: CGPoint(x: t.x - 5, y: t.y + (back ? 0 : -9)))
                    arrow.addLine(to: CGPoint(x: t.x + (back ? -9 : 5), y: t.y + (back ? 5 : -9)))
                    arrow.closeSubpath()
                    ctx.fill(arrow, with: .color(back ? theme.attention : theme.muted))
                }
            }.frame(width: canvasSize.width, height: canvasSize.height)
            ForEach(e.edges.filter { $0.kind == "on_red" && $0.bound > 0 }, id: \.self.from) { edge in
                if let a = e.nodes.first(where: { $0.name == edge.from }) {
                    Text("red, at most \(edge.bound)×").font(theme.small()).foregroundStyle(theme.attention)
                        .offset(x: CGFloat(a.x) + dx - 100, y: center(a).y - 18)
                }
            }
            ForEach(e.nodes, id: \.name) { n in node(n, dx: dx) }
        }
        .frame(width: canvasSize.width, height: canvasSize.height, alignment: .topLeading)
    }

    func node(_ n: Mason_V1_EditorNode, dx: CGFloat) -> some View {
        let selected = e.selected == n.name
        return NodeView(model: model, n: n, selected: selected)
            .offset(x: CGFloat(n.x) + dx, y: CGFloat(n.y))
    }
}

private struct NodeView: View {
    @Environment(\.theme) var theme
    @ObservedObject var model: MonitorModel
    let n: Mason_V1_EditorNode
    let selected: Bool
    @State private var drag = CGSize.zero

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack(spacing: 4) {
                Text(n.name).font(theme.body(.semibold)).lineLimit(1)
                if n.isStart { Text("start").font(theme.small()).foregroundStyle(theme.muted) }
                Spacer()
                if !n.problems.isEmpty { Text("!").font(theme.body(.bold)).foregroundStyle(theme.attention).help(n.problems.joined(separator: "\n")) }
            }
            Text(n.kind + (n.summary.isEmpty ? "" : ": " + n.summary)).font(theme.small()).foregroundStyle(theme.muted).lineLimit(1)
            ForEach(Array(n.problems.prefix(2).enumerated()), id: \.offset) { _, p in
                Text(p).font(theme.small()).foregroundStyle(theme.attention).lineLimit(1)
            }
        }
        .padding(8)
        .frame(width: 170, height: 70, alignment: .topLeading)
        .background(theme.background)
        .overlay(RoundedRectangle(cornerRadius: 8).stroke(selected ? theme.accent : (n.problems.isEmpty ? theme.muted.opacity(0.5) : theme.attention), lineWidth: selected ? 3 : 1.5))
        .offset(drag)
        .onTapGesture { model.selectStep(n.name) }
        .gesture(DragGesture(minimumDistance: 6)
            .onChanged { drag = $0.translation }
            .onEnded { v in
                drag = .zero
                model.editorOp(["op": "move", "step": n.name, "x": Int(CGFloat(n.x) + v.translation.width), "y": Int(CGFloat(n.y) + v.translation.height)])
            })
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("node:\(n.name)")
    }
}

private struct Palette: View {
    @Environment(\.theme) var theme
    @ObservedObject var model: MonitorModel
    let e: Mason_V1_EditorView

    struct Entry { var type: String; var categories: [String]; var available: Bool; var about: String; var fields: [String: Any] }

    var entries: [Entry] {
        e.palette.compactMap { s in
            guard let d = try? JSONSerialization.jsonObject(with: Data(s.utf8)) as? [String: Any], let t = d["type"] as? String else { return nil }
            return Entry(type: t, categories: d["categories"] as? [String] ?? [], available: d["available"] as? Bool ?? true, about: d["about"] as? String ?? "", fields: d["fields"] as? [String: Any] ?? [:])
        }
    }

    func freshName(_ base: String) -> String {
        var n = 1
        while e.stepNames.contains("\(base)-\(n)") { n += 1 }
        return "\(base)-\(n)"
    }

    var selectedNode: Mason_V1_EditorNode? { e.nodes.first { $0.name == e.selected } }

    func insert(_ en: Entry, extends: Bool) {
        guard let sel = selectedNode else { return }
        let laneOrder = e.lanes.map(\.category)
        let here = laneOrder.firstIndex(of: sel.category) ?? 0
        let category = en.categories.contains(sel.category) ? sel.category
            : (en.categories.first { (laneOrder.firstIndex(of: $0) ?? 0) >= here } ?? en.categories.first ?? sel.category)
        var op: [String: Any] = ["op": "insert_step", "after": sel.name, "name": freshName(extends ? sel.name : en.type), "type": extends ? sel.kind : en.type, "category": extends ? sel.category : category]
        if extends { op["extends"] = sel.name } else { op["fields"] = en.fields }
        model.editorOp(op)
    }

    var body: some View {
        section("Add a step", theme: theme) {
            Text(selectedNode.map { "After `\($0.name)`:" } ?? "Select a step; the new one goes after it.").font(theme.body()).foregroundStyle(theme.muted)
            ForEach(entries.filter { $0.available }, id: \.type) { en in
                Button("+ \(en.type)") { insert(en, extends: false) }
                    .disabled(selectedNode == nil).help(en.about).accessibilityIdentifier("palette:\(en.type)")
            }
            Button("+ copy of the selected (extends)") { if let en = entries.first(where: { $0.type == selectedNode?.kind }) { insert(en, extends: true) } }
                .disabled(selectedNode == nil).accessibilityIdentifier("palette:extends")
        }
    }
}

private struct Inspector: View {
    @Environment(\.theme) var theme
    @ObservedObject var model: MonitorModel
    let e: Mason_V1_EditorView
    @State private var newName = ""

    var node: Mason_V1_EditorNode? { e.nodes.first { $0.name == e.selected } }

    func target(_ kind: String) -> String { e.edges.first { $0.from == e.selected && $0.kind == kind }?.to ?? "" }

    func edgePicker(_ title: String, kind: String) -> some View {
        let binding = Binding<String>(get: { target(kind) }, set: { model.editorOp(["op": "set_edge", "from": e.selected, "edge": kind, "to": $0.isEmpty ? NSNull() : $0]) })
        return Picker(title, selection: binding) {
            Text("(nowhere)").tag("")
            ForEach(e.stepNames.filter { $0 != e.selected || kind == "on_red" }, id: \.self) { Text($0).tag($0) }
        }.accessibilityIdentifier("edge:\(kind)")
    }

    var body: some View {
        section("Step \(e.selected) (\(e.inspector.kind))", theme: theme) {
            HStack {
                TextField("Rename to", text: $newName).onSubmit { if !newName.isEmpty { model.editorOp(["op": "rename_step", "from": e.selected, "to": newName]); newName = "" } }
                Button("Rename") { if !newName.isEmpty { model.editorOp(["op": "rename_step", "from": e.selected, "to": newName]); newName = "" } }
            }
            edgePicker("On success", kind: "next")
            edgePicker("On red", kind: "on_red")
            HStack {
                Button("Start here") { model.editorOp(["op": "set_start", "name": e.selected]) }.disabled(node?.isStart ?? true)
                Button("Remove", role: .destructive) { model.editorOp(["op": "remove_step", "name": e.selected]); model.selectStep("") }
                    .accessibilityIdentifier("remove")
            }
            SchemaForm(model: model, step: e.selected, schema: e.inspector.schema, values: e.inspector.values)
        }
    }
}

/// A form built from a JSON Schema alone: a string (with its choices, if it has any), a
/// number, a switch, a list (of choices or of text), a table of whole numbers.
private struct SchemaForm: View {
    @Environment(\.theme) var theme
    @ObservedObject var model: MonitorModel
    let step: String
    let schema: String
    let values: String

    var props: [(String, [String: Any])] {
        guard let s = try? JSONSerialization.jsonObject(with: Data(schema.utf8)) as? [String: Any],
              let p = s["properties"] as? [String: [String: Any]] else { return [] }
        let order = s["x-order"] as? [String] ?? p.keys.sorted()
        return order.compactMap { k in p[k].map { (k, $0) } }
    }

    var vals: [String: Any] { (try? JSONSerialization.jsonObject(with: Data(values.utf8)) as? [String: Any]) ?? [:] }

    func set(_ key: String, _ v: Any?) {
        if let v { model.editorOp(["op": "set_field", "step": step, "key": key, "value": v]) }
        else { model.editorOp(["op": "unset_field", "step": step, "key": key]) }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: theme.spacing) {
            ForEach(props, id: \.0) { key, p in
                VStack(alignment: .leading, spacing: 2) {
                    Text((p["title"] as? String) ?? key).font(theme.body(.semibold))
                    field(key, p)
                    if let d = p["description"] as? String { Text(d).font(theme.small()).foregroundStyle(theme.muted) }
                }.accessibilityIdentifier("field:\(key)")
            }
        }
    }

    @ViewBuilder
    func field(_ key: String, _ p: [String: Any]) -> some View {
        let v = vals[key]
        let type = p["type"] as? String ?? "string"
        if type == "string", let choices = p["enum"] as? [String] {
            Picker("", selection: Binding(get: { (v as? String) ?? "" }, set: { set(key, $0.isEmpty ? nil : $0) })) {
                Text("(none)").tag("")
                ForEach(choices, id: \.self) { Text($0).tag($0) }
            }.labelsHidden()
        } else if type == "string" {
            FieldText(initial: (v as? String) ?? "") { t in set(key, t.isEmpty ? nil : t) }.id("\(step)\(key)\(v as? String ?? "")")
        } else if type == "integer" {
            FieldText(initial: (v as? Int).map(String.init) ?? "") { t in set(key, Int(t.trimmingCharacters(in: .whitespaces))) }.id("\(step)\(key)\(v as? Int ?? -1)")
        } else if type == "boolean" {
            Toggle("", isOn: Binding(get: { (v as? Bool) ?? false }, set: { set(key, $0) })).labelsHidden()
        } else if type == "array", let items = p["items"] as? [String: Any], let choices = items["enum"] as? [String] {
            let cur = (v as? [String]) ?? []
            HStack {
                ForEach(choices, id: \.self) { c in
                    Toggle(c, isOn: Binding(get: { cur.contains(c) }, set: { on in
                        let next = on ? cur + [c] : cur.filter { $0 != c }
                        set(key, next.isEmpty ? nil : next)
                    })).toggleStyle(.button)
                }
            }
        } else if type == "array" {
            FieldText(initial: ((v as? [String]) ?? []).joined(separator: ", ")) { t in
                let l = t.split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
                set(key, l.isEmpty ? nil : l)
            }.id("\(step)\(key)\(((v as? [String]) ?? []).joined(separator: ","))")
        } else if type == "object" {
            let cur = (v as? [String: Int]) ?? [:]
            FieldText(initial: cur.sorted { $0.key < $1.key }.map { "\($0.key)=\($0.value)" }.joined(separator: ", ")) { t in
                var d: [String: Int] = [:]
                for part in t.split(separator: ",") {
                    let kv = part.split(separator: "=").map { $0.trimmingCharacters(in: .whitespaces) }
                    if kv.count == 2, let n = Int(kv[1]) { d[kv[0]] = n }
                }
                set(key, d.isEmpty ? nil : d)
            }.id("\(step)\(key)\(cur.count)")
        }
    }
}

private struct FieldText: View {
    @State var text: String
    let commit: (String) -> Void
    init(initial: String, commit: @escaping (String) -> Void) { _text = State(initialValue: initial); self.commit = commit }
    var body: some View { TextField("", text: $text).onSubmit { commit(text) } }
}

// MARK: the test grid

private struct TestsTab: View {
    @Environment(\.theme) var theme
    @ObservedObject var model: MonitorModel
    let e: Mason_V1_EditorView

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: theme.spacing) {
                Text("Where each suite's level runs. A tick binds the level, so every suite of that level runs there.").font(theme.body()).foregroundStyle(theme.muted)
                if e.grid.rows.isEmpty { Text("This pipeline has no suites.").font(theme.body()).foregroundStyle(theme.muted) }
                Grid(alignment: .leading, horizontalSpacing: theme.spacingLarge, verticalSpacing: theme.spacing) {
                    GridRow {
                        Text("Suite").font(theme.body(.semibold))
                        Text("Level").font(theme.body(.semibold))
                        ForEach(e.grid.columns, id: \.self) { Text($0.replacingOccurrences(of: "_", with: " ")).font(theme.body(.semibold)) }
                    }
                    ForEach(e.grid.rows, id: \.suite) { r in
                        GridRow {
                            Text(r.suite).font(theme.body())
                            Picker("", selection: Binding(get: { r.level }, set: { model.editorOp(["op": "set_suite_level", "suite": r.suite, "level": $0]) })) {
                                ForEach(Array(Set(e.grid.levels + [r.level])).sorted(), id: \.self) { Text($0).tag($0) }
                            }.labelsHidden().frame(width: 130)
                            ForEach(Array(e.grid.columns.enumerated()), id: \.offset) { i, c in
                                Toggle("", isOn: Binding(get: { i < r.cells.count && r.cells[i] }, set: { on in
                                    model.editorOp(["op": "set_binding", "level": r.level, "when": c, "on": on])
                                })).labelsHidden().accessibilityIdentifier("cell:\(r.suite):\(c)")
                            }
                        }.accessibilityIdentifier("grid:\(r.suite)")
                    }
                }
            }
        }
    }
}

// MARK: the landing editor

private struct LandingTab: View {
    @Environment(\.theme) var theme
    @ObservedObject var model: MonitorModel
    let e: Mason_V1_EditorView
    @State private var newTier = ""

    var form: [String: Any] { (try? JSONSerialization.jsonObject(with: Data(e.landing.utf8)) as? [String: Any]) ?? [:] }

    func set(_ key: String, _ v: Any) { model.editorOp(["op": "landing_set", "key": key, "value": v]) }

    var body: some View {
        let f = form
        let modes = f["modes"] as? [String] ?? []
        let tiers = f["tiers"] as? [[String: Any]] ?? []
        let ladder = f["ladder"] as? [String] ?? []
        let rungs = f["rungs"] as? [String] ?? []
        ScrollView {
            VStack(alignment: .leading, spacing: theme.spacingLarge) {
                section("How this project lands", theme: theme) {
                    Picker("Mode", selection: Binding(get: { f["mode"] as? String ?? "direct" }, set: { set("mode", $0) })) {
                        ForEach(modes, id: \.self) { Text($0).tag($0) }
                    }.frame(maxWidth: 360).accessibilityIdentifier("landing:mode")
                    row("Window (collect from the oldest ready task)", "\(f["window_s"] as? Int ?? 0)s") { set("window", $0) }
                    row("Largest batch (0: no cap)", "\(f["max_size"] as? Int ?? 0)") { if let n = Int($0) { set("max_size", n) } }
                    row("Batches at once", "\(f["parallel"] as? Int ?? 1)") { if let n = Int($0) { set("parallel", n) } }
                    row("Times the base may move before giving up", "\(f["max_fold"] as? Int ?? 3)") { if let n = Int($0) { set("max_fold", n) } }
                    row("Conflict resolver command", f["resolver"] as? String ?? "") { model.editorOp($0.isEmpty ? ["op": "landing_unset", "key": "resolver"] : ["op": "landing_set", "key": "resolver", "value": $0]) }
                }
                section("Tiers", theme: theme) {
                    ForEach(Array(tiers.enumerated()), id: \.offset) { _, t in
                        let name = t["name"] as? String ?? ""
                        let have = t["suites"] as? [String] ?? []
                        VStack(alignment: .leading, spacing: theme.spacingSmall) {
                            HStack {
                                Text(name).font(theme.body(.semibold))
                                Spacer()
                                Button("Remove", role: .destructive) { model.editorOp(["op": "landing_remove_tier", "name": name]) }
                            }
                            HStack {
                                ForEach(e.suites, id: \.self) { s in
                                    Toggle(s, isOn: Binding(get: { have.contains(s) }, set: { on in
                                        model.editorOp(["op": "landing_set_tier", "name": name, "key": "suites", "value": on ? have + [s] : have.filter { $0 != s }])
                                    })).toggleStyle(.button)
                                }
                            }
                            row("On red (freeze)", t["on_red"] as? String ?? "") { model.editorOp(["op": "landing_set_tier", "name": name, "key": "on_red", "value": $0.isEmpty ? NSNull() : $0]) }
                            row("CI", t["ci"] as? String ?? "") { model.editorOp(["op": "landing_set_tier", "name": name, "key": "ci", "value": $0.isEmpty ? NSNull() : $0]) }
                        }.accessibilityIdentifier("tier:\(name)")
                    }
                    HStack {
                        TextField("New tier (batch, post-land, ...)", text: $newTier).frame(maxWidth: 260)
                        Button("Add tier") { if !newTier.isEmpty { model.editorOp(["op": "landing_add_tier", "name": newTier, "suites": []]); newTier = "" } }
                    }
                }
                section("When a batch is red, in this order", theme: theme) {
                    let all = ladder + rungs.filter { !ladder.contains($0) }
                    ForEach(Array(all.enumerated()), id: \.element) { i, r in
                        HStack {
                            Toggle(r, isOn: Binding(get: { ladder.contains(r) }, set: { on in
                                model.editorOp(["op": "landing_set_ladder", "rungs": on ? ladder + [r] : ladder.filter { $0 != r }])
                            }))
                            Button("↑") { move(ladder, i, -1) }.disabled(!ladder.contains(r) || i == 0)
                            Button("↓") { move(ladder, i, 1) }.disabled(!ladder.contains(r) || i >= ladder.count - 1)
                        }
                    }
                }
                ForEach(f["problems"] as? [String] ?? [], id: \.self) { Text($0).font(theme.body()).foregroundStyle(theme.attention) }
            }
        }
    }

    func move(_ ladder: [String], _ i: Int, _ d: Int) {
        var l = ladder
        guard i + d >= 0, i + d < l.count else { return }
        l.swapAt(i, i + d)
        model.editorOp(["op": "landing_set_ladder", "rungs": l])
    }

    func row(_ title: String, _ initial: String, _ commit: @escaping (String) -> Void) -> some View {
        HStack {
            Text(title).font(theme.body()).frame(width: 340, alignment: .leading)
            FieldText(initial: initial, commit: commit).id("\(title)\(initial)").frame(maxWidth: 220)
        }
    }
}

// MARK: preview and simulate

private struct PreviewTab: View {
    @Environment(\.theme) var theme
    @ObservedObject var model: MonitorModel
    let e: Mason_V1_EditorView
    @State private var scenario = ""

    var scenarios: [[String: Any]] {
        guard let d = try? JSONSerialization.jsonObject(with: Data(e.simulation.utf8)) as? [String: Any] else { return [] }
        return d["scenarios"] as? [[String: Any]] ?? []
    }

    /// `test: red, red` becomes a scripted task whose `test` step is red twice.
    func custom() {
        var results: [String: [String]] = [:]
        for part in scenario.split(separator: ";") {
            let kv = part.split(separator: ":", maxSplits: 1).map { $0.trimmingCharacters(in: .whitespaces) }
            if kv.count == 2 { results[kv[0]] = kv[1].split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) } }
        }
        model.editorCall("simulate", ["name": "your scenario", "results": results])
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: theme.spacingLarge) {
                section("What this edit changes", theme: theme) {
                    if e.changes.isEmpty && e.affected.isEmpty { Text("Nothing yet.").font(theme.body()).foregroundStyle(theme.muted) }
                    ForEach(Array(e.changes.enumerated()), id: \.offset) { i, c in
                        Text("This \(c)").font(theme.body()).accessibilityIdentifier("change:\(i)")
                    }
                    ForEach(e.affected, id: \.self) { Text($0).font(theme.body()).foregroundStyle(theme.attention) }
                }
                section("Simulate a scripted task through the draft", theme: theme) {
                    HStack {
                        Button("Run the standard scenarios") { model.editorCall("simulate") }
                        TextField("test: red, red; develop: green", text: $scenario).frame(maxWidth: 320).onSubmit { custom() }
                        Button("Run this") { custom() }
                    }
                    ForEach(Array(scenarios.enumerated()), id: \.offset) { _, s in
                        let steps = (s["steps"] as? [[String: Any]] ?? []).map { st -> String in
                            let o = st["outcome"] as? String ?? ""
                            return (st["step"] as? String ?? "") + (o == "green" ? "" : " (\(o))")
                        }.joined(separator: " > ")
                        VStack(alignment: .leading, spacing: 2) {
                            Text(s["name"] as? String ?? "").font(theme.body(.semibold))
                            Text("\(steps)  =>  \(s["end"] as? String ?? (s["error"] as? String ?? ""))").font(theme.mono())
                        }
                    }
                }
                section("The change to the files", theme: theme) {
                    Text(e.diff.isEmpty ? "No change." : e.diff).font(theme.mono()).textSelection(.enabled)
                }
            }
        }
    }
}
