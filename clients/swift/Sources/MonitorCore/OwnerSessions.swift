// SPDX-License-Identifier: GPL-3.0-or-later

import Foundation
import MasonProtocol

/// The owner's chat with an agent (s364) as the dialog shows it, and the sidebar's panel of
/// the open ones (s432). The daemon does the work; this reads what it says.
public struct OwnerSessionTranscript: Equatable, Sendable {
    public struct Message: Equatable, Sendable, Identifiable {
        public var id: UInt64
        /// `owner`, `agent` or `tool`.
        public var role: String
        public var text: String
    }
    public var id: UInt64 = 0
    public var kind = ""
    public var state = ""
    public var working = false
    public var messages: [Message] = []

    public init() {}

    /// From the body of `GET /sessions/{id}`.
    public init(json: String) {
        guard let d = json.data(using: .utf8),
              let o = (try? JSONSerialization.jsonObject(with: d) as? [String: Any])?["session"] as? [String: Any]
        else { return }
        id = (o["id"] as? NSNumber)?.uint64Value ?? 0
        kind = o["kind"] as? String ?? ""
        state = o["state"] as? String ?? ""
        working = o["working"] as? Bool ?? false
        messages = (o["messages"] as? [[String: Any]] ?? []).map {
            Message(id: ($0["id"] as? NSNumber)?.uint64Value ?? 0, role: $0["role"] as? String ?? "", text: $0["text"] as? String ?? "")
        }
    }
}

public enum ActiveSessions {
    /// The panel's title; nil when no session is open (the panel is hidden).
    public static func title(_ sessions: [Mason_V1_ActiveSession]) -> String? {
        sessions.isEmpty ? nil : "Active sessions (\(sessions.count))"
    }

    /// The marker beside a session: replying, waiting for the owner, or none.
    public static func marker(_ s: Mason_V1_ActiveSession) -> String {
        s.working ? "replying" : (s.waiting ? "waiting for you" : "")
    }
}
