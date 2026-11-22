// SPDX-License-Identifier: GPL-3.0-or-later

import AppKit
import MonitorCore
import SwiftUI
import XCTest

@testable import MonitorUI

/// #891: the "Where runs go" section in the three source states. Rendered offscreen
/// (ImageRenderer); nothing appears on screen, and no XCUITest runs.
@MainActor
final class RemotePlacementTests: XCTestCase {
    private let body = ##"{"mode":"favor-remote","default":"local","problem":"unknown mode `x`, treated as local","changed_by":"owner","changed_at":"2026-11-21T10:00:00Z","projects":[{"project":"web","remote":true,"runs_on":"macos-latest","mode":"remote","source":"project"},{"project":"api","remote":true,"runs_on":"ubuntu-latest","mode":"favor-remote","source":"global"},{"project":"local-only","remote":false,"runs_on":"","mode":"local","source":"no [remote_runs]"}]}"##

    private func render(_ project: String) -> CGImage? {
        let view = RemotePlacementBody(remote: RemoteSettings(json: body), project: project, pickGlobal: { _ in }, pickProject: { _ in })
            .environment(\.theme, Theme(dark: false, scale: 1)).frame(width: 600, height: 300)
        let r = ImageRenderer(content: view)
        r.scale = 1
        return r.cgImage
    }

    func testTheSectionRendersForAProjectSetHereFollowingGlobalAndWithNoRemoteRuns() {
        for p in ["web", "api", "local-only"] { XCTAssertNotNil(render(p), p) }
    }

    func testThePickerIsDisabledOnlyWhenTheProjectFileHasNoRemoteRuns() {
        let r = RemoteSettings(json: body)
        XCTAssertEqual(["web", "api", "local-only"].map { r.project($0)!.canChange }, [true, true, false])
    }
}
