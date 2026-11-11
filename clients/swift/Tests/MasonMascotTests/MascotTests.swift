// SPDX-License-Identifier: GPL-3.0-or-later

import XCTest

#if canImport(AppKit)
    import AppKit
#endif

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

    func testEveryBannerKindMapsToAnImageOrDeliberatelyNone() {
        for (kind, id) in BannerIcons.daemonKinds {
            XCTAssertEqual(BannerIcons.id(forDaemonKind: kind), id, kind)
            if let id { XCTAssertNotNil(Mascot.bannerIcon(id), "\(kind) -> \(id)") }
        }
        for (_, id) in BannerIcons.clientBanners { XCTAssertNotNil(Mascot.bannerIcon(id), id) }
        XCTAssertEqual(BannerIcons.id(forDaemonKind: "landing_frozen"), "landing_frozen")
        XCTAssertEqual(BannerIcons.id(forDaemonKind: "provider_budget"), "provider")
    }

    func testAnUnknownBannerKindHasNoIcon() {
        XCTAssertNil(BannerIcons.id(forDaemonKind: "nonsense"))
        XCTAssertNil(BannerIcons.id(forDaemonKind: ""))
    }

    #if canImport(AppKit)
        func testTheMacIconIsShapedWithTransparentCorners() throws {
            let d = try XCTUnwrap(Mascot.macAppIconData())
            let rep = try XCTUnwrap(NSBitmapImageRep(data: d))
            XCTAssertEqual(rep.pixelsWide, 1024)
            XCTAssertEqual(rep.pixelsHigh, 1024)
            XCTAssertTrue(rep.hasAlpha)
            func alpha(_ x: Int, _ y: Int) -> CGFloat { rep.colorAt(x: x, y: y)?.alphaComponent ?? -1 }
            for (x, y) in [(0, 0), (1023, 0), (0, 1023), (1023, 1023), (50, 512)] {
                XCTAssertEqual(alpha(x, y), 0, "(\(x), \(y)) is transparent")
            }
            XCTAssertEqual(alpha(512, 512), 1, "the centre is opaque")
        }

        func testTheIosIconStaysOpaqueAndFullBleed() throws {
            var url = URL(fileURLWithPath: #filePath)
            for _ in 0..<3 { url.deleteLastPathComponent() }  // .../clients/swift
            url.appendPathComponent("ios/MasonIOS/AppIcon-1024.png")
            let rep = try XCTUnwrap(NSBitmapImageRep(data: try Data(contentsOf: url)))
            XCTAssertEqual(rep.pixelsWide, 1024)
            XCTAssertFalse(rep.hasAlpha)
        }
    #endif
}
