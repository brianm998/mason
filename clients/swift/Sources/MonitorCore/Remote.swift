// SPDX-License-Identifier: GPL-3.0-or-later

import Combine
import Foundation

/// A paired daemon as kept in `UserDefaults`. The token is not here: it stays in the Keychain,
/// keyed by the fingerprint.
public struct PairedRecord: Equatable, Sendable {
    public var host: String
    public var port: UInt16
    public var fingerprint: String
    public var role: String
    public var alternates: [String]

    public init(host: String, port: UInt16, fingerprint: String, role: String, alternates: [String] = []) {
        self.host = host; self.port = port; self.fingerprint = fingerprint; self.role = role; self.alternates = alternates
    }

    public init(_ d: PairedDaemon) {
        self.init(host: d.host, port: d.port, fingerprint: d.fingerprint, role: d.role, alternates: d.alternates)
    }
}

/// Where the one paired daemon is remembered. Pairing writes it; the connection reads it and
/// clears it when the daemon says the token is revoked.
@MainActor
public protocol PairedStore: AnyObject {
    func load() -> PairedRecord?
    func save(_ r: PairedRecord)
    func clear()
}

@MainActor
public final class MemoryPairedStore: PairedStore {
    public private(set) var record: PairedRecord?
    public init(_ r: PairedRecord? = nil) { record = r }
    public func load() -> PairedRecord? { record }
    public func save(_ r: PairedRecord) { record = r }
    public func clear() { record = nil }
}

/// `UserDefaults`, under one key. One paired daemon at a time: saving replaces.
@MainActor
public final class DefaultsPairedStore: PairedStore {
    public static let key = "mason.paired-daemon"
    private let defaults: UserDefaults
    public init(defaults: UserDefaults = .standard) { self.defaults = defaults }

    public func load() -> PairedRecord? {
        guard let d = defaults.dictionary(forKey: Self.key),
              let host = d["host"] as? String, !host.isEmpty,
              let port = (d["port"] as? Int).flatMap({ UInt16(exactly: $0) }),
              let fp = (d["fp"] as? String)?.lowercased(), Pin.isFingerprint(fp),
              let role = d["role"] as? String else { return nil }
        return PairedRecord(host: host, port: port, fingerprint: fp, role: role, alternates: d["alternates"] as? [String] ?? [])
    }

    public func save(_ r: PairedRecord) {
        defaults.set(["host": r.host, "port": Int(r.port), "fp": r.fingerprint, "role": r.role, "alternates": r.alternates] as [String: Any], forKey: Self.key)
    }

    public func clear() { defaults.removeObject(forKey: Self.key) }
}

/// A Bonjour record that has been resolved to an address.
public struct ResolvedService: Equatable, Sendable {
    public var record: ServiceRecord
    public var host: String
    public var port: UInt16
    public init(record: ServiceRecord, host: String, port: UInt16) {
        self.record = record; self.host = host; self.port = port
    }
}

/// What the network browse has found so far. The real one is `BonjourServiceSource`; tests script one.
public protocol ServiceSource: AnyObject, Sendable {
    func services() -> [ResolvedService]
}

/// One address to try.
public struct Endpoint: Equatable, Sendable {
    public var host: String
    public var port: UInt16
    /// Found by Bonjour (as opposed to a name the pairing gave).
    public var viaBonjour: Bool
}

/// Chooses the endpoint for each connection attempt. On an attempt it looks at what Bonjour has
/// found for the paired fingerprint (a record with any other fingerprint is ignored), puts that
/// first, then walks `Reach.order`. An attempt that failed moves to the next endpoint; one that
/// was welcomed keeps its endpoint for the next. While connected, the endpoint does not change.
public final class RemoteDiscovery: @unchecked Sendable {
    private let paired: PairedDaemon
    private let source: ServiceSource?
    private let lock = NSLock()
    private var cursor = 0
    private var chosen: Endpoint?
    private var chooseHandler: (@Sendable (Endpoint, ServiceRecord?) -> Void)?

    public init(paired: PairedDaemon, source: ServiceSource?) {
        self.paired = paired
        self.source = source
    }

    public func onChoose(_ f: @escaping @Sendable (Endpoint, ServiceRecord?) -> Void) {
        lock.lock(); chooseHandler = f; lock.unlock()
    }

    /// The matching Bonjour answer, if any: same fingerprint as the pairing.
    func match() -> ResolvedService? {
        source?.services().first { Pin.matches(shown: $0.record.fingerprint, pinned: paired.fingerprint) }
    }

    /// Every endpoint, in the order to try them.
    public func candidates() -> [Endpoint] {
        let found = match()
        return Reach.order(paired: paired, bonjour: found?.host).map { h in
            if let f = found, f.host == h { return Endpoint(host: h, port: f.port, viaBonjour: true) }
            return Endpoint(host: h, port: paired.port, viaBonjour: false)
        }
    }

    /// The endpoint for the attempt in hand: asked at its start and again while connected, and
    /// it answers the same until `attemptEnded`.
    public func current() -> DaemonInfo? {
        lock.lock()
        if let c = chosen { lock.unlock(); return DaemonInfo(port: c.port, host: c.host) }
        lock.unlock()
        let list = candidates()
        guard !list.isEmpty else { return nil }
        lock.lock()
        let e = list[cursor % list.count]
        chosen = e
        let f = chooseHandler
        lock.unlock()
        f?(e, match()?.record)
        return DaemonInfo(port: e.port, host: e.host)
    }

    /// The attempt is over. After one that never got a welcome, the next tries the next endpoint.
    public func attemptEnded(welcomed: Bool) {
        lock.lock()
        if welcomed { cursor = 0 } else { cursor += 1 }
        chosen = nil
        lock.unlock()
    }
}

/// What the start and banner views say about the remote link. Pure.
public enum LinkStatus {
    public static func looking(instance: String) -> String { "Looking for \(instance)…" }
    public static func reached(host: String) -> String { "Reached through \(host)" }
    public static func instance(host: String, record: ServiceRecord?) -> String { record?.instance ?? "MASON on \(host)" }
}

public enum RemoteState: Equatable, Sendable {
    /// Nothing is paired: the pairing screen shows.
    case unpaired
    /// Paired, and the connection is running (or waiting for the scene to become active).
    case paired
}

#if canImport(CryptoKit)
/// The monitor's link to the one paired daemon, driven by the app's lifecycle: `activate()` when
/// the scene becomes active, `deactivate()` when it goes to the background.
@MainActor
public final class RemoteSession: ObservableObject {
    @Published public private(set) var state: RemoteState = .unpaired
    private let model: MonitorModel
    private let store: PairedStore
    private let tokens: TokenStore
    private let source: ServiceSource?
    private let makeSocket: (PairedDaemon) -> MonitorSocket
    private let timing: ConnectionTiming
    private var connection: MonitorConnection?
    private var discovery: RemoteDiscovery?
    private var watch: AnyCancellable?
    private var instance = ""
    private var reachedHost: String?
    /// Told to delete this device's push registration before a Forget clears the pairing.
    public var push: PushRegistrar?

    public init(
        model: MonitorModel, store: PairedStore, tokens: TokenStore, source: ServiceSource?,
        timing: ConnectionTiming = ConnectionTiming(),
        makeSocket: ((PairedDaemon) -> MonitorSocket)? = nil
    ) {
        self.model = model
        self.store = store
        self.tokens = tokens
        self.source = source
        self.timing = timing
        self.makeSocket = makeSocket ?? { PinnedSocket(daemon: $0) }
        state = store.load() == nil ? .unpaired : .paired
    }

    /// The paired daemon with its token; nil when unpaired or the token is gone.
    public var paired: PairedDaemon? {
        guard let r = store.load(), let t = tokens.token(forFingerprint: r.fingerprint) else { return nil }
        return PairedDaemon(host: r.host, port: r.port, fingerprint: r.fingerprint, token: t, role: r.role, alternates: r.alternates)
    }

    /// The scene became active: connect (resuming from the last sequence number if this is a return).
    public func activate() {
        guard connection == nil else { connection?.start(); return }
        guard let d = paired else { forget(); return }
        state = .paired
        instance = LinkStatus.instance(host: d.host, record: nil)
        model.linkStatus = LinkStatus.looking(instance: instance)
        let discovery = RemoteDiscovery(paired: d, source: source)
        self.discovery = discovery
        discovery.onChoose { [weak self] e, rec in
            Task { @MainActor in self?.chose(e, rec) }
        }
        let socketFor = makeSocket
        let c = MonitorConnection(
            model: model, discover: { discovery.current() }, timing: timing,
            makeSocket: { socketFor(d) },
            onAttemptEnd: { discovery.attemptEnded(welcomed: $0) },
            onRefused: { [weak self] _ in self?.revoked() })
        connection = c
        watch = model.$connected.sink { [weak self] up in
            guard let self else { return }
            if up, let h = self.reachedHost { self.model.linkStatus = LinkStatus.reached(host: h) }
            else if !up, self.connection != nil { self.model.linkStatus = LinkStatus.looking(instance: self.instance) }
        }
        c.start()
    }

    /// The scene went to the background.
    public func deactivate() {
        connection?.suspend()
        // The next attempt looks again: the network may be another one by then.
        discovery?.attemptEnded(welcomed: true)
    }

    /// Pairing wrote a new daemon: connect to it instead.
    public func pairingChanged() {
        dropConnection()
        activate()
    }

    /// The user unpairs. With push set up, the daemon is told first (DELETE), then the pairing is
    /// cleared; the token is still needed for that call.
    public func unpair() {
        guard let push else { finishUnpair(); return }
        Task { @MainActor in
            await push.clear()
            finishUnpair()
        }
    }

    private func finishUnpair() {
        if let r = store.load() { tokens.setToken(nil, forFingerprint: r.fingerprint) }
        forget()
    }

    private func chose(_ e: Endpoint, _ rec: ServiceRecord?) {
        reachedHost = e.host
        if let rec { instance = rec.instance }
        if !model.connected { model.linkStatus = LinkStatus.looking(instance: instance) }
    }

    /// The daemon answered 401: the token was revoked. Forget the pairing; the pairing screen shows.
    private func revoked() { finishUnpair() }

    private func forget() {
        if let r = store.load() { tokens.setToken(nil, forFingerprint: r.fingerprint) }
        dropConnection()
        store.clear()
        model.linkStatus = nil
        state = .unpaired
    }

    private func dropConnection() {
        connection?.stop()
        connection = nil
        discovery = nil
        watch = nil
        reachedHost = nil
    }
}
#endif
