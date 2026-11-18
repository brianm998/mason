// SPDX-License-Identifier: GPL-3.0-or-later

import AppKit
import MasonProtocol
import MasonTokens
import MonitorCore
import SwiftUI
import XCTest

@testable import MonitorUI

/// s527: the queue-held banner, the cause badge and the lease rows, from a fixture of the
/// 2026-11-16 stall. Rendered offscreen (a hosting controller sized; nothing is shown).
@MainActor
final class QueueHoldBannerTests: XCTestCase {
    private func size<V: View>(_ view: V) -> CGSize {
        NSHostingController(rootView: view.environment(\.theme, Theme(dark: false, scale: 1)))
            .sizeThatFits(in: CGSize(width: 900, height: 10_000))
    }

    private func lease() -> Mason_V1_WaitParty {
        var p = Mason_V1_WaitParty()
        p.label = "starpipe heavy"
        p.lease = true
        p.origin = "starpipe in /work/star"
        p.needs = "exclusive"
        p.exclusive = true
        p.sinceMs = 1_000_000
        return p
    }

    private func hold() -> Mason_V1_QueueHold {
        var h = Mason_V1_QueueHold()
        h.kind = "exclusive-request"
        h.cause = "behind-exclusive"
        h.summary = "Queue held: an exclusive `mason run` (starpipe heavy, from starpipe in /work/star, waiting 9 min) is waiting for #510's compile slot; 31 tasks wait behind it."
        h.waiting = 31
        h.party = lease()
        return h
    }

    func testTheBannerRendersAndNamesTheLeaseBelowTheSummary() {
        let model = MonitorModel()
        let with = size(QueueHoldBanner(model: model, hold: hold()))
        XCTAssertGreaterThan(with.height, 0)
        var gate = hold()
        gate.kind = "gate"
        gate.clearParty()
        XCTAssertGreaterThan(with.height, size(QueueHoldBanner(model: model, hold: gate)).height,
                             "a lease is listed under the summary; a gate has none")
    }

    func testTheBadgeRendersForEveryCause() {
        for c in ["full", "behind-exclusive", "cpu", "syspolicyd", "disk-stall"] {
            XCTAssertGreaterThan(size(WaitBadge(cause: c)).width, 0, c)
        }
    }

    func testTheCompilationSectionListsLeasesWithNoTask() {
        let model = MonitorModel()
        var c = Mason_V1_Compilation()
        c.limit = 6
        let plain = size(CompilationSection(model: model, c: c)).height
        c.leases = [lease()]
        XCTAssertGreaterThan(size(CompilationSection(model: model, c: c)).height, plain)
    }
}
