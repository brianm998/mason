// SPDX-License-Identifier: GPL-3.0-or-later

import Foundation
import MasonProtocol

/// What a Land now button on the landing queue is about: one task, or a whole running batch.
public enum LandNowTarget: Equatable, Sendable {
    case task(UInt64)
    case batch(UInt64)

    /// The one-line confirm shown before anything is sent.
    public var confirmText: String {
        switch self {
        case .task(let n): return "Land #\(n) now? Its remaining tests run after the merge; a red result raises an alert."
        case .batch(let n): return "Land batch #\(n) now? Its remaining tests run after the merge; a red result raises an alert."
        }
    }

    /// Where the pending command is kept (see `PendingCommand.batchKey`) and under what action.
    public var pendingKey: (task: UInt64, action: String) {
        switch self {
        case .task(let n): return (n, "land_now")
        case .batch(let n): return (PendingCommand.batchKey(n), "land_now_batch")
        }
    }
}

extension MonitorModel {
    /// Land now is the owner's, on a device that can act, and only where the daemon offers it.
    public func canLandNow(_ i: Mason_V1_TaskLanding) -> Bool {
        canAct && i.allowedActions.contains("land_now")
    }

    public func canLandNow(batch b: Mason_V1_BatchLine) -> Bool {
        canAct && b.running && b.allowedActions.contains("land_now")
    }

    /// What the button says while its command is pending; nil when none is.
    public func landNowLabel(_ t: LandNowTarget) -> String? {
        let k = t.pendingKey
        return pendingLabel(task: k.task, action: k.action)
    }

    /// The button was pressed: ask first.
    public func askLandNow(_ t: LandNowTarget) { landNowAsk = t }

    public func cancelLandNow() { landNowAsk = nil }

    /// "Land now" in the confirm: send the command for what was asked.
    public func confirmLandNow() {
        guard let t = landNowAsk else { return }
        landNowAsk = nil
        switch t {
        case .task(let n): landNow(n)
        case .batch(let n): landNowBatch(n)
        }
    }
}
