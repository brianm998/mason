// SPDX-License-Identifier: GPL-3.0-or-later

import Foundation
import MasonProtocol

/// A command the owner sent that masond has not finished with (task #522). The model records it
/// before the command is written, so the control that was pressed redraws on the same frame.
public struct PendingCommand: Equatable, Sendable {
    public enum Stage: Equatable, Sendable {
        /// Written (or about to be); no answer yet.
        case sending
        /// masond accepted it; waiting for a view in which the task has changed.
        case accepted
        /// The connection went away before an answer.
        case notSent
    }

    /// A batch's place among the task ids a pending command is kept under: batch and task ids
    /// are both small numbers, so a batch's is lifted clear of every task's.
    public static func batchKey(_ batch: UInt64) -> UInt64 { (1 << 62) | batch }

    /// How many later sidebar or task views are waited for before the action is offered again.
    public static let viewsToWait = 3

    public let id: String
    /// The row's action name: "approve", "retry", "cancel", "land_now", ...
    public let action: String
    /// The task it is about.
    public let task: UInt64
    public let sent: Date
    public var stage: Stage = .sending
    /// The task's state and allowed actions when the command was sent.
    let baseline: String
    var viewsSeen = 0

    /// What the control says while this is pending.
    public var label: String {
        switch stage {
        case .sending: return Self.verbs(action).now
        case .accepted: return Self.verbs(action).done + ", waiting for the pipeline…"
        case .notSent: return "Not sent: reconnecting"
        }
    }

    static func verbs(_ action: String) -> (now: String, done: String) {
        switch action {
        case "approve": return ("Approving…", "Approved")
        case "reject": return ("Rejecting…", "Rejected")
        case "retry": return ("Retrying…", "Retried")
        case "cancel": return ("Cancelling…", "Cancelled")
        case "defer": return ("Deferring…", "Deferred")
        case "undefer": return ("Restoring…", "Restored")
        case "revive": return ("Reviving…", "Revived")
        case "explore_anyway": return ("Exploring…", "Asked to explore")
        case "land_now", "land_now_batch": return ("Landing…", "Landing requested")
        case "force": return ("Forcing…", "Forced")
        case "skip_test": return ("Skipping…", "Skipped")
        case "skip_all_tests": return ("Skipping all testing…", "Immune to testing")
        case "answer": return ("Sending answer…", "Answer sent")
        case "block": return ("Adding blocker…", "Blocker added")
        case "unblock": return ("Releasing…", "Released")
        case "pause": return ("Pausing…", "Paused")
        case "resume": return ("Resuming…", "Resumed")
        case "set_placement": return ("Changing where runs go…", "Changed")
        case "set_limit": return ("Saving…", "Saved")
        case "add_task": return ("Filing…", "Filed")
        default:
            let name = action.replacingOccurrences(of: "_", with: " ")
            return (name + "…", name)
        }
    }

    /// The action and task a command is about; nil for one that is not about one task (pause, limits, add task, raw), which is not tracked.
    static func describe(_ c: Mason_V1_Command) -> (action: String, task: UInt64)? {
        switch c.body {
        case .cancel(let x)?: return ("cancel", x.task)
        case .retry(let x)?: return ("retry", x.task)
        case .approve(let x)?: return ("approve", x.task)
        case .approveHold(let x)?: return ("approve", x.task)
        case .reject(let x)?: return ("reject", x.task)
        case .exploreAnyway(let x)?: return ("explore_anyway", x.task)
        case .answer(let x)?: return ("answer", x.task)
        case .skip(let x)?: return (x.allTests ? "skip_all_tests" : "skip_test", x.task)
        case .landNow(let x)?: return ("land_now", x.task)
        case .landNowBatch(let x)?: return ("land_now_batch", batchKey(x.batch))
        case .force(let x)?: return ("force", x.task)
        case .unblock(let x)?: return ("unblock", x.task)
        case .block(let x)?: return ("block", x.task)
        case .revive(let x)?: return ("revive", x.task)
        case .defer(let x)?: return ("defer", x.task)
        case .undefer(let x)?: return ("undefer", x.task)
        case .setPlacement(let x)?:
            let scope: PlacementScope = x.scope == "task" ? .task(x.task) : x.scope == "project" ? .project(x.project) : .global
            return ("set_placement", placementKey(scope))
        default: return nil
        }
    }
}

extension MonitorModel {
    /// The pending command for this action on this task, if any.
    public func pendingCommand(task: UInt64, action: String) -> PendingCommand? {
        pending.values.first { $0.task == task && $0.action == action }
    }

    /// What a control for this action should say instead of its name; nil when nothing is pending.
    public func pendingLabel(task: UInt64, action: String) -> String? {
        pendingCommand(task: task, action: action)?.label
    }

    /// True while any command about this task is pending: its other controls are disabled too.
    public func isBusy(task: UInt64) -> Bool {
        pending.values.contains { $0.task == task }
    }

    /// The daemon's refusal of the last command about this task, shown beside its controls.
    public func actionError(task: UInt64) -> String? { actionErrors[task] }

    /// The sidebar's row for a task, so its page can be drawn before masond sends the task view.
    public func sidebarRow(_ task: UInt64) -> TaskRow? {
        for s in liveSidebar?.sections ?? [] {
            if let r = s.rows.first(where: { $0.id == task }) { return r }
        }
        return nil
    }

    /// The state and allowed actions the sidebar (or the open task page) shows for a task.
    func signature(task: UInt64) -> String {
        var row = sidebarRow(task)
        if row == nil, let d = taskDetail, d.hasRow, d.row.id == task { row = d.row }
        guard let r = row else { return "" }
        return r.state + "|" + r.allowedActions.joined(separator: ",")
    }

    /// Records the command as pending. False when one for the same control is already in flight.
    func beginPending(_ c: inout Mason_V1_Command) -> Bool {
        guard c.id.isEmpty, let (action, task) = PendingCommand.describe(c) else { return true }
        if pending.values.contains(where: { $0.task == task }) { return false }
        nextActionId += 1
        c.id = "act:\(action):\(task):\(nextActionId)"
        actionErrors[task] = nil
        pending[c.id] = PendingCommand(id: c.id, action: action, task: task, sent: Date(), baseline: signature(task: task))
        return true
    }

    func settle(result r: Mason_V1_CommandResult) {
        guard var p = pending[r.id] else { return }
        if !r.ok {
            pending[r.id] = nil
            actionErrors[p.task] = r.error
        } else if p.task != 0 && signature(task: p.task) != p.baseline {
            pending[r.id] = nil
        } else {
            p.stage = .accepted
            pending[r.id] = p
        }
    }

    /// A sidebar or task view arrived: an accepted command ends when its task changed, or after
    /// `viewsToWait` views that did not change it.
    func settleAfterView() {
        for (id, var p) in pending where p.stage == .accepted {
            p.viewsSeen += 1
            if (p.task != 0 && signature(task: p.task) != p.baseline) || p.viewsSeen >= PendingCommand.viewsToWait {
                pending[id] = nil
            } else {
                pending[id] = p
            }
        }
    }

    func markPendingNotSent() {
        for (id, var p) in pending where p.stage == .sending {
            p.stage = .notSent
            pending[id] = p
        }
    }
}
