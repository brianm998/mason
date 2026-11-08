// SPDX-License-Identifier: GPL-3.0-or-later

import Foundation

/// What `daemon.json` says about the running daemon. `pid` and `startedMs` are
/// nil when the port was given by hand (`--port`), so there is nothing to watch.
public struct DaemonInfo: Equatable, Sendable {
    public var port: UInt16
    public var pid: Int?
    public var startedMs: Int?

    public init(port: UInt16, pid: Int? = nil, startedMs: Int? = nil) {
        self.port = port
        self.pid = pid
        self.startedMs = startedMs
    }

    /// A different daemon (or a restarted one) than `old`. A hand-given port has no
    /// identity to compare, so it never counts as changed.
    public func replaces(_ old: DaemonInfo) -> Bool {
        if port != old.port { return true }
        if let p = pid, let q = old.pid, p != q { return true }
        if let s = startedMs, let t = old.startedMs, s != t { return true }
        return false
    }
}

/// The pause before each retry: 1, 2, 4, 8 seconds, then every 10. A welcome
/// starts it over.
public struct ReconnectPolicy: Equatable, Sendable {
    public static let schedule: [TimeInterval] = [1, 2, 4, 8]
    public static let steady: TimeInterval = 10
    private var failures = 0
    /// Multiplies every pause; tests shrink it, the app leaves it at 1.
    private let scale: Double

    public init(scale: Double = 1) { self.scale = scale }

    public mutating func nextPause() -> TimeInterval {
        defer { failures += 1 }
        return (failures < Self.schedule.count ? Self.schedule[failures] : Self.steady) * scale
    }

    public mutating func reset() { failures = 0 }
}

/// How often the connection looks while it is up.
public struct ConnectionTiming: Sendable {
    /// Ask the daemon "are you there" this often.
    public var pingEvery: TimeInterval = 5
    /// No message and no Pong for this long is a stall: reconnect.
    public var stallAfter: TimeInterval = 15
    /// Re-read `daemon.json` this often while connected.
    public var watchEvery: TimeInterval = 2
    /// The pause before each retry.
    public var policy = ReconnectPolicy()

    public init() {}
}

/// One WebSocket, as the connection needs it. The real one wraps URLSession;
/// tests script a fake.
@MainActor
public protocol MonitorSocket: AnyObject {
    func open(port: UInt16)
    func send(_ text: String) async throws
    /// The next text frame; throws when the socket fails or is closed.
    func receive() async throws -> String
    func close()
}

@MainActor
final class URLSessionSocket: MonitorSocket {
    private let session = URLSession(configuration: .ephemeral)
    private var task: URLSessionWebSocketTask?

    func open(port: UInt16) {
        let t = session.webSocketTask(with: URL(string: "ws://127.0.0.1:\(port)/ws")!)
        task = t
        t.resume()
    }

    func send(_ text: String) async throws { try await task?.send(.string(text)) }

    func receive() async throws -> String {
        guard let t = task else { throw URLError(.cancelled) }
        switch try await t.receive() {
        case .string(let s): return s
        case .data(let d): return String(decoding: d, as: UTF8.self)
        @unknown default: return ""
        }
    }

    func close() {
        task?.cancel(with: .goingAway, reason: nil)
        session.invalidateAndCancel()
    }
}
