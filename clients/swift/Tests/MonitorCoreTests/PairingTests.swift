// SPDX-License-Identifier: GPL-3.0-or-later

import XCTest

@testable import MonitorCore

final class PairingTests: XCTestCase {
    let fp = String(repeating: "ab", count: 32)

    func testPairURLParses() {
        let u = PairURL.parse("mason://pair?host=studio.local&port=7424&fp=\(fp)&code=12345678")
        XCTAssertEqual(u, PairURL(host: "studio.local", port: 7424, fingerprint: fp, code: "12345678"))
    }

    func testBadPairURLsAreRefused() {
        for bad in [
            "https://pair?host=h&port=1&fp=\(fp)&code=12345678",
            "mason://home",
            "mason://pair?host=h&port=1&fp=short&code=12345678",
            "mason://pair?host=h&port=1&fp=\(fp)&code=1234",
            "mason://pair?host=h&port=0&fp=\(fp)&code=12345678",
            "mason://pair?port=1&fp=\(fp)&code=12345678",
            "mason://pair?host=h&port=99999&fp=\(fp)&code=12345678",
        ] { XCTAssertNil(PairURL.parse(bad), bad) }
    }

    func testThePinMatchesOnlyTheSameFingerprint() {
        XCTAssertTrue(Pin.matches(shown: fp, pinned: fp.uppercased()))
        XCTAssertFalse(Pin.matches(shown: fp, pinned: String(repeating: "cd", count: 32)))
        XCTAssertFalse(Pin.matches(shown: fp, pinned: ""))
    }

    func testFingerprintIsSha256OfTheDer() {
        XCTAssertEqual(Pin.fingerprint(der: Data("abc".utf8)),
                       "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
    }

    func testTokensAreKeyedByFingerprint() {
        let s = MemoryTokenStore()
        s.setToken("t1", forFingerprint: fp)
        XCTAssertEqual(s.token(forFingerprint: fp.uppercased()), "t1")
        XCTAssertNil(s.token(forFingerprint: String(repeating: "cd", count: 32)), "another certificate means pair again")
        XCTAssertEqual(TokenKey.account(fingerprint: "AB"), "token:ab")
        s.setToken(nil, forFingerprint: fp)
        XCTAssertNil(s.token(forFingerprint: fp))
    }

    func testServiceRecords() {
        let r = ServiceRecord.parse(instance: "MASON on studio", txt: ["fp": fp, "proto": "1", "version": "0.0.0"])
        XCTAssertEqual(r?.fingerprint, fp)
        XCTAssertEqual(r?.proto, 1)
        XCTAssertNil(ServiceRecord.parse(instance: "x", txt: ["proto": "1"]))
        XCTAssertNil(ServiceRecord.parse(instance: "x", txt: ["fp": fp]))
    }

    func testPhoneLayoutOrder() {
        XCTAssertEqual(PhoneLayout.firstScreen, .sidebar)
        XCTAssertEqual(PhoneLayout.sidebarOrder.first, .tiles)
        XCTAssertEqual(PhoneLayout.sidebarOrder.firstIndex(of: .tiles)!, PhoneLayout.sidebarOrder.firstIndex(of: .home)! - 1,
                       "the tiles sit directly above the Home row")
        XCTAssertLessThan(PhoneLayout.sidebarOrder.firstIndex(of: .home)!, PhoneLayout.sidebarOrder.firstIndex(of: .sections)!)
    }

    func testSelectingPushesAndBackPops() {
        var n = PhoneNav()
        XCTAssertEqual(n.screen, .sidebar)
        n.select(.task(4))
        XCTAssertEqual(n.screen, .detail(.task(4)))
        n.select(.task(4))
        n.back()
        XCTAssertEqual(n.screen, .sidebar)
        n.back()
        XCTAssertEqual(n.screen, .sidebar)
        n.select(.home)
        XCTAssertEqual(n.screen, .detail(.home))
    }
}
