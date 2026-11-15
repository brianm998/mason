// SPDX-License-Identifier: GPL-3.0-or-later

import XCTest

@testable import MonitorCore

@MainActor
final class PairingScreenTests: XCTestCase {
    let fp = String(repeating: "ab", count: 32)
    var link: String { "mason://pair?host=studio.local&port=7424&fp=\(fp)&code=12345678" }
    var service: ResolvedService {
        ResolvedService(record: ServiceRecord(instance: "MASON on studio", proto: 1, fingerprint: fp, version: "1"), host: "192.168.1.5", port: 7424)
    }

    func testScanFillsAddressAndCode() {
        var s = PairingScreenState()
        XCTAssertTrue(s.scanned(link))
        XCTAssertEqual(s.code, "12345678")
        XCTAssertEqual(s.request?.host, "studio.local")
        XCTAssertTrue(s.canSubmit)
    }

    func testScanOfNonsenseIsAnError() {
        var s = PairingScreenState()
        XCTAssertFalse(s.scanned("https://example.com"))
        XCTAssertNotNil(s.error)
        XCTAssertNil(s.request)
    }

    func testNearbyNeedsAValidCode() {
        var s = PairingScreenState()
        s.choose(service)
        XCTAssertNil(s.request)
        s.setCode("12ab34 56-78x9")
        XCTAssertEqual(s.code, "12345678")
        XCTAssertEqual(s.request, PairURL(host: "192.168.1.5", port: 7424, fingerprint: fp, code: "12345678"))
        s.setCode("1234")
        XCTAssertNil(s.request)
        XCTAssertNil(s.begin())
        XCTAssertEqual(s.error, "The code is 8 digits.")
    }

    func testBeginWithoutTarget() {
        var s = PairingScreenState()
        s.setCode("12345678")
        XCTAssertNil(s.begin())
        XCTAssertNotNil(s.error)
    }

    func testShortFingerprintIsEightCharacters() {
        XCTAssertEqual(PairingScreenState.shortFingerprint(fp), "abababab")
    }

    func testErrorsAreMappedFromTheDaemonsMessage() {
        XCTAssertTrue(PairingMessage.text(status: 400, daemon: "wrong code").contains("wrong"))
        XCTAssertTrue(PairingMessage.text(status: 410, daemon: "code expired").contains("expired"))
        XCTAssertTrue(PairingMessage.text(status: 429, daemon: "too many tries").contains("locked"))
        XCTAssertTrue(PairingMessage.text(status: 403, daemon: "lan off").contains("refused"))
        XCTAssertTrue(PairingMessage.text(status: 500, daemon: "boom").contains("boom"))
        XCTAssertTrue(PairingMessage.text(PairingError.refused(410, "x")).contains("expired"))
    }

    func testFailureThenRetry() {
        var s = PairingScreenState()
        s.scanned(link)
        XCTAssertNotNil(s.begin())
        XCTAssertTrue(s.busy)
        XCTAssertNil(s.begin())
        s.failed(PairingError.refused(400, "wrong code"))
        XCTAssertFalse(s.busy)
        XCTAssertNotNil(s.error)
        XCTAssertTrue(s.canSubmit)
    }

    #if canImport(CryptoKit)
    func testPairSavesAndForgetReturnsToPairing() async {
        let store = MemoryPairedStore()
        let tokens = MemoryTokenStore()
        let model = MonitorModel(preferences: MemoryPreferences())
        let session = RemoteSession(model: model, store: store, tokens: tokens, source: nil)
        XCTAssertEqual(session.state, .unpaired)
        let p = PairingModel(session: session, store: store, tokens: tokens, deviceName: "Test Phone") { u, _, t in
            t.setToken("tok", forFingerprint: u.fingerprint)
            return PairedDaemon(host: u.host, port: u.port, fingerprint: u.fingerprint, token: "tok", role: "owner")
        }
        p.open(url: URL(string: link)!)
        await p.submit()
        XCTAssertEqual(store.load()?.role, "owner")
        XCTAssertEqual(session.state, .paired)
        p.forget()
        XCTAssertNil(store.load())
        XCTAssertNil(tokens.token(forFingerprint: fp))
        XCTAssertEqual(session.state, .unpaired)
    }

    func testRefusedPairingKeepsNothing() async {
        let store = MemoryPairedStore()
        let tokens = MemoryTokenStore()
        let session = RemoteSession(model: MonitorModel(preferences: MemoryPreferences()), store: store, tokens: tokens, source: nil)
        let p = PairingModel(session: session, store: store, tokens: tokens, deviceName: "x") { _, _, _ in
            throw PairingError.refused(429, "locked")
        }
        p.open(url: URL(string: link)!)
        await p.submit()
        XCTAssertNil(store.load())
        XCTAssertTrue(p.state.error?.contains("locked") ?? false)
    }
    #endif
}
