// SPDX-License-Identifier: GPL-3.0-or-later

import AppKit
import Foundation
import MasonProtocol
import MonitorCore
import MonitorUI
import SwiftUI

/// `monitor-tool`: what the exit test and the owner checkpoint use.
///
///     monitor-tool conformance --port P --out DIR   dump what the client shows at every demo checkpoint
///     monitor-tool snapshot --port P --out DIR      render screenshots from `masond --demo-manual`
///     monitor-tool editor --port P --project NAME [--step S] --out DIR   dump what the pipeline editor shows
///     monitor-tool answer --port P --task N [--option 1]   answer a task's open question as the monitor does
///     monitor-tool cancel --port P --task N         cancel a task as the Cancel button does
///     monitor-tool skip --port P --task N [--step S]  skip a test step as the Skip button does
@main
struct Tool {
    @MainActor
    static func main() async {
        _ = NSApplication.shared
        var args = Array(CommandLine.arguments.dropFirst())
        guard !args.isEmpty else { fail("usage: monitor-tool conformance|snapshot --port P --out DIR") }
        let cmd = args.removeFirst()
        guard let port = Discovery.port(arguments: args) else { fail("--port P (or --data-dir D)") }
        var out = URL(fileURLWithPath: "/tmp")
        if !["answer", "cancel", "skip"].contains(cmd) {
            guard let o = args.firstIndex(of: "--out"), o + 1 < args.count else { fail("--out DIR") }
            out = URL(fileURLWithPath: args[o + 1])
            try? FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)
        }

        let model = MonitorModel()
        let connection = MonitorConnection(model: model, port: port)
        connection.start()
        await until("a connection") { model.connected && model.header != nil && model.sidebar != nil && model.home != nil }

        switch cmd {
        case "conformance": await conformance(model, out)
        case "editor": await editor(model, args, out)
        case "snapshot": await snapshots(model, out)
        case "answer": await answer(model, args)
        case "cancel", "skip": await act(model, cmd, args)
        default: fail("unknown command \(cmd)")
        }
        exit(0)
    }

    static func fail(_ m: String) -> Never {
        FileHandle.standardError.write(Data("monitor-tool: \(m)\n".utf8))
        exit(2)
    }

    /// Wait for a condition on the main actor.
    @MainActor
    static func until(_ what: String, seconds: Double = 15, _ cond: @MainActor () -> Bool) async {
        let end = Date().addingTimeInterval(seconds)
        while !cond() {
            if Date() > end { fail("timed out waiting for \(what)") }
            try? await Task.sleep(nanoseconds: 50_000_000)
        }
    }

    /// Let the views settle: no new sequence number for a moment.
    @MainActor
    static func settle(_ model: MonitorModel) async {
        var last = model.lastSeq
        var quiet = 0
        while quiet < 6 {
            try? await Task.sleep(nanoseconds: 50_000_000)
            if model.lastSeq == last { quiet += 1 } else { quiet = 0; last = model.lastSeq }
        }
    }

    /// Advance the demo one step. Returns false when the script is over.
    @MainActor
    static func step(_ model: MonitorModel) async -> Bool {
        var advanced: Bool?
        model.onResult = { r in
            if r.id == "step" {
                let v = (try? JSONSerialization.jsonObject(with: Data(r.json.utf8))) as? [String: Any]
                advanced = v?["advanced"] as? Bool ?? false
            }
        }
        var c = Mason_V1_Command()
        c.id = "step"
        c.raw = .with { $0.method = "POST"; $0.path = "/demo/next" }
        model.send(c)
        await until("the demo to advance") { advanced != nil }
        await settle(model)
        return advanced ?? false
    }

    // MARK: cancel and skip

    /// What the Cancel and Skip buttons send: the same commands, with the owner's identity.
    @MainActor
    static func act(_ model: MonitorModel, _ cmd: String, _ args: [String]) async {
        func opt(_ n: String) -> String? {
            args.firstIndex(of: n).flatMap { $0 + 1 < args.count ? args[$0 + 1] : nil }
        }
        guard let t = opt("--task").flatMap(UInt64.init) else { fail("--task N") }
        var result: Mason_V1_CommandResult?
        model.onResult = { r in if r.id == cmd { result = r } }
        var c = Mason_V1_Command()
        c.id = cmd
        if cmd == "cancel" {
            c.cancel = .with { $0.task = t }
        } else {
            let step = opt("--step") ?? "test"
            c.skip = .with { $0.task = t; $0.step = step }
        }
        model.send(c)
        await until("the result") { result != nil }
        guard result?.ok == true else { fail("the daemon refused: \(result?.error ?? "?")") }
        print(cmd == "cancel" ? "cancelled task #\(t)" : "skipped step \(opt("--step") ?? "test") of task #\(t)")
    }

    // MARK: answering

    /// What clicking an option on the decision card does: find the task's card
    /// in the Home view, send its answer with the card's hash, and wait until
    /// the task is no longer waiting for the owner.
    @MainActor
    static func answer(_ model: MonitorModel, _ args: [String]) async {
        func opt(_ n: String) -> String? {
            args.firstIndex(of: n).flatMap { $0 + 1 < args.count ? args[$0 + 1] : nil }
        }
        guard let t = opt("--task").flatMap(UInt64.init) else { fail("--task N") }
        let option = opt("--option") ?? "1"
        await until("task #\(t)'s decision card") {
            model.home?.attention.contains { $0.task.id == t && $0.hasCard } ?? false
        }
        let item = model.home!.attention.first { $0.task.id == t }!
        print("card: \(item.card.ask)")
        print("options: " + item.card.options.map { "\($0.label)\($0.recommended ? " (recommended)" : "")" }.joined(separator: " | "))
        var result: Mason_V1_CommandResult?
        model.onResult = { r in if r.id == "answer" { result = r } }
        var c = Mason_V1_Command()
        c.id = "answer"
        c.answer = .with { $0.task = t; $0.answer = option; $0.cardHash = item.card.hash }
        model.send(c)
        await until("the answer's result") { result != nil }
        guard result?.ok == true else { fail("the daemon refused the answer: \(result?.error ?? "?")") }
        await until("the task to leave needs-you") {
            !(model.home?.attention.contains { $0.task.id == t } ?? true)
        }
        print("answered task #\(t) with option \(option); it is no longer waiting for the owner")
    }

    // MARK: the pipeline editor

    /// What the editor shows for a project's draft, as items: the pipeline page, opened
    /// on a project with a step selected, and its `visibleItems()` written out. (Needs a
    /// real daemon: the demo has no repositories.)
    @MainActor
    static func editor(_ model: MonitorModel, _ args: [String], _ out: URL) async {
        func opt(_ n: String) -> String? {
            args.firstIndex(of: n).flatMap { $0 + 1 < args.count ? args[$0 + 1] : nil }
        }
        guard let project = opt("--project") else { fail("--project NAME") }
        let step = opt("--step") ?? ""
        model.project = project
        model.selectStep(step)
        model.go(.pipeline)
        await until("the editor for \(project)") { model.editor?.project == project && model.editor?.selected == step }
        await settle(model)
        let items = model.visibleItems().map(\.json)
        let data = try! JSONSerialization.data(withJSONObject: items, options: [.prettyPrinted, .sortedKeys])
        try! data.write(to: out.appendingPathComponent("checkpoint-0.json"))
        print("the editor shows \(items.count) items")
    }

    // MARK: conformance

    @MainActor
    static func conformance(_ model: MonitorModel, _ out: URL) async {
        var k = 0
        while true {
            await settle(model)
            let items = model.visibleItems().map(\.json)
            let data = try! JSONSerialization.data(withJSONObject: items, options: [.prettyPrinted, .sortedKeys])
            try! data.write(to: out.appendingPathComponent("checkpoint-\(k).json"))
            print("checkpoint \(k): \(items.count) visible items")
            guard await step(model) else { break }
            k += 1
        }
    }

    // MARK: screenshots

    @MainActor
    static func snapshots(_ model: MonitorModel, _ out: URL) async {
        // Checkpoint 4: a question is open, tasks are in several stages.
        for _ in 0..<4 { _ = await step(model) }
        model.setExpanded("needs_attention", true)
        shot(model, out, "home", size: CGSize(width: 1440, height: 900), dark: false)
        shot(model, out, "home-dark", size: CGSize(width: 1440, height: 900), dark: true)

        // The rest of the script: failed and landed tasks exist; open every section.
        for _ in 0..<3 { _ = await step(model) }
        for s in model.sidebar?.sections ?? [] { model.setExpanded(s.state, true) }
        shot(model, out, "sidebar-expanded", size: CGSize(width: 1440, height: 1100), dark: false)

        model.go(.task(1))
        await until("task #1") { model.taskDetail?.row.id == 1 }
        await settle(model)
        shot(model, out, "task-detail", size: CGSize(width: 1440, height: 900), dark: false)

        model.go(.task(2))
        await until("task #2") { model.taskDetail?.row.id == 2 }
        await settle(model)
        shot(model, out, "decision-card", size: CGSize(width: 1440, height: 900), dark: false)

        model.zoomIn(); model.zoomIn(); model.zoomIn()
        model.goHome()
        await settle(model)
        shot(model, out, "home-larger-text", size: CGSize(width: 1440, height: 900), dark: false)
    }

    /// Render the real window content (AppKit-backed controls included) to a PNG.
    @MainActor
    static func shot(_ model: MonitorModel, _ out: URL, _ name: String, size: CGSize, dark: Bool) {
        let host = NSHostingView(rootView: RootView(model: model))
        host.frame = CGRect(origin: .zero, size: size)
        let window = NSWindow(contentRect: host.frame, styleMask: [.titled], backing: .buffered, defer: false)
        window.appearance = NSAppearance(named: dark ? .darkAqua : .aqua)
        window.contentView = host
        host.layoutSubtreeIfNeeded()
        RunLoop.main.run(until: Date().addingTimeInterval(0.6))
        host.layoutSubtreeIfNeeded()
        guard let rep = host.bitmapImageRepForCachingDisplay(in: host.bounds) else { fail("cannot render \(name)") }
        host.cacheDisplay(in: host.bounds, to: rep)
        guard let png = rep.representation(using: .png, properties: [:]) else { fail("cannot encode \(name)") }
        try! png.write(to: out.appendingPathComponent("\(name).png"))
        print("wrote \(name).png")
    }
}
