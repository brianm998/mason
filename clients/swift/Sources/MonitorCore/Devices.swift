// SPDX-License-Identifier: GPL-3.0-or-later

import Foundation

/// `GET /lan`: is the LAN listener up, and the names it can be reached at (s433).
public struct LanStatus: Equatable, Sendable {
    public var enabled = false
    public var hosts: [String] = []
    public var port = 0

    public init() {}

    public init(json: String) {
        guard let d = json.data(using: .utf8),
              let v = try? JSONSerialization.jsonObject(with: d) as? [String: Any] else { return }
        enabled = v["enabled"] as? Bool ?? false
        hosts = v["hosts"] as? [String] ?? []
        port = v["port"] as? Int ?? 0
    }

    /// What to tell the owner when the listener is off.
    public static let howToTurnOn = "LAN access is off. Set `[lan] enabled = true` in daemon.toml, or start masond with --lan, then restart it."
}

public struct PairedDevice: Equatable, Sendable {
    public var id: UInt64
    public var name: String
    public var role: String
    public var createdAt: UInt64
    public var lastSeen: UInt64?
    public var revokedAt: UInt64?
    public var isRevoked: Bool { revokedAt != nil }
}

/// `GET /devices`: live devices (most recently seen first, never-seen last) and revoked ones
/// shown apart (most recently revoked first).
public struct DeviceList: Equatable, Sendable {
    public var active: [PairedDevice] = []
    public var revoked: [PairedDevice] = []

    public init() {}

    public init(json: String) {
        guard let d = json.data(using: .utf8),
              let v = try? JSONSerialization.jsonObject(with: d) as? [String: Any] else { return }
        let rows: [PairedDevice] = (v["devices"] as? [[String: Any]] ?? []).compactMap {
            guard let id = ($0["id"] as? NSNumber)?.uint64Value else { return nil }
            return PairedDevice(
                id: id, name: $0["name"] as? String ?? "device \(id)", role: $0["role"] as? String ?? "?",
                createdAt: ($0["created_at"] as? NSNumber)?.uint64Value ?? 0,
                lastSeen: ($0["last_seen"] as? NSNumber)?.uint64Value,
                revokedAt: ($0["revoked_at"] as? NSNumber)?.uint64Value)
        }
        active = rows.filter { !$0.isRevoked }.sorted {
            switch ($0.lastSeen, $1.lastSeen) {
            case let (a?, b?) where a != b: return a > b
            case (.some, nil): return true
            case (nil, .some): return false
            default: return $0.id < $1.id
            }
        }
        revoked = rows.filter { $0.isRevoked }.sorted {
            let (a, b) = ($0.revokedAt ?? 0, $1.revokedAt ?? 0)
            return a != b ? a > b : $0.id < $1.id
        }
    }

    /// "never", "just now", "5 min ago", "3 h ago", "2 d ago".
    public static func lastSeenText(_ ms: UInt64?, nowMs: UInt64) -> String {
        guard let ms else { return "never" }
        let s = nowMs > ms ? (nowMs - ms) / 1000 : 0
        if s < 60 { return "just now" }
        if s < 3600 { return "\(s / 60) min ago" }
        if s < 86400 { return "\(s / 3600) h ago" }
        return "\(s / 86400) d ago"
    }
}

/// The answer to `POST /pair/codes`, with the moment it arrived so the countdown can run on an
/// injected clock.
public struct PairOffer: Equatable, Sendable {
    public var code: String
    public var role: String
    public var url: String
    public var hosts: [String]
    public var expiresInS: Int
    public var issuedAtMs: UInt64

    public enum State: Equatable, Sendable {
        case valid(secondsLeft: Int)
        case expired
    }

    public init(code: String, role: String, url: String, hosts: [String], expiresInS: Int, issuedAtMs: UInt64) {
        self.code = code; self.role = role; self.url = url; self.hosts = hosts
        self.expiresInS = expiresInS; self.issuedAtMs = issuedAtMs
    }

    public init?(json: String, nowMs: UInt64) {
        guard let d = json.data(using: .utf8),
              let v = try? JSONSerialization.jsonObject(with: d) as? [String: Any],
              let code = v["code"] as? String, Pin.isCode(code),
              let url = v["url"] as? String, !url.isEmpty else { return nil }
        self.init(code: code, role: v["role"] as? String ?? "owner", url: url,
                  hosts: v["hosts"] as? [String] ?? [], expiresInS: v["expires_in_s"] as? Int ?? 0,
                  issuedAtMs: nowMs)
    }

    /// Whole seconds left, rounded up, while the code works; `.expired` from the deadline on.
    public func state(nowMs: UInt64) -> State {
        let end = issuedAtMs + UInt64(max(0, expiresInS)) * 1000
        if nowMs >= end { return .expired }
        return .valid(secondsLeft: Int((end - nowMs + 999) / 1000))
    }

    /// "4:59".
    public static func clock(_ seconds: Int) -> String {
        String(format: "%d:%02d", seconds / 60, seconds % 60)
    }
}
