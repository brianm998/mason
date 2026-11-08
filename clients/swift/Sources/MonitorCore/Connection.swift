// SPDX-License-Identifier: GPL-3.0-or-later

import Foundation
import MasonProtocol
import SwiftProtobuf

final class WelcomeBox { var value = false }

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
    private var timing: ConnectionTiming
    private var socket: MonitorSocket?
    private var running = false
    private var nextId = 1
    private var heard = Date()
    private var pings: UInt64 = 0

    /// `discover` is asked again after every failure and while connected, so a
    /// restarted daemon is found wherever it now listens.
    public init(
        model: MonitorModel,
        discover: @escaping () -> DaemonInfo?,
        timing: ConnectionTiming = ConnectionTiming(),
        makeSocket: (() -> MonitorSocket)? = nil
    ) {
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
        Task { await self.loop() }
    }

    public func stop() {
        running = false
        socket?.close()
    }

    private func loop() async {
        while running {
            var welcomed = false
            do {
                try await session(welcomed: &welcomed)
            } catch {
                // Down for any reason: show "reconnecting", not an error.
                model.disconnected()
            }
            guard running else { break }
            if welcomed { timing.policy.reset() }
            let pause = timing.policy.nextPause()
            try? await Task.sleep(nanoseconds: UInt64(pause * 1_000_000_000))
        }
    }

    /// One connection, from open to the first failure. Returns by throwing.
    private func session(welcomed: inout Bool) async throws {
        guard let info = discover() else { throw Down.noDaemon }
        let s = makeSocket()
        socket = s
        s.open(port: info.port)
        defer { s.close() }
        try await hello(s)
        sendSubscription()
        heard = Date()
        let box = WelcomeBox()
        do {
            try await withThrowingTaskGroup(of: Void.self) { g in
                g.addTask { @MainActor in
                    while true {
                        let text = try await s.receive()
                        self.heard = Date()
                        var opts = JSONDecodingOptions()
                        opts.ignoreUnknownFields = true
                        if let msg = try? Mason_V1_ServerMessage(jsonString: text, options: opts) {
                            if case .welcome? = msg.body { box.value = true }
                            self.model.apply(msg)
                        }
                    }
                }
                g.addTask { @MainActor in
                    var lastPing = Date()
                    let tick = min(self.timing.watchEvery, self.timing.pingEvery)
                    while true {
                        try await Task.sleep(nanoseconds: UInt64(tick * 1_000_000_000))
                        if let now = self.discover(), info.pid != nil || info.startedMs != nil, now.replaces(info) {
                            throw Down.restarted
                        }
                        if Date().timeIntervalSince(self.heard) > self.timing.stallAfter { throw Down.stalled }
                        if Date().timeIntervalSince(lastPing) >= self.timing.pingEvery {
                            lastPing = Date()
                            self.pings += 1
                            var p = Mason_V1_Ping()
                            p.nonce = self.pings
                            var m = Mason_V1_ClientMessage()
                            m.ping = p
                            try await s.send(try m.jsonString())
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

    private func hello(_ t: MonitorSocket) async throws {
        var h = Mason_V1_Hello()
        h.protocol = 1
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
        switch model.page {
        case .task(let id): views.append(.taskDetail); taskId = id
        case .landing: views.append(.landing)
        case .pipeline: views.append(.editor)
        case .health:
            model.raw("GET", "/resources")
        default: break
        }
        var s = Mason_V1_Subscribe()
        s.views = views
        s.project = model.project
        s.task = taskId
        s.editorStep = model.editorStep
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
        var m = Mason_V1_ClientMessage()
        m.command = c
        write(m)
    }

    private func write(_ m: Mason_V1_ClientMessage) {
        guard let t = socket, let text = try? m.jsonString() else { return }
        Task { try? await t.send(text) }
    }
}
