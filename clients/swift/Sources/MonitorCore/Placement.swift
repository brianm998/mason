// SPDX-License-Identifier: GPL-3.0-or-later

import Foundation
import MasonProtocol

/// Where runs go can be set at three levels (#834): all of MASON, one project, one task.
public enum PlacementScope: Equatable, Sendable {
    case global
    case project(String)
    case task(UInt64)

    /// "global", "project" or "task": the `SetPlacement.scope` the daemon reads.
    public var wire: String {
        switch self {
        case .global: return "global"
        case .project: return "project"
        case .task: return "task"
        }
    }

    /// What clearing the choice is called, nil where there is nothing above to follow.
    public var followLabel: String? {
        switch self {
        case .global: return nil
        case .project: return "Follow MASON"
        case .task: return "Follow project"
        }
    }

    /// The word for the level in the confirm.
    var noun: String {
        switch self {
        case .global: return "all of MASON"
        case .project(let p): return "project \(p)"
        case .task(let n): return "#\(n)"
        }
    }
}

/// The four modes, in the order a control lists them.
public let placementModes = ["local", "remote", "favor-local", "favor-remote"]

/// The mode the owner picked, or "follow the level above" (the wire's `default`).
public struct PlacementAsk: Equatable, Sendable {
    public let scope: PlacementScope
    /// A mode name, or "default".
    public let mode: String

    public init(scope: PlacementScope, mode: String) {
        self.scope = scope
        self.mode = mode
    }

    /// The one-line confirm shown before anything is sent.
    public var confirmText: String {
        if mode == "default" {
            return "Make \(scope.noun) \(scope.followLabel.map { $0.lowercased() } ?? "follow its default")? Runs not yet started move; a run in progress is not moved."
        }
        return "Run \(scope.noun) on \(mode)? Runs not yet started move; a run in progress is not moved."
    }

    /// The command the confirm sends.
    public var command: Mason_V1_Command {
        var c = Mason_V1_Command()
        c.setPlacement = .with {
            $0.scope = scope.wire
            $0.mode = mode
            switch scope {
            case .global: break
            case .project(let p): $0.project = p
            case .task(let n): $0.task = n
            }
        }
        return c
    }

    /// Where the pending command is kept (see `PendingCommand.placementKey`).
    public var pendingKey: (task: UInt64, action: String) {
        (PendingCommand.placementKey(scope), "set_placement")
    }
}

extension PendingCommand {
    /// A project's or MASON's place among the task ids a pending command is kept under: a task id
    /// stays itself; the others are lifted clear of every task's (and of a batch's).
    public static func placementKey(_ s: PlacementScope) -> UInt64 {
        switch s {
        case .task(let n): return n
        case .global: return 1 << 61
        case .project(let p):
            var h: UInt64 = 1469598103934665603
            for b in p.utf8 { h = (h ^ UInt64(b)) &* 1099511628211 }
            return (1 << 61) | (h & 0xFFFF_FFFF) | (1 << 32)
        }
    }
}

extension MonitorModel {
    /// The placement the daemon last sent for this level; nil before a view has it.
    public func placement(_ s: PlacementScope) -> Mason_V1_Placement? {
        switch s {
        case .global:
            guard let h = home, h.hasPlacement else { return nil }
            return h.placement
        case .project(let p):
            return header?.projectPlacements.first { $0.project == p && $0.hasPlacement }?.placement
        case .task(let n):
            guard let d = taskDetail, d.hasRow, d.row.id == n, d.hasPlacement else { return nil }
            return d.placement
        }
    }

    /// "favor-remote, from the project": the mode in force and the level that set it.
    public func placementText(_ s: PlacementScope) -> String? {
        guard let p = placement(s) else { return nil }
        return Self.placementText(p)
    }

    public static func placementText(_ p: Mason_V1_Placement) -> String {
        "\(p.mode), from the \(p.source == "global" ? "global switch" : p.source)"
    }

    /// The control is the owner's, on a device that can act, where the daemon sent a placement
    /// (an older daemon sends none) and the level can be set.
    public func canSetPlacement(_ s: PlacementScope) -> Bool {
        guard canAct, let p = placement(s) else { return false }
        return p.settable
    }

    /// "Follow MASON" / "Follow project" is offered only while the level has a choice of its own.
    public func canFollow(_ s: PlacementScope) -> Bool {
        canSetPlacement(s) && s.followLabel != nil && placement(s)?.own == true
    }

    /// What the control says while its command is pending; nil when none is.
    public func placementPendingLabel(_ s: PlacementScope) -> String? {
        pendingLabel(task: PendingCommand.placementKey(s), action: "set_placement")
    }

    /// A mode (or "default") was chosen: ask first.
    public func askPlacement(_ s: PlacementScope, mode: String) {
        placementAsk = PlacementAsk(scope: s, mode: mode)
    }

    public func cancelPlacement() { placementAsk = nil }

    /// "Change" in the confirm: send the command for what was asked.
    public func confirmPlacement() {
        guard let a = placementAsk else { return }
        placementAsk = nil
        send(a.command)
    }

    /// The task row's small marker: the task overrides its project's mode.
    public static func runOnMarker(_ r: TaskRow) -> String? {
        r.runOnOverride.isEmpty ? nil : "runs on \(r.runOnOverride)"
    }
}
