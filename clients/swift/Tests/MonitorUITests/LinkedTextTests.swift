// SPDX-License-Identifier: GPL-3.0-or-later

import AppKit
import MasonProtocol
import MonitorCore
import SwiftUI
import XCTest

@testable import MonitorUI

/// s204: a task's page links the task numbers and CI references in its text. Rendered offscreen
/// (a hosting controller that is never shown); nothing is opened.
@MainActor
final class LinkedTextTests: XCTestCase {
    private func detail() -> TaskDetailView {
        var d = TaskDetailView()
        var r = TaskRow()
        r.id = 5
        r.key = "#5"
        r.title = "t"
        r.state = "waiting"
        r.whyWaiting = "waiting for the pull request's checks (#7), after #12"
        r.blockers = [12]
        d.row = r
        d.failure = "PR #7 (mason/pr/x): checks failed: lint, then #99"
        d.body = "Fix what #12 did"
        var pr = CiLink()
        pr.label = "PR #7"
        pr.url = "https://github.com/o/r/pull/7"
        pr.kind = "pull_request"
        var lint = CiLink()
        lint.label = "lint"
        lint.url = "https://github.com/o/r/actions/runs/1/job/2"
        lint.kind = "check"
        d.ciLinks = [pr, lint]
        return d
    }

    private func links(_ a: AttributedString) -> [(String, String)] {
        a.runs.compactMap { run in
            run.link.map { (String(a[run.range].characters), $0.absoluteString) }
        }
    }

    func testASampleDetailGetsATaskLinkAndCiLinks() {
        let model = MonitorModel()
        model.taskDetail = detail()
        let why = LinkedText.attributed(model.linkify(detail().row.whyWaiting), linkColor: .blue)
        XCTAssertEqual(links(why).map { $0.0 }, ["#7", "#12"])
        XCTAssertEqual(links(why).map { $0.1 }, ["https://github.com/o/r/pull/7", "mason-task://12"])
        let failure = LinkedText.attributed(model.linkify(detail().failure), linkColor: .blue)
        XCTAssertEqual(links(failure).map { $0.0 }, ["PR #7", "lint"], "#99 is unknown and stays plain")
        XCTAssertEqual(String(failure.characters), detail().failure)
    }

    func testStyledTextKeepsItsTextAndGainsLinks() {
        let model = MonitorModel()
        model.taskDetail = detail()
        let base = inlineMarkdown("run **#12** now")
        let a = LinkedText.link(base, model.linkify(String(base.characters)), linkColor: .blue)
        XCTAssertEqual(links(a).map { $0.0 }, ["#12"])
        XCTAssertEqual(String(a.characters), "run #12 now")
    }

    func testThePageRendersOffscreen() {
        let model = MonitorModel()
        model.connected = true
        model.taskDetail = detail()
        let host = NSHostingController(rootView: TaskDetailPage(model: model, id: 5)
            .environment(\.theme, Theme(dark: false, scale: 1)))
        let size = host.sizeThatFits(in: CGSize(width: 600, height: 10_000))
        XCTAssertGreaterThan(size.height, 0)
    }
}
