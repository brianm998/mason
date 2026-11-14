// SPDX-License-Identifier: GPL-3.0-or-later

import AppKit
import MasonProtocol
import MonitorCore
import SwiftUI
import XCTest

@testable import MonitorUI

/// s299: the pipeline editor driven by its controls' actions (add a step, connect it, edit a form
/// field, undo), with the operations they send checked against a small stand-in for the daemon's
/// draft, and the drawing checked as a bitmap. Rendered offscreen: a hosting view that is never
/// put in a window, so nothing appears on screen. No golden files; the bitmaps are compared with
/// each other (changed after an edit, the same again after undo).
@MainActor
final class EditorSnapshotTests: XCTestCase {
    /// The daemon's draft, as far as these operations go; every command is answered with a new view.
    struct Draft: Equatable {
        var steps: [(name: String, kind: String, category: String)] = [("develop", "agent", "developing")]
        var edges: [(from: String, kind: String, to: String)] = []
        var fields: [String: String] = [:]
        static func == (a: Draft, b: Draft) -> Bool {
            a.steps.map(\.name) == b.steps.map(\.name) && a.edges.map { "\($0.from)\($0.kind)\($0.to)" } == b.edges.map { "\($0.from)\($0.kind)\($0.to)" } && a.fields == b.fields
        }
    }

    final class FakeDaemon {
        var draft = Draft()
        var history: [Draft] = []
        var ops: [[String: Any]] = []
        let model: MonitorModel

        init(_ model: MonitorModel) {
            self.model = model
            model.project = "web"
            model.sender = { [unowned self] c in self.receive(c) }
            publish()
        }

        func receive(_ c: Mason_V1_Command) {
            if c.raw.path.hasSuffix("/editor/undo") {
                if let last = history.popLast() { draft = last }
            } else if c.raw.path.hasSuffix("/editor/op"),
                      let op = try? JSONSerialization.jsonObject(with: Data(c.raw.json.utf8)) as? [String: Any] {
                ops.append(op)
                history.append(draft)
                switch op["op"] as? String {
                case "insert_step": draft.steps.append((op["name"] as! String, op["type"] as! String, op["category"] as! String))
                case "set_edge":
                    draft.edges.removeAll { $0.from == op["from"] as? String && $0.kind == op["edge"] as? String }
                    if let to = op["to"] as? String { draft.edges.append((op["from"] as! String, op["edge"] as! String, to)) }
                case "set_field": draft.fields[op["key"] as! String] = op["value"] as? String
                default: break
                }
            }
            publish()
        }

        func publish() {
            var e = Mason_V1_EditorView()
            e.project = "web"
            e.lanes = [("developing", "Developing"), ("testing", "Testing")].map { c, t in .with { $0.category = c; $0.title = t } }
            e.nodes = draft.steps.enumerated().map { i, s in
                .with { $0.name = s.name; $0.kind = s.kind; $0.category = s.category; $0.isStart = i == 0; $0.x = Int64(i * 190); $0.y = s.category == "testing" ? 140 : 0; $0.summary = draft.fields["model"].flatMap { s.name == "develop" ? $0 : nil } ?? "" }
            }
            e.edges = draft.edges.map { ed in .with { $0.from = ed.from; $0.to = ed.to; $0.kind = ed.kind } }
            e.stepNames = draft.steps.map(\.name)
            e.selected = "develop"
            e.dirty = !history.isEmpty
            e.canUndo = !history.isEmpty
            e.palette = [#"{"type":"test","categories":["testing"],"available":true,"about":"run a suite","fields":{"suite":"unit"}}"#]
            e.inspector = .with {
                $0.step = "develop"
                $0.kind = "agent"
                $0.schema = #"{"properties":{"model":{"type":"string","title":"Model"}},"x-order":["model"]}"#
                $0.values = draft.fields["model"].map { #"{"model":"\#($0)"}"# } ?? "{}"
            }
            model.editor = e
        }
    }

    /// The editor page drawn into a bitmap, without a window.
    func render(_ model: MonitorModel) -> Data {
        let host = NSHostingView(rootView: EditorPage(model: model).environment(\.colorScheme, .light))
        host.frame = NSRect(x: 0, y: 0, width: 1100, height: 700)
        host.layoutSubtreeIfNeeded()
        let rep = host.bitmapImageRepForCachingDisplay(in: host.bounds)!
        host.cacheDisplay(in: host.bounds, to: rep)
        return rep.representation(using: .png, properties: [:]) ?? Data()
    }

    func testAddConnectEditAndUndoThroughTheControlsActions() throws {
        let model = MonitorModel()
        let daemon = FakeDaemon(model)
        let blank = render(model)
        XCTAssertFalse(blank.isEmpty)
        let e0 = try XCTUnwrap(model.editor)

        // Add a step from the palette.
        EditorActions.insertStep(model, e0, type: "test", categories: ["testing"], fields: ["suite": "unit"], extends: false)
        XCTAssertEqual(daemon.ops.last?["op"] as? String, "insert_step")
        XCTAssertEqual(daemon.ops.last?["name"] as? String, "test-1")
        XCTAssertEqual(daemon.ops.last?["after"] as? String, "develop")
        XCTAssertEqual(daemon.ops.last?["category"] as? String, "testing")
        XCTAssertEqual(daemon.draft.steps.map(\.name), ["develop", "test-1"])
        let added = render(model)
        XCTAssertNotEqual(added, blank, "the new step is drawn")

        // Connect it.
        EditorActions.connect(model, from: "develop", kind: "next", to: "test-1")
        XCTAssertEqual(daemon.ops.last?["op"] as? String, "set_edge")
        XCTAssertEqual(daemon.draft.edges.map { "\($0.from)>\($0.to)" }, ["develop>test-1"])
        XCTAssertEqual(model.editor?.edges.map(\.to), ["test-1"])
        let connected = render(model)
        XCTAssertNotEqual(connected, added, "the edge is drawn")

        // Edit a form field.
        EditorActions.setField(model, step: "develop", key: "model", value: "deep")
        XCTAssertEqual(daemon.ops.last?["op"] as? String, "set_field")
        XCTAssertEqual(daemon.ops.last?["key"] as? String, "model")
        XCTAssertEqual(daemon.draft.fields["model"], "deep")
        XCTAssertEqual(model.editor?.nodes.first?.summary, "deep")
        let edited = render(model)
        XCTAssertNotEqual(edited, connected, "the field's value is drawn")

        // Undo the three edits, one at a time; each drawing is the one before.
        EditorActions.undo(model)
        XCTAssertNil(daemon.draft.fields["model"])
        XCTAssertEqual(render(model), connected)
        EditorActions.undo(model)
        XCTAssertTrue(daemon.draft.edges.isEmpty)
        XCTAssertEqual(render(model), added)
        EditorActions.undo(model)
        XCTAssertEqual(daemon.draft.steps.map(\.name), ["develop"])
        XCTAssertEqual(render(model), blank, "back to the first drawing")
        XCTAssertEqual(model.editor?.canUndo, false)
        XCTAssertEqual(daemon.ops.count, 3, "undo is a call, not an operation")
    }
}
