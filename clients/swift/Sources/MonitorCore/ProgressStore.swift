// SPDX-License-Identifier: GPL-3.0-or-later

import Foundation
import MasonProtocol

/// One task's live progress cells, observed apart from the model: a progress message for the task
/// redraws the views that watch this object and nothing else. `nil` means "no live update since
/// the last view": the view's own cells are the truth.
@MainActor
public final class TaskProgress: ObservableObject {
    @Published public var cells: [ProgressCell]?
    public init() {}
}

/// Live progress, keyed by task id and kept out of `sidebar` and `taskDetail`, so a StepProgress
/// message never republishes the whole sidebar. A view from the daemon replaces (clears) the
/// entries of the tasks it carries.
@MainActor
public final class ProgressStore {
    private var perTask: [UInt64: TaskProgress] = [:]
    /// The open task page's cells, apart from the sidebar's entry for the same task: each view
    /// clears only its own.
    public let detail = TaskProgress()

    public init() {}

    /// The observable entry for a sidebar row (made on first ask).
    public func entry(_ id: UInt64) -> TaskProgress {
        if let e = perTask[id] { return e }
        let e = TaskProgress()
        perTask[id] = e
        return e
    }

    /// The live cells for a task, if a progress message set them since its last view.
    public func cells(_ id: UInt64) -> [ProgressCell]? { perTask[id]?.cells }

    /// A new sidebar arrived carrying these tasks: its cells are the truth, so drop live ones.
    func reset(tasks ids: [UInt64]) {
        for id in ids where perTask[id]?.cells != nil { perTask[id]?.cells = nil }
    }

    func resetAll() {
        for e in perTask.values where e.cells != nil { e.cells = nil }
    }

    func resetDetail() {
        if detail.cells != nil { detail.cells = nil }
    }

    /// Merge one step's cell into a task's cells. A step the task does not show is ignored; a
    /// finished or stopped run says what it ended as on the next view, so until then no label.
    static func merged(_ old: [ProgressCell], _ cell: ProgressCell) -> [ProgressCell]? {
        guard let i = old.firstIndex(where: { $0.step == cell.step }) else { return nil }
        var c = cell
        if c.state != .part { c.label = "" }
        var out = old
        out[i] = c
        return out
    }
}
