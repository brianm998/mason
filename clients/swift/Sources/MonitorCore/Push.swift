// SPDX-License-Identifier: GPL-3.0-or-later

import Foundation

/// Which Apple push service the build's token belongs to. A debug build is signed with the
/// development entitlement and gets a sandbox token; TestFlight and App Store builds get a
/// production one. The daemon needs to know which host to send to.
public enum PushEnv: String, Equatable, Sendable {
    case sandbox
    case production

    /// From the compile-time flag: `DEBUG` is sandbox, anything else production.
    public static var current: PushEnv {
        #if DEBUG
        return .sandbox
        #else
        return .production
        #endif
    }
}

/// What a notification's `mason` dictionary says (the daemon's APNs payload: `kind`, `key`, `task`).
public struct PushPayload: Equatable, Sendable {
    public var kind: String
    public var key: String
    public var task: UInt64?

    public init(kind: String, key: String = "", task: UInt64? = nil) {
        self.kind = kind; self.key = key; self.task = task
    }

    /// Reads the notification's userInfo; nil when it carries no `mason` dictionary.
    public init?(userInfo: [AnyHashable: Any]) {
        guard let m = userInfo["mason"] as? [String: Any] else { return nil }
        kind = m["kind"] as? String ?? ""
        key = m["key"] as? String ?? ""
        switch m["task"] {
        case let n as NSNumber: task = n.int64Value >= 0 ? UInt64(n.int64Value) : nil
        case let s as String: task = UInt64(s)
        default: task = nil
        }
    }

    /// The page a tap opens: the task's page when the notification names a task, otherwise Home
    /// (a condition without a task, a kind this build does not know).
    public var page: Page { task.map(Page.task) ?? .home }

    /// Whether the app, in the foreground, should still show this as a banner: only when the
    /// page it points at is not the one on screen.
    public func showsBanner(whileShowing current: Page) -> Bool { current != page }
}

/// One call to the daemon's push routes.
public struct PushRequest: Equatable, Sendable {
    public var method: String
    public var path: String
    public var body: [String: String]?
}

/// Sends a request to the paired daemon over the pinned session. The real one is
/// `PinnedPushTransport`; tests record.
public protocol PushTransport: Sendable {
    func send(_ r: PushRequest) async throws
}

/// What is remembered between launches: whether notifications are on for this device, whether
/// the explanation was shown, and the registration last sent (so an unchanged token is not re-sent).
@MainActor
public protocol PushPrefs: AnyObject {
    var enabled: Bool { get set }
    var explained: Bool { get set }
    var lastSent: String? { get set }
}

@MainActor
public final class MemoryPushPrefs: PushPrefs {
    public var enabled = true
    public var explained = false
    public var lastSent: String?
    public init() {}
}

@MainActor
public final class DefaultsPushPrefs: PushPrefs {
    private let defaults: UserDefaults
    public init(_ defaults: UserDefaults = .standard) { self.defaults = defaults }
    public var enabled: Bool {
        get { defaults.object(forKey: "mason.push.enabled") as? Bool ?? true }
        set { defaults.set(newValue, forKey: "mason.push.enabled") }
    }
    public var explained: Bool {
        get { defaults.bool(forKey: "mason.push.explained") }
        set { defaults.set(newValue, forKey: "mason.push.explained") }
    }
    public var lastSent: String? {
        get { defaults.string(forKey: "mason.push.last-sent") }
        set { defaults.set(newValue, forKey: "mason.push.last-sent") }
    }
}

/// Keeps the daemon's idea of this device's push token right: sends it when it is new or has
/// changed, removes it when notifications are turned off or the daemon is forgotten.
@MainActor
public final class PushRegistrar: ObservableObject {
    /// The explanation screen should show (before the system permission prompt).
    @Published public private(set) var needsExplanation = false
    @Published public private(set) var testResult: String?

    private let prefs: PushPrefs
    private let env: PushEnv
    private let transport: () -> PushTransport?
    private let fingerprint: () -> String?
    private let requestRegistration: () -> Void
    private var token: String?

    /// - transport: for the paired daemon, nil when unpaired.
    /// - fingerprint: the paired daemon's, so a new pairing sends the token again.
    /// - requestRegistration: asks the system for a token (`registerForRemoteNotifications`).
    public init(prefs: PushPrefs, env: PushEnv = .current, transport: @escaping () -> PushTransport?,
                fingerprint: @escaping () -> String?, requestRegistration: @escaping () -> Void) {
        self.prefs = prefs; self.env = env; self.transport = transport
        self.fingerprint = fingerprint; self.requestRegistration = requestRegistration
    }

    public var enabled: Bool { prefs.enabled }

    /// The token as the daemon takes it: lowercase hex.
    public static func hex(_ data: Data) -> String { data.map { String(format: "%02x", $0) }.joined() }

    /// On each launch, once paired: ask once with an explanation, then register.
    public func launched() {
        guard prefs.enabled, fingerprint() != nil else { return }
        if prefs.explained { requestRegistration() } else { needsExplanation = true }
    }

    /// The explanation screen was answered. Either way it is not shown again; "Not now" leaves
    /// notifications off for this device.
    public func explanationAnswered(allow: Bool) {
        needsExplanation = false
        prefs.explained = true
        if allow { prefs.enabled = true; requestRegistration() } else { prefs.enabled = false }
    }

    /// The system gave a device token.
    public func didRegister(deviceToken: Data) async {
        token = Self.hex(deviceToken)
        await sendIfChanged()
    }

    /// Sends the token unless the daemon already has this one for this pairing.
    public func sendIfChanged() async {
        guard prefs.enabled, let token, let t = transport(), let fp = fingerprint() else { return }
        let sent = "\(env.rawValue):\(token)@\(fp)"
        guard prefs.lastSent != sent else { return }
        do {
            try await t.send(PushRequest(method: "POST", path: "/devices/self/push", body: ["token": token, "env": env.rawValue]))
            prefs.lastSent = sent
        } catch {}
    }

    /// Settings: notifications on or off for this device.
    public func setEnabled(_ on: Bool) async {
        prefs.enabled = on
        if on {
            prefs.explained = true
            if token == nil { requestRegistration() } else { await sendIfChanged() }
        } else {
            await clear()
        }
    }

    /// DELETE the registration; best effort, and the next send starts clean either way.
    public func clear() async {
        prefs.lastSent = nil
        guard let t = transport() else { return }
        try? await t.send(PushRequest(method: "DELETE", path: "/devices/self/push", body: nil))
    }

    /// Settings: ask the daemon to send a test push.
    public func sendTest() async {
        guard let t = transport() else { testResult = "Not paired."; return }
        do {
            try await t.send(PushRequest(method: "POST", path: "/notify/test", body: ["channel": "apns"]))
            testResult = "Sent. It should arrive in a moment."
        } catch {
            testResult = "The daemon could not send it: \(error.localizedDescription)"
        }
    }
}

#if canImport(CryptoKit)
public struct PushHTTPError: Error, LocalizedError, Sendable {
    public var status: Int
    public var message: String
    public var errorDescription: String? { message.isEmpty ? "HTTP \(status)" : message }
}

/// HTTPS to the paired daemon, pinned to its fingerprint, with the token. Tries each of the
/// daemon's names in turn until one answers.
public final class PinnedPushTransport: PushTransport, @unchecked Sendable {
    private let daemon: PairedDaemon

    public init(daemon: PairedDaemon) { self.daemon = daemon }

    public func send(_ r: PushRequest) async throws {
        let delegate = PinningDelegate(pinned: daemon.fingerprint)
        let session = URLSession(configuration: .ephemeral, delegate: delegate, delegateQueue: nil)
        defer { session.finishTasksAndInvalidate() }
        var last: Error = URLError(.cannotConnectToHost)
        for host in daemon.hosts {
            var req = URLRequest(url: URL(string: "https://\(Reach.urlHost(host)):\(daemon.port)\(r.path)")!)
            req.httpMethod = r.method
            req.timeoutInterval = 10
            req.setValue("Bearer \(daemon.token)", forHTTPHeaderField: "Authorization")
            if let b = r.body {
                req.setValue("application/json", forHTTPHeaderField: "Content-Type")
                req.httpBody = try JSONSerialization.data(withJSONObject: b)
            }
            do {
                let (data, resp) = try await session.data(for: req)
                let status = (resp as? HTTPURLResponse)?.statusCode ?? 0
                if (200..<300).contains(status) { return }
                let msg = ((try? JSONSerialization.jsonObject(with: data)) as? [String: Any])?["error"] as? String ?? ""
                throw PushHTTPError(status: status, message: msg)
            } catch let e as PushHTTPError {
                throw e
            } catch {
                last = error
            }
        }
        throw last
    }
}
#endif
