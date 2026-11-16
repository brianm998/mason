// SPDX-License-Identifier: GPL-3.0-or-later

import XCTest

@testable import MonitorCore

final class RecordingTransport: PushTransport, @unchecked Sendable {
    private let lock = NSLock()
    private var log: [PushRequest] = []
    var requests: [PushRequest] { lock.lock(); defer { lock.unlock() }; return log }
    func send(_ r: PushRequest) async throws { lock.lock(); log.append(r); lock.unlock() }
}

@MainActor
final class PushTests: XCTestCase {
    let fp = String(repeating: "ab", count: 32)

    final class Counter { var n = 0 }

    func make(_ env: PushEnv = .sandbox) -> (PushRegistrar, RecordingTransport, MemoryPushPrefs, Counter) {
        let t = RecordingTransport()
        let prefs = MemoryPushPrefs()
        let asked = Counter()
        let fp = self.fp
        let r = PushRegistrar(prefs: prefs, env: env, transport: { t }, fingerprint: { fp }, requestRegistration: { asked.n += 1 })
        return (r, t, prefs, asked)
    }

    func testTokenIsHexAndSentWithTheEnv() async {
        let (r, t, _, _) = make(.sandbox)
        await r.didRegister(deviceToken: Data([0x00, 0xab, 0x0f, 0xff]))
        XCTAssertEqual(t.requests, [PushRequest(method: "POST", path: "/devices/self/push", body: ["token": "00ab0fff", "env": "sandbox"])])
        let (r2, t2, _, _) = make(.production)
        await r2.didRegister(deviceToken: Data([1, 2]))
        XCTAssertEqual(t2.requests.first?.body, ["token": "0102", "env": "production"])
    }

    func testSameTokenIsNotResentButAChangedOneIs() async {
        let (r, t, _, _) = make()
        await r.didRegister(deviceToken: Data([1]))
        await r.didRegister(deviceToken: Data([1]))
        XCTAssertEqual(t.requests.count, 1)
        await r.didRegister(deviceToken: Data([2]))
        XCTAssertEqual(t.requests.count, 2)
        XCTAssertEqual(t.requests.last?.body?["token"], "02")
    }

    func testOffDeletesAndOnSendsAgain() async {
        let (r, t, prefs, _) = make()
        await r.didRegister(deviceToken: Data([7]))
        await r.setEnabled(false)
        XCTAssertEqual(t.requests.last, PushRequest(method: "DELETE", path: "/devices/self/push", body: nil))
        XCTAssertFalse(prefs.enabled)
        await r.didRegister(deviceToken: Data([7]))
        XCTAssertEqual(t.requests.count, 2, "off: nothing is sent")
        await r.setEnabled(true)
        XCTAssertEqual(t.requests.count, 3)
        XCTAssertEqual(t.requests.last?.method, "POST")
    }

    func testExplanationShowsOnceThenRegisters() {
        let (r, _, prefs, asked) = make()
        r.launched()
        XCTAssertTrue(r.needsExplanation)
        XCTAssertEqual(asked.n, 0)
        r.explanationAnswered(allow: true)
        XCTAssertFalse(r.needsExplanation)
        XCTAssertEqual(asked.n, 1)
        r.launched()
        XCTAssertFalse(r.needsExplanation)
        XCTAssertEqual(asked.n, 2)
        XCTAssertTrue(prefs.explained)
    }

    func testDecliningTurnsItOff() {
        let (r, _, prefs, asked) = make()
        r.launched()
        r.explanationAnswered(allow: false)
        XCTAssertFalse(prefs.enabled)
        r.launched()
        XCTAssertEqual(asked.n, 0)
    }

    func testTestButtonCallsTheDaemon() async {
        let (r, t, _, _) = make()
        await r.sendTest()
        XCTAssertEqual(t.requests, [PushRequest(method: "POST", path: "/notify/test", body: ["channel": "apns"])])
        XCTAssertNotNil(r.testResult)
    }

    #if canImport(CryptoKit)
    func testForgetDeletesBeforeClearing() async {
        let fp = self.fp
        let store = MemoryPairedStore(PairedRecord(host: "h.local", port: 7424, fingerprint: fp, role: "owner"))
        let tokens = MemoryTokenStore()
        tokens.setToken("tok", forFingerprint: fp)
        let t = RecordingTransport()
        final class Seen { var pairingStillThere: [Bool] = [] }
        let seen = Seen()
        let session = RemoteSession(model: MonitorModel(preferences: MemoryPreferences()), store: store, tokens: tokens, source: nil)
        let r = PushRegistrar(prefs: MemoryPushPrefs(), env: .sandbox, transport: {
            seen.pairingStillThere.append(tokens.token(forFingerprint: fp) != nil && store.load() != nil)
            return t
        }, fingerprint: { fp }, requestRegistration: {})
        session.push = r
        session.unpair()
        for _ in 0..<5000 where session.state != .unpaired { await Task.yield() }
        XCTAssertEqual(session.state, .unpaired)
        XCTAssertEqual(t.requests.map(\.method), ["DELETE"])
        XCTAssertEqual(seen.pairingStillThere, [true], "the pairing was still there when DELETE was built")
        XCTAssertNil(store.load())
        XCTAssertNil(tokens.token(forFingerprint: fp))
    }
    #endif

    func testPayloadMapsToPages() {
        let task = PushPayload(userInfo: ["mason": ["kind": "needs_you", "key": "k", "task": 204]])
        XCTAssertEqual(task?.page, .task(204))
        XCTAssertEqual(task?.kind, "needs_you")
        let none = PushPayload(userInfo: ["mason": ["kind": "daemon_down", "key": "k"]])
        XCTAssertEqual(none?.page, .home)
        let unknown = PushPayload(userInfo: ["mason": ["kind": "from_the_future"]])
        XCTAssertEqual(unknown?.page, .home)
        XCTAssertNil(PushPayload(userInfo: ["aps": [:]]))
    }

    func testForegroundBannerOnlyWhenNotAlreadyShown() {
        let p = PushPayload(kind: "needs_you", task: 5)
        XCTAssertFalse(p.showsBanner(whileShowing: .task(5)))
        XCTAssertTrue(p.showsBanner(whileShowing: .task(6)))
        XCTAssertTrue(p.showsBanner(whileShowing: .home))
        XCTAssertFalse(PushPayload(kind: "daemon_down").showsBanner(whileShowing: .home))
    }
}
