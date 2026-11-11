// SPDX-License-Identifier: GPL-3.0-or-later

import XCTest

@testable import MasonMascot

final class MascotTests: XCTestCase {
    func testTheArtIsFound() {
        XCTAssertNotNil(Mascot.artRoot)
    }

    func testEveryIdInTheManifestLoads() {
        for id in MascotManifest.stateIds {
            for size in [20, 24] {
                XCTAssertNotNil(Mascot.icon(id, size: size), "icon \(id) \(size)")
                for f in Mascot.iconFiles(id, size: size) { XCTAssertNotNil(Mascot.data(f), f) }
                XCTAssertEqual(Mascot.iconFiles(id, size: size).count, 3, id)
            }
        }
        for id in MascotManifest.bannerIds {
            XCTAssertNotNil(Mascot.bannerIcon(id), "banner \(id)")
        }
        for id in MascotManifest.illustrationIds {
            XCTAssertNotNil(Mascot.illustration(id), "illustration \(id)")
        }
        for f in MascotManifest.files { XCTAssertNotNil(Mascot.data(f), f) }
    }

    func testAnUnknownIdGivesNothing() {
        XCTAssertNil(Mascot.icon("nonsense", size: 20))
        XCTAssertNil(Mascot.icon("", size: 20))
        XCTAssertNil(Mascot.icon("queued", size: 17))
        XCTAssertNil(Mascot.icon("../icons/queued-20@1x", size: 20))
        XCTAssertNil(Mascot.bannerIcon("nonsense"))
        XCTAssertNil(Mascot.bannerIcon("queued"))
        XCTAssertNil(Mascot.illustration("nonsense"))
        XCTAssertNil(Mascot.illustration("../brand/app_icon"))
    }
}
