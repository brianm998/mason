// SPDX-License-Identifier: GPL-3.0-or-later

import Foundation

/// The logic of the diagnosis card and the context field (#1046), apart from any window so it is
/// tested without one. The Compose client has the same rules (`DiagnosisCard.kt`).

/// What a click sends from a decision card: the answer (an option's number or free text) and the
/// owner's added context, which is empty when there is none.
public struct AnswerRequest: Equatable, Sendable {
    public let answer: String
    public let context: String

    /// An option clicked, or Return on the selected one. `option` is 0-based.
    public static func option(_ option: Int, context: String) -> AnswerRequest {
        AnswerRequest(answer: String(option + 1), context: contextText(context) ?? "")
    }

    /// The "Other" box: free text, with the context field's text as `context` when there is any.
    /// Nil when the free text is blank.
    public static func other(_ text: String, context: String) -> AnswerRequest? {
        otherAnswer(text).map { AnswerRequest(answer: $0, context: contextText(context) ?? "") }
    }

    /// The checked options of a multi-select card (already "1,3").
    public static func multi(_ answer: String, context: String) -> AnswerRequest {
        AnswerRequest(answer: answer, context: contextText(context) ?? "")
    }
}

/// The context field's text, trimmed; nil when it is blank (an empty context is not sent).
public func contextText(_ text: String) -> String? { otherAnswer(text) }

/// What a failed task's page shows first.
public enum FailedPage: Equatable, Sendable {
    /// The diagnosis card, with the failure line under it.
    case card
    /// A diagnosis is running: "Diagnosing on …", in place of the card.
    case diagnosing
    /// No diagnosis: the buttons as before, plus Diagnose when it is offered.
    case buttons(diagnose: Bool)

    /// "Other actions" (Retry, Cancel, Skip all testing) is collapsed by default behind a card,
    /// and shown under a running diagnosis only as a collapsed row too; with no diagnosis the
    /// buttons are the page itself.
    public var otherActionsCollapsed: Bool {
        switch self {
        case .card, .diagnosing: return true
        case .buttons: return false
        }
    }

    /// Chooses the presentation from what the task view carries.
    public static func choose(hasCard: Bool, diagnosing: Bool, actions: [String]) -> FailedPage {
        if hasCard { return .card }
        if diagnosing { return .diagnosing }
        return .buttons(diagnose: actions.contains("diagnose"))
    }

    /// Actions that stay visible outside the collapsed "Other actions" row.
    public static let alwaysVisible: Set<String> = ["open_session", "diagnose", "answer"]

    /// Whether `action` goes under "Other actions" for this presentation.
    public func hidesUnderOther(_ action: String) -> Bool {
        otherActionsCollapsed && !Self.alwaysVisible.contains(action)
    }
}

/// "Diagnosing on Opus medium… started 3 min ago".
public func diagnosingLine(startedMs: UInt64, ranOn: String, now: UInt64) -> String {
    let on = ranOn.isEmpty ? "" : " on \(ranOn)"
    let ago = now > startedMs ? "\(Format.duration(ms: now - startedMs)) ago" : "just now"
    return "Diagnosing\(on)… started \(ago)"
}

/// What a failed task's tile and sidebar row add next to its activity ("idle"):
/// "diagnosis ready", "diagnosing", or nothing.
public func diagnosisBadge(_ diagnosis: String) -> String? {
    switch diagnosis {
    case "ready": return "diagnosis ready"
    case "running": return "diagnosing"
    default: return nil
    }
}
