// SPDX-License-Identifier: GPL-3.0-or-later

import XCTest
@testable import MonitorCore

final class DevicesTests: XCTestCase {
    func testLanStatus() {
        let on = LanStatus(json: #"{"enabled":true,"port":7443,"hosts":["a.local","b.ts.net"]}"#)
        XCTAssertTrue(on.enabled)
        XCTAssertEqual(on.hosts, ["a.local", "b.ts.net"])
        XCTAssertFalse(LanStatus(json: #"{"enabled":false}"#).enabled)
        XCTAssertFalse(LanStatus(json: "garbage").enabled)
    }

    func testLiveDevicesSortBySeenAndRevokedAreApart() {
        let l = DeviceList(json: """
        {"devices":[
         {"id":1,"name":"old","role":"owner","created_at":1,"last_seen":100,"revoked_at":null},
         {"id":2,"name":"new","role":"viewer","created_at":2,"last_seen":900,"revoked_at":null},
         {"id":3,"name":"never","role":"viewer","created_at":3,"last_seen":null,"revoked_at":null},
         {"id":4,"name":"gone1","role":"owner","created_at":4,"last_seen":950,"revoked_at":1000},
         {"id":5,"name":"gone2","role":"owner","created_at":5,"last_seen":null,"revoked_at":2000}]}
        """)
        XCTAssertEqual(l.active.map(\.id), [2, 1, 3])
        XCTAssertEqual(l.revoked.map(\.id), [5, 4])
        XCTAssertTrue(l.revoked.allSatisfy(\.isRevoked))
        XCTAssertEqual(DeviceList(json: "x"), DeviceList())
    }

    func testLastSeenText() {
        XCTAssertEqual(DeviceList.lastSeenText(nil, nowMs: 5), "never")
        XCTAssertEqual(DeviceList.lastSeenText(1_000, nowMs: 30_000), "just now")
        XCTAssertEqual(DeviceList.lastSeenText(0, nowMs: 300_000), "5 min ago")
        XCTAssertEqual(DeviceList.lastSeenText(0, nowMs: 3 * 3_600_000), "3 h ago")
        XCTAssertEqual(DeviceList.lastSeenText(0, nowMs: 2 * 86_400_000), "2 d ago")
    }

    func testCountdownRunsOnAnInjectedClock() {
        let json = #"{"code":"12345678","role":"viewer","expires_in_s":300,"hosts":["h.local"],"url":"mason://pair?x=1"}"#
        let o = PairOffer(json: json, nowMs: 10_000)!
        XCTAssertEqual(o.role, "viewer")
        XCTAssertEqual(o.state(nowMs: 10_000), .valid(secondsLeft: 300))
        XCTAssertEqual(o.state(nowMs: 10_001), .valid(secondsLeft: 300))
        XCTAssertEqual(o.state(nowMs: 10_000 + 299_001), .valid(secondsLeft: 1))
        XCTAssertEqual(o.state(nowMs: 10_000 + 300_000), .expired)
        XCTAssertEqual(o.state(nowMs: 0), .valid(secondsLeft: 310))
        XCTAssertEqual(PairOffer.clock(299), "4:59")
        XCTAssertNil(PairOffer(json: #"{"code":"12","url":"u"}"#, nowMs: 0))
    }

    #if canImport(CoreImage)
    func testQRImageDecodesBackToTheSameURL() throws {
        let url = "mason://pair?host=mac.local&port=7443&fp=" + String(repeating: "ab", count: 32) + "&code=12345678&alt=x.ts.net"
        let img = try XCTUnwrap(QRCode.image(for: url, side: 480))
        XCTAssertEqual(QRCode.decode(img), url)
        XCTAssertEqual(PairURL.parse(QRCode.decode(img) ?? "")?.code, "12345678")
    }
    #endif
}
