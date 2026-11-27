// SPDX-License-Identifier: GPL-3.0-or-later

import AppKit
import MonitorCore
import SwiftUI
import XCTest

@testable import MonitorUI

/// s1093: the "Pipeline review" section of Settings: what `GET /v1/review` says, the bodies the
/// toggle, the time and the frequency send, and the section rendered offscreen (ImageRenderer);
/// nothing appears on screen, and no XCUITest runs.
@MainActor
final class ReviewSettingsTests: XCTestCase {
    private let body = ##"{"settings":{"enabled":true,"at":"03:30","every_days":7,"model":"opus","effort":"medium","max_proposals":10,"changed_by":"owner","changed_at":1},"problem":null,"project":"mason","includes":["mason","web"],"running":false,"running_task":null,"next_run":1,"next_run_text":"2026-12-01 03:30","last":{"task":41,"completed_text":"03:41","filed":[42,43],"summary":"task #41: 2 filed (#42, #43)"},"last_status":"task #41: 2 filed (#42, #43)"}"##

    func testTheReviewIsRead() {
        let r = ReviewSettings(json: body)
        XCTAssertTrue(r.enabled)
        XCTAssertEqual(r.at, "03:30")
        XCTAssertEqual(r.everyDays, 7)
        XCTAssertEqual(r.model, "opus")
        XCTAssertEqual(r.effort, "medium")
        XCTAssertEqual(r.lastTask, 41)
        XCTAssertEqual(r.filed, [42, 43])
        XCTAssertEqual(r.filedText, "Proposals filed: #42, #43")
        XCTAssertEqual(r.nextText, "Next run: 2026-12-01 03:30")
        XCTAssertNil(r.runningTask)
        XCTAssertEqual(r.everyText, "Every 7 days")
    }

    func testAReviewThatNeverRanOrIsRunningOrIsOffSaysSo() {
        let never = ReviewSettings(json: ##"{"settings":{"enabled":false},"project":"mason","last":null}"##)
        XCTAssertNil(never.lastTask)
        XCTAssertEqual(never.nextText, "Next run: none (off)")
        XCTAssertEqual(never.filedText, "No proposals filed")
        XCTAssertEqual(never.everyText, "Every day")
        XCTAssertEqual(ReviewSettings(json: ##"{"project":"mason","running_task":9}"##).nextText, "Running now: #9")
        XCTAssertTrue(ReviewSettings(json: "{}").nextText.contains("No project of MASON's own"))
    }

    func testTheToggleTheTimeAndTheFrequencySendOneKeyEach() {
        XCTAssertEqual(SettingsPayload.reviewEnabled(false), #"{"enabled":false}"#)
        XCTAssertEqual(SettingsPayload.reviewAt("03:30"), #"{"at":"03:30"}"#)
        XCTAssertEqual(SettingsPayload.reviewEvery(days: 1), #"{"every_days":1}"#)
        XCTAssertEqual(ReviewSettings.presets.map { SettingsPayload.reviewEvery(days: $0.days) },
                       [#"{"every_days":1}"#, #"{"every_days":7}"#])
    }

    func testTheTimePickerKeepsTheTimeOfDay() {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = TimeZone(identifier: "America/Los_Angeles")!
        for at in ["00:00", "03:30", "23:59"] {
            XCTAssertEqual(ReviewSettings.timeText(ReviewSettings.date(at: at, calendar: calendar), calendar: calendar), at)
        }
        XCTAssertNil(ReviewSettings.minutes("24:00"))
        XCTAssertNil(ReviewSettings.minutes("9"))
    }

    func testTheSectionRendersOffscreen() {
        for json in [body, "{}"] {
            let view = ReviewSectionBody(model: MonitorModel(), review: ReviewSettings(json: json), change: { _ in }, runNow: {})
                .environment(\.theme, Theme(dark: false, scale: 1)).frame(width: 600, height: 420)
            let r = ImageRenderer(content: view)
            r.scale = 1
            XCTAssertNotNil(r.cgImage)
        }
    }
}
