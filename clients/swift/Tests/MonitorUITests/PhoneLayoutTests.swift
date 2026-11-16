// SPDX-License-Identifier: GPL-3.0-or-later

import AppKit
import MasonProtocol
import MonitorCore
import SwiftUI
import XCTest

@testable import MonitorUI

/// s459: the phone's portrait layout at 393x852 and 375x667 points. Rows wrap and fit the width
/// (the `SidebarFitTests` approach), the screens render offscreen at both sizes, and the session
/// stepper sends the Mac's command. No window is shown and nothing is timed. Set
/// `MASON_PHONE_SHOTS=<dir>` to also write the pictures (the stage note's screenshots).
@MainActor
final class PhoneLayoutTests: XCTestCase {
    private let widths: [CGFloat] = [393, 375]

    private func needed<V: View>(_ view: V, width: CGFloat) -> CGSize {
        let host = NSHostingController(rootView: view.environment(\.theme, Theme(dark: false, scale: 1)))
        return host.sizeThatFits(in: CGSize(width: width, height: 10_000))
    }

    private func longRow(_ title: String) -> TaskRow {
        var r = TaskRow()
        r.id = 1234
        r.key = "#1234"
        r.title = title
        r.state = "developing"
        r.activity = "running the slow integration suite against every fixture in the repository"
        r.whyWaiting = "waiting for a compile slot, behind two other tasks"
        return r
    }

    private func model() -> MonitorModel {
        let model = MonitorModel()
        model.connected = true
        var h = HeaderView()
        h.attention = 2
        h.selfProject = "mason"
        h.projects = ["mason", "a-project-with-a-long-name"]
        h.counts = [("needs_attention", true, 2), ("queued", false, 12), ("developing", false, 1), ("landing", false, 3), ("landed", false, 120)]
            .map { (s: String, w: Bool, n: UInt32) -> StateCount in
                var c = StateCount()
                c.state = s
                c.label = s.replacingOccurrences(of: "_", with: " ")
                c.wantsYou = w
                c.count = n
                return c
            }
        model.header = h
        var sidebar = SidebarView()
        sidebar.sections = [("needs_attention", "Needs you"), ("developing", "Developing"), ("queued", "Queued")].map { (s: String, t: String) -> SidebarSection in
            var sec = SidebarSection()
            sec.state = s
            sec.title = t
            var r = longRow("Make the phone list fit every row at narrow widths without cutting the title short")
            r.state = s
            sec.rows = [r]
            sec.count = 1
            return sec
        }
        model.sidebar = sidebar
        var d = TaskDetailView()
        var row = longRow("Make the phone list fit every row")
        row.state = "needs_attention"
        row.allowedActions = ["cancel", "land_now", "skip_test", "retry", "defer", "explore_anyway"]
        d.row = row
        var card = Mason_V1_DecisionCard()
        card.ask = "Which way should the phone's task page lay out its decision, given a very narrow screen?"
        card.hash = "abc"
        card.options = ["One column, options stacked", "Two columns"].map { (l: String) -> Mason_V1_CardOption in
            var o = Mason_V1_CardOption()
            o.label = l
            return o
        }
        d.card = card
        model.taskDetail = d
        return model
    }

    func testARowWrapsAndFitsAtBothWidths() {
        for width in widths {
            let short = needed(PhoneTaskRow(row: longRow("Short")), width: width)
            let long = needed(PhoneTaskRow(row: longRow("Make the phone list fit every row at narrow widths without cutting the title short")), width: width)
            XCTAssertLessThanOrEqual(long.width, width + 0.5, "row needs \(long.width) of \(width)")
            XCTAssertGreaterThan(long.height, short.height, "a long title wraps to more lines at \(width), never truncates")
        }
    }

    func testTaskActionsStackWhenTheyDoNotFitOneRow() {
        let m = model()
        let row = m.taskDetail!.row
        for width in widths {
            let size = needed(ActionButtons(model: m, row: row), width: width)
            XCTAssertLessThanOrEqual(size.width, width + 0.5, "actions need \(size.width) of \(width)")
        }
    }

    func testTheSessionStepperFitsAndSendsTheMacsCommand() {
        let m = model()
        var home = HomeView()
        home.sessionsUsed = 2
        home.sessionsLimit = 4
        m.home = home
        for width in widths {
            XCTAssertLessThanOrEqual(needed(PhoneSessionStepper(model: m), width: width).width, width + 0.5)
        }
        var sent: [Mason_V1_Command] = []
        m.sender = { sent.append($0) }
        // The phone's buttons call this; the Mac's call the same function.
        m.stepSessionLimit(by: 1)
        if case .setLimit(let l)? = sent.first?.body { XCTAssertEqual(l.units, 5) } else { XCTFail() }
    }

    func testTheSidebarAndATaskRenderAtBothSizes() throws {
        let m = model()
        m.go(.task(1234))
        for size in PhoneSnapshot.sizes {
            for pushed: [Page] in [[], [.task(1234)]] {
                let rep = try XCTUnwrap(PhoneSnapshot.bitmap(m, pushed: pushed, size: size, dark: false))
                XCTAssertGreaterThanOrEqual(rep.pixelsWide, Int(size.width))
                XCTAssertGreaterThan(rep.pixelsHigh, 0)
            }
        }
        if let dir = ProcessInfo.processInfo.environment["MASON_PHONE_SHOTS"] {
            let url = URL(fileURLWithPath: dir)
            _ = try PhoneSnapshot.write(m, name: "sidebar", pushed: [], to: url)
            _ = try PhoneSnapshot.write(m, name: "task", pushed: [.task(1234)], to: url)
        }
    }
}
