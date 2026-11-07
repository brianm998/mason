// SPDX-License-Identifier: GPL-3.0-or-later

import Foundation
import MasonProtocol
import SwiftProtobuf

/// The monitor's one WebSocket to `masond`. It says hello with the last
/// sequence number it saw, so a reconnect costs only what was missed, and it
/// reconnects by itself with a growing pause. The protocol is the JSON
/// encoding (the proto3 mapping), spoken with the generated types.
@MainActor
public final class MonitorConnection {
    private let model: MonitorModel
    private let url: URL
    private var task: URLSessionWebSocketTask?
    private var session = URLSession(configuration: .ephemeral)
    private var running = false
    private var nextId = 1

    public init(model: MonitorModel, port: UInt16) {
        self.model = model
        self.url = URL(string: "ws://127.0.0.1:\(port)/ws")!
        model.sender = { [weak self] c in self?.send(command: c) }
        model.subscribe = { [weak self] in self?.sendSubscription() }
    }

    public func start() {
        guard !running else { return }
        running = true
        Task { await self.loop() }
    }

    public func stop() {
        running = false
        task?.cancel(with: .goingAway, reason: nil)
    }

    private func loop() async {
        var pause = 0.25
        while running {
            let t = session.webSocketTask(with: url)
            task = t
            t.resume()
            do {
                try await hello(t)
                pause = 0.25
                sendSubscription()
                while running {
                    let m = try await t.receive()
                    let text: String
                    switch m {
                    case .string(let s): text = s
                    case .data(let d): text = String(decoding: d, as: UTF8.self)
                    @unknown default: continue
                    }
                    var opts = JSONDecodingOptions()
                    opts.ignoreUnknownFields = true
                    if let msg = try? Mason_V1_ServerMessage(jsonString: text, options: opts) {
                        model.apply(msg)
                    }
                }
            } catch {
                model.disconnected()
            }
            guard running else { break }
            try? await Task.sleep(nanoseconds: UInt64(pause * 1_000_000_000))
            pause = min(pause * 2, 5)
        }
    }

    private func hello(_ t: URLSessionWebSocketTask) async throws {
        var h = Mason_V1_Hello()
        h.protocol = 1
        h.lastSeq = model.lastSeq
        h.client = "mason-monitor-swiftui"
        h.clientVersion = model.ownVersion
        h.actor = model.identity
        var m = Mason_V1_ClientMessage()
        m.hello = h
        try await t.send(.string(try m.jsonString()))
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
        guard let t = task, let text = try? m.jsonString() else { return }
        t.send(.string(text)) { _ in }
    }
}
