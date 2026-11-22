// SPDX-License-Identifier: GPL-3.0-or-later

import Foundation
import MasonProtocol
import SwiftProtobuf

final class WelcomeBox { var value = false }

/// Turns one text frame into a message. Runs off the main thread, so it must not touch the model.
public typealias MessageDecoder = @Sendable (String) -> Mason_V1_ServerMessage?

/// Carries a value that is only ever used from one place at a time across a task boundary.
struct UncheckedBox<T>: @unchecked Sendable { let value: T }

extension MonitorConnection {
    /// The JSON (proto3 mapping) decoder, tolerant of fields this build does not know.
    public static let jsonDecoder: MessageDecoder = { text in
        var opts = JSONDecodingOptions()
        opts.ignoreUnknownFields = true
        return try? Mason_V1_ServerMessage(jsonString: text, options: opts)
    }
}

/// The monitor's one WebSocket to `masond`. It says hello with the last
/// sequence number it saw, so a reconnect costs only what was missed, and it
/// reconnects by itself: 1, 2, 4, 8 seconds, then every 10, re-reading
/// `daemon.json` each time, so a restarted daemon is found on its new port.
/// A changed pid or start time, or 15 seconds of silence, also reconnects. The protocol is the JSON
/// encoding (the proto3 mapping), spoken with the generated types.
@MainActor
public final class MonitorConnection {
    private enum Down: Error { case noDaemon, restarted, stalled }

    private let model: MonitorModel
    private let discover: () -> DaemonInfo?
    private let makeSocket: () -> MonitorSocket
    private let decode: MessageDecoder
    private var timing: ConnectionTiming
    private var socket: MonitorSocket?
    private var running = false
    private var nextId = 1
    private var heard = Date()
    private var pings: UInt64 = 0
    /// Bumped by every `start`, so a loop left over from before a `stop` ends instead of racing the new one.
    private var generation = 0
    private let onAttemptEnd: ((Bool) -> Void)?
    private let onRefused: ((Int) -> Void)?

    /// `discover` is asked again after every failure and while connected, so a
    /// restarted daemon is found wherever it now listens.
    public init(
        model: MonitorModel,
        discover: @escaping () -> DaemonInfo?,
        timing: ConnectionTiming = ConnectionTiming(),
        makeSocket: (() -> MonitorSocket)? = nil,
        decode: MessageDecoder? = nil,
        onAttemptEnd: ((Bool) -> Void)? = nil,
        onRefused: ((Int) -> Void)? = nil
    ) {
        self.onAttemptEnd = onAttemptEnd
        self.onRefused = onRefused
        self.decode = decode ?? MonitorConnection.jsonDecoder
        self.model = model
        self.discover = discover
        self.timing = timing
        self.makeSocket = makeSocket ?? { URLSessionSocket() }
        model.sender = { [weak self] c in self?.send(command: c) }
        model.subscribe = { [weak self] in self?.sendSubscription() }
    }

    /// A fixed port, for callers that were given one.
    public convenience init(model: MonitorModel, port: UInt16) {
        self.init(model: model, discover: { DaemonInfo(port: port) })
    }

    public func start() {
        guard !running else { return }
        running = true
        generation += 1
        let g = generation
        Task { await self.loop(g) }
    }

    public func stop() {
        running = false
        socket?.close()
    }

    /// The app went to the background: drop the socket (iOS would anyway) and show it as down.
    /// `start()` on return says hello with the last sequence number again.
    public func suspend() {
        stop()
        model.connected = false
        model.reconnecting = true
    }

    private func loop(_ g: Int) async {
        while running, g == generation {
            var welcomed = false
            do {
                try await session(welcomed: &welcomed)
            } catch {
                // Down for any reason: show "reconnecting", not an error.
                if g == generation, running { model.disconnected() }
            }
            guard running, g == generation else { break }
            onAttemptEnd?(welcomed)
            if let code = socket?.refusal, code == 401 {
                // The daemon no longer knows this token: stop retrying and say so.
                running = false
                onRefused?(code)
                break
            }
            if welcomed { timing.policy.reset() }
            let pause = timing.policy.nextPause()
            try? await Task.sleep(nanoseconds: UInt64(pause * 1_000_000_000))
        }
    }

    /// One connection, from open to the first failure. Returns by throwing.
    private func session(welcomed: inout Bool) async throws {
        guard let info = await findDaemon() else { throw Down.noDaemon }
        let s = makeSocket()
        socket = s
        s.open(info: info)
        defer { s.close() }
        try await hello(s)
        sendSubscription()
        heard = Date()
        let box = WelcomeBox()
        let decode = self.decode
        let timing = self.timing
        do {
            try await withThrowingTaskGroup(of: Void.self) { g in
                // Reading and decoding run off the main actor; only the finished message comes back.
                g.addTask {
                    while true {
                        let text = try await s.receive()
                        await self.deliver(decode(text), welcome: box)
                    }
                }
                // The daemon.json read is file I/O, so it also runs off the main actor.
                g.addTask {
                    var lastPing = Date()
                    let tick = min(timing.watchEvery, timing.pingEvery)
                    while true {
                        try await Task.sleep(nanoseconds: UInt64(tick * 1_000_000_000))
                        let now = await self.findDaemon()
                        if let ping = try await self.watch(info: info, now: now, lastPing: lastPing) {
                            lastPing = Date()
                            try await s.send(ping)
                        }
                    }
                }
                defer { s.close() }
                do { try await g.next() } catch { s.close(); g.cancelAll(); throw error }
            }
        } catch {
            welcomed = box.value
            throw error
        }
    }

    /// `discover` on a background thread: it reads `daemon.json`.
    private func findDaemon() async -> DaemonInfo? {
        let d = UncheckedBox(value: discover)
        return await Task.detached { d.value() }.value
    }

    /// A decoded message (nil when the frame did not decode) arrives on the main actor, in order.
    private func deliver(_ msg: Mason_V1_ServerMessage?, welcome: WelcomeBox) {
        heard = Date()
        guard let msg else { return }
        if case .welcome? = msg.body { welcome.value = true }
        model.apply(msg)
    }

    /// One watch tick on the main actor: throws when the daemon restarted or went quiet;
    /// returns the text of a ping when one is due.
    private func watch(info: DaemonInfo, now: DaemonInfo?, lastPing: Date) throws -> String? {
        if let now, info.pid != nil || info.startedMs != nil, now.replaces(info) { throw Down.restarted }
        if Date().timeIntervalSince(heard) > timing.stallAfter { throw Down.stalled }
        guard Date().timeIntervalSince(lastPing) >= timing.pingEvery else { return nil }
        pings += 1
        var p = Mason_V1_Ping()
        p.nonce = pings
        var m = Mason_V1_ClientMessage()
        m.ping = p
        return try m.jsonString()
    }

    private func hello(_ t: MonitorSocket) async throws {
        var h = Mason_V1_Hello()
        h.protocol = 2
        h.lastSeq = model.lastSeq
        h.client = "mason-monitor-swiftui"
        h.clientVersion = model.ownVersion
        h.actor = model.identity
        var m = Mason_V1_ClientMessage()
        m.hello = h
        try await t.send(try m.jsonString())
    }

    /// Which views to keep sending, given the page being shown.
    public func sendSubscription() {
        var views: [Mason_V1_ViewKind] = [.header, .sidebar, .home, .usage]
        var taskId: UInt64 = 0
        if !model.visible { views = [.header, .sidebar] }
        else { switch model.page {
        case .task(let id): views.append(.taskDetail); taskId = id
        case .landing: views.append(.landing)
        case .home, .section("cpu"), .section("disk"): views.append(contentsOf: [.cpuHistory, .diskHistory])
        case .pipeline: views.append(.editor)
        case .health:
            model.raw("GET", "/resources")
            model.raw("GET", "/health")
        case .settings:
            model.loadSettings()
        default: break
        } }
        var s = Mason_V1_Subscribe()
        s.views = views
        s.project = model.project
        s.task = taskId
        s.editorStep = model.editorStep
        s.cpuSpanMs = model.cpu.span.ms
        s.rowBodies = model.search.wantsRowBodies
        s.diskSpanMs = model.disk.span.ms
        var m = Mason_V1_ClientMessage()
        m.subscribe = s
        write(m)
    }

    public func send(command c: Mason_V1_Command) {
        var c = c
        if c.id.isEmpty {
            c.id = "m\(nextId)"
            nextId += 1
        }
        if case .raw(let r)? = c.body, r.path == "/resources" { c.id = "health" }
        if case .raw(let r)? = c.body, r.path == "/health" { c.id = "introspection" }
        if case .raw(let r)? = c.body, r.path == "/announce/claim" { c.id = "announce" }
        var m = Mason_V1_ClientMessage()
        m.command = c
        write(m)
    }

    private func write(_ m: Mason_V1_ClientMessage) {
        guard let t = socket, let text = try? m.jsonString() else { return }
        Task { try? await t.send(text) }
    }
}
