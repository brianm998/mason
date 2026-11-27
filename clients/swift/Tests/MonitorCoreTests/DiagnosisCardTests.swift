// SPDX-License-Identifier: GPL-3.0-or-later

import MasonProtocol
import XCTest

@testable import MonitorCore

/// #1046: the answer request from (option, context) and (Other, context), the choice a failed
/// task's page makes, and the badge on its row.
final class DiagnosisCardTests: XCTestCase {
    func testAnAnswerRequestFromAnOptionAndContext() {
        XCTAssertEqual(AnswerRequest.option(1, context: "  the flaky one \n"), AnswerRequest(answer: "2", context: "the flaky one"))
    }

    func testAnEmptyContextIsOmitted() {
        XCTAssertEqual(AnswerRequest.option(0, context: "   ").context, "")
        XCTAssertNil(contextText(" \n"))
        // The command built from it carries no context at all.
        var c = Mason_V1_Command()
        c.answer = .with { $0.task = 1; $0.answer = "1"; $0.cardHash = "h"; $0.context = AnswerRequest.option(0, context: "").context }
        XCTAssertTrue(c.answer.context.isEmpty)
    }

    func testOtherSendsFreeTextAndTheContext() {
        XCTAssertEqual(AnswerRequest.other(" use X ", context: "because Y"), AnswerRequest(answer: "use X", context: "because Y"))
        XCTAssertEqual(AnswerRequest.other("use X", context: "")?.context, "")
        XCTAssertNil(AnswerRequest.other("  ", context: "because Y"))
    }

    func testAFailedPageChoosesCardDiagnosingOrButtons() {
        let acts = ["retry", "cancel", "open_session", "diagnose"]
        XCTAssertEqual(FailedPage.choose(hasCard: true, diagnosing: false, actions: ["answer"] + acts), .card)
        XCTAssertEqual(FailedPage.choose(hasCard: false, diagnosing: true, actions: ["retry"]), .diagnosing)
        XCTAssertEqual(FailedPage.choose(hasCard: false, diagnosing: false, actions: acts), .buttons(diagnose: true))
        XCTAssertEqual(FailedPage.choose(hasCard: false, diagnosing: false, actions: ["retry"]), .buttons(diagnose: false))
    }

    func testOtherActionsIsCollapsedWithACardAndOpenSessionStaysVisible() {
        XCTAssertTrue(FailedPage.card.otherActionsCollapsed)
        XCTAssertTrue(FailedPage.diagnosing.otherActionsCollapsed)
        XCTAssertFalse(FailedPage.buttons(diagnose: true).otherActionsCollapsed)
        for a in ["retry", "cancel", "skip_all_tests"] { XCTAssertTrue(FailedPage.card.hidesUnderOther(a)) }
        XCTAssertFalse(FailedPage.card.hidesUnderOther("open_session"))
        XCTAssertFalse(FailedPage.buttons(diagnose: true).hidesUnderOther("retry"))
    }

    func testTheDiagnosingLineAndTheRowBadge() {
        XCTAssertEqual(diagnosingLine(startedMs: 0, ranOn: "Opus medium", now: 0), "Diagnosing on Opus medium… started just now")
        XCTAssertTrue(diagnosingLine(startedMs: 1_000, ranOn: "", now: 181_000).hasSuffix("ago"))
        XCTAssertEqual(diagnosisBadge("ready"), "diagnosis ready")
        XCTAssertEqual(diagnosisBadge("running"), "diagnosing")
        XCTAssertNil(diagnosisBadge(""))
        var r = Mason_V1_TaskRow()
        r.activity = "idle"
        r.diagnosis = "ready"
        XCTAssertEqual(QueueWait.line(r), "idle · diagnosis ready")
    }
}
