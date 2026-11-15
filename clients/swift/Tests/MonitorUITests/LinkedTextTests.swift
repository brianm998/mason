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
        let page = TaskDetailPage(model: model, id: 5)
        let host = NSHostingController(rootView: page.content(detail())
            .environment(\.theme, Theme(dark: false, scale: 1)))
        let size = host.sizeThatFits(in: CGSize(width: 600, height: 10_000))
        XCTAssertGreaterThan(size.height, 0)
    }

    private func underlined(_ a: AttributedString) -> [(String, Bool, Bool)] {
        a.runs.map { run in
            (String(a[run.range].characters), run.underlineStyle == .single, run.foregroundColor == .blue)
        }
    }

    func testEveryLinkRunIsUnderlinedAndInTheLinkColourAndPlainRunsAreNeither() {
        let model = MonitorModel()
        model.taskDetail = detail()
        let a = LinkedText.attributed(model.linkify("see #12, PR #7 and #99"), linkColor: .blue)
        for (text, under, blue) in underlined(a) {
            let isLink = ["#12", "PR #7"].contains(text)
            XCTAssertEqual(under, isLink, "underline on '\(text)'")
            XCTAssertEqual(blue, isLink, "link colour on '\(text)'")
        }
        XCTAssertTrue(a.runs.contains { String(a[$0.range].characters).contains("#99") && $0.link == nil },
                      "an unknown #N stays plain")
        let styled = LinkedText.link(inlineMarkdown("run **#12** now"), model.linkify("run #12 now"), linkColor: .blue)
        XCTAssertTrue(underlined(styled).contains { $0.0 == "#12" && $0.1 && $0.2 })
    }

    func testTheLinkColourIsNotTheAccent() {
        for dark in [false, true] {
            let t = Theme(dark: dark, scale: 1)
            XCTAssertNotEqual(t.link, t.accent)
        }
    }

    /// Grep-style guard: no `.buttonStyle(.link)` (accent colour, no underline) remains in MonitorUI.
    func testNoButtonUsesTheSystemLinkStyle() throws {
        let dir = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().appendingPathComponent("Sources/MonitorUI")
        let files = try FileManager.default.contentsOfDirectory(atPath: dir.path).filter { $0.hasSuffix(".swift") }
        XCTAssertFalse(files.isEmpty)
        for f in files {
            let text = try String(contentsOf: dir.appendingPathComponent(f), encoding: .utf8)
            XCTAssertFalse(text.contains("buttonStyle(.link)"), "\(f) uses buttonStyle(.link): use TaskLink or WebLink")
        }
    }

    /// Offscreen render of the shared link look: it draws (the cursor itself cannot be unit tested).
    func testALinkRendersOffscreen() {
        let view = VStack(alignment: .leading) {
            TaskLink(title: "#12") {}
            Text("#12").linkLook()
        }
        .padding().background(Color.white)
        .environment(\.theme, Theme(dark: false, scale: 1))
        let r = ImageRenderer(content: view)
        XCTAssertNotNil(r.cgImage)
    }
}
