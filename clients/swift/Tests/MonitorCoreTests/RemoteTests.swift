// SPDX-License-Identifier: GPL-3.0-or-later

import MasonProtocol
import XCTest

@testable import MonitorCore

final class FakeSource: ServiceSource, @unchecked Sendable {
    var found: [ResolvedService] = []
    func services() -> [ResolvedService] { found }
}

@MainActor
final class RemoteFakeSocket: MonitorSocket {
    var opened: DaemonInfo?
    var sent: [String] = []
    var inbox: [String] = []
    var waiter: CheckedContinuation<String, Error>?
    var closed = false
    var refusal: Int?
    /// Fails the way a refused upgrade does: the first send throws.
    var failAtOnce = false
    func open(port: UInt16) { opened = DaemonInfo(port: port) }
    func open(info: DaemonInfo) { opened = info }
    func send(_ text: String) async throws {
        if closed || failAtOnce { throw URLError(.networkConnectionLost) }
        sent.append(text)
    }
    func receive() async throws -> String {
        if closed || failAtOnce { throw URLError(.networkConnectionLost) }
        if !inbox.isEmpty { return inbox.removeFirst() }
        return try await withCheckedThrowingContinuation { waiter = $0 }
    }
    func push(_ text: String) {
        if let w = waiter { waiter = nil; w.resume(returning: text) } else { inbox.append(text) }
    }
    func close() {
        closed = true
        waiter?.resume(throwing: URLError(.networkConnectionLost))
        waiter = nil
    }
}

@MainActor
final class RemoteTests: XCTestCase {
    let fp = String(repeating: "ab", count: 32)
    let other = String(repeating: "cd", count: 32)

    func daemon() -> PairedDaemon {
        PairedDaemon(host: "m.local", port: 7424, fingerprint: fp, token: "tok", role: "owner", alternates: ["m.ts.net"])
    }

    func service(fp: String, host: String, port: UInt16) -> ResolvedService {
        ResolvedService(record: ServiceRecord(instance: "MASON on m", proto: 1, fingerprint: fp, version: "1"), host: host, port: port)
    }

    func welcomeJSON() throws -> String {
        var w = Mason_V1_Welcome()
        w.version = "test"
        var m = Mason_V1_ServerMessage()
        m.welcome = w
        return try m.jsonString()
    }

    func waitUntil(_ what: String, _ f: @escaping () -> Bool) async {
        for _ in 0..<500 {
            if f() { return }
            try? await Task.sleep(nanoseconds: 10_000_000)
        }
        XCTFail("timed out waiting for \(what)")
    }

    // MARK: attempt order

    func testWithoutABonjourMatchItWalksTheReachOrder() {
        let d = RemoteDiscovery(paired: daemon(), source: FakeSource())
        XCTAssertEqual(d.candidates().map(\.host), ["m.local", "m.ts.net"])
        XCTAssertEqual(d.current(), DaemonInfo(port: 7424, host: "m.local"))
        XCTAssertEqual(d.current(), DaemonInfo(port: 7424, host: "m.local"), "the same until the attempt ends")
        d.attemptEnded(welcomed: false)
        XCTAssertEqual(d.current()?.host, "m.ts.net")
        d.attemptEnded(welcomed: false)
        XCTAssertEqual(d.current()?.host, "m.local", "it wraps round")
    }

    func testABonjourMatchComesFirstWithItsOwnPort() {
        let s = FakeSource()
        s.found = [service(fp: fp, host: "192.168.1.5", port: 9001)]
        let d = RemoteDiscovery(paired: daemon(), source: s)
        XCTAssertEqual(d.candidates().map(\.host), ["192.168.1.5", "m.local", "m.ts.net"])
        XCTAssertEqual(d.current(), DaemonInfo(port: 9001, host: "192.168.1.5"))
        d.attemptEnded(welcomed: false)
        XCTAssertEqual(d.current(), DaemonInfo(port: 7424, host: "m.local"))
    }

    func testAWelcomedAttemptKeepsItsEndpointForTheNext() {
        let d = RemoteDiscovery(paired: daemon(), source: nil)
        d.attemptEnded(welcomed: false)
        XCTAssertEqual(d.current()?.host, "m.ts.net")
        d.attemptEnded(welcomed: true)
        XCTAssertEqual(d.current()?.host, "m.local", "a welcome starts the walk over")
    }

    func testARecordWithAnotherFingerprintIsIgnored() {
        let s = FakeSource()
        s.found = [service(fp: other, host: "10.0.0.9", port: 9001)]
        let d = RemoteDiscovery(paired: daemon(), source: s)
        XCTAssertEqual(d.candidates().map(\.host), ["m.local", "m.ts.net"])
        XCTAssertFalse(d.candidates().contains { $0.host == "10.0.0.9" })
        // A match among others is still found.
        s.found.append(service(fp: fp.uppercased(), host: "10.0.0.7", port: 9002))
        XCTAssertEqual(d.candidates().first, Endpoint(host: "10.0.0.7", port: 9002, viaBonjour: true))
    }

    // MARK: the paired store

    func testDefaultsStoreRoundTripsAndHoldsNoToken() {
        let name = "remote-tests-\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: name)!
        defer { defaults.removePersistentDomain(forName: name) }
        let store = DefaultsPairedStore(defaults: defaults)
        XCTAssertNil(store.load())
        let r = PairedRecord(daemon())
        store.save(r)
        XCTAssertEqual(store.load(), r)
        XCTAssertFalse(String(describing: defaults.dictionary(forKey: DefaultsPairedStore.key) ?? [:]).contains("tok"))
        store.clear()
        XCTAssertNil(store.load())
    }

    // MARK: the session

    func session(
        model: MonitorModel, store: MemoryPairedStore, tokens: MemoryTokenStore, source: ServiceSource? = nil,
        sockets: Box<[RemoteFakeSocket]>, configure: @escaping (RemoteFakeSocket) -> Void = { _ in }
    ) -> RemoteSession {
        var t = ConnectionTiming()
        t.policy = ReconnectPolicy(scale: 0.01)
        t.watchEvery = 0.02
        t.pingEvery = 0.05
        return RemoteSession(model: model, store: store, tokens: tokens, source: source, timing: t, makeSocket: { _ in
            let s = RemoteFakeSocket()
            configure(s)
            sockets.value.append(s)
            return s
        })
    }

    final class Box<T> { var value: T; init(_ v: T) { value = v } }

    func paired() -> (MemoryPairedStore, MemoryTokenStore) {
        let tokens = MemoryTokenStore()
        tokens.setToken("tok", forFingerprint: fp)
        return (MemoryPairedStore(PairedRecord(daemon())), tokens)
    }

    func testA401ClearsThePairingAndReturnsToThePairingScreen() async throws {
        let model = MonitorModel()
        let (store, tokens) = paired()
        let sockets = Box<[RemoteFakeSocket]>([])
        let s = session(model: model, store: store, tokens: tokens, sockets: sockets) { $0.failAtOnce = true; $0.refusal = 401 }
        XCTAssertEqual(s.state, .paired)
        s.activate()
        await waitUntil("unpaired") { s.state == .unpaired }
        XCTAssertNil(store.load())
        XCTAssertNil(tokens.token(forFingerprint: fp), "the token goes too")
        XCTAssertNil(model.linkStatus)
        XCTAssertEqual(sockets.value.count, 1, "it does not retry a revoked token")
    }

    func testAnOtherFailureKeepsThePairingAndWalksTheHosts() async throws {
        let model = MonitorModel()
        let (store, tokens) = paired()
        let sockets = Box<[RemoteFakeSocket]>([])
        let s = session(model: model, store: store, tokens: tokens, sockets: sockets) { $0.failAtOnce = true }
        s.activate()
        await waitUntil("three sockets") { sockets.value.count >= 3 }
        s.deactivate()
        XCTAssertEqual(s.state, .paired)
        XCTAssertNotNil(store.load())
        let hosts = sockets.value.prefix(3).map { $0.opened?.host }
        XCTAssertEqual(hosts, ["m.local", "m.ts.net", "m.local"])
    }

    func testReturningToTheForegroundResumesFromTheLastSequenceNumber() async throws {
        let model = MonitorModel()
        let (store, tokens) = paired()
        let sockets = Box<[RemoteFakeSocket]>([])
        let src = FakeSource()
        src.found = [service(fp: fp, host: "192.168.1.5", port: 9001)]
        let s = session(model: model, store: store, tokens: tokens, source: src, sockets: sockets)
        s.activate()
        await waitUntil("the first socket") { sockets.value.count == 1 }
        XCTAssertEqual(sockets.value[0].opened, DaemonInfo(port: 9001, host: "192.168.1.5"))
        XCTAssertTrue(model.linkStatus?.hasPrefix("Looking for MASON on") ?? false, model.linkStatus ?? "nil")
        sockets.value[0].push(try welcomeJSON())
        await waitUntil("connected") { model.connected }
        XCTAssertEqual(model.linkStatus, "Reached through 192.168.1.5")
        model.lastSeq = 41
        s.deactivate()
        XCTAssertTrue(sockets.value[0].closed)
        XCTAssertFalse(model.connected)
        s.activate()
        await waitUntil("a second socket") { sockets.value.count == 2 }
        await waitUntil("hello") { !sockets.value[1].sent.isEmpty }
        XCTAssertTrue(sockets.value[1].sent[0].contains("\"lastSeq\":\"41\""), sockets.value[1].sent[0])
        XCTAssertEqual(sockets.value.count, 2, "one connection, not two loops")
        s.deactivate()
    }

    func testNothingPairedStaysUnpairedAndOpensNoSocket() async throws {
        let model = MonitorModel()
        let sockets = Box<[RemoteFakeSocket]>([])
        let s = session(model: model, store: MemoryPairedStore(), tokens: MemoryTokenStore(), sockets: sockets)
        XCTAssertEqual(s.state, .unpaired)
        s.activate()
        XCTAssertEqual(s.state, .unpaired)
        XCTAssertTrue(sockets.value.isEmpty)
    }

    func testLinkStatusWords() {
        XCTAssertEqual(LinkStatus.looking(instance: "MASON on studio"), "Looking for MASON on studio…")
        XCTAssertEqual(LinkStatus.reached(host: "studio.local"), "Reached through studio.local")
        XCTAssertEqual(Reach.urlHost("fe80::1"), "[fe80::1]")
        XCTAssertEqual(Reach.urlHost("m.local"), "m.local")
    }
}
