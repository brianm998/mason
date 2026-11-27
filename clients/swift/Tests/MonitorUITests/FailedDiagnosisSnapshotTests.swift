// SPDX-License-Identifier: GPL-3.0-or-later

import AppKit
import MasonProtocol
import MasonTokens
import MonitorCore
import SwiftUI
import XCTest

@testable import MonitorUI

/// #1046: a failed task's page with a diagnosis card, rendered offscreen (ImageRenderer) for
/// review. Set `DIAGNOSIS_SNAPSHOT_DIR` to keep the pictures. Nothing appears on screen.
@MainActor
final class FailedDiagnosisSnapshotTests: XCTestCase {
    private func detail(card: Bool, running: Bool) -> TaskDetailView {
        var d = TaskDetailView()
        d.row = .with {
            $0.id = 9; $0.key = "#9"; $0.title = "Fix the thing"; $0.project = "mason"; $0.state = "failed"
            $0.allowedActions = (card ? ["answer"] : []) + ["retry", "cancel", "open_session"] + (card || running ? [] : ["diagnose"])
            $0.diagnosis = card ? "ready" : running ? "running" : ""
        }
        d.failure = "step develop: the build failed"
        if card {
            d.card = .with {
                $0.ask = "How should #9 go on?"
                $0.why = "The test relies on the clock; it fails when the machine is busy."
                $0.options = [
                    .with { $0.label = "Retry with a note to drop the timing assertion"; $0.recommended = true },
                    .with { $0.label = "Cancel the task" },
                ]
                $0.hash = "h"; $0.origin = "agent"
            }
        }
        if running { d.diagnosisRunning = .with { $0.startedMs = 1_000; $0.ranOn = "Opus medium" } }
        return d
    }

    func testAFailedPageWithADiagnosisCardRendersOffscreen() {
        for (card, running) in [(true, false), (false, true), (false, false)] {
            let m = MonitorModel()
            m.taskDetail = detail(card: card, running: running)
            let view = TaskDetailPage(model: m, id: 9).environment(\.theme, Theme(dark: false, scale: 1))
                .frame(width: 700, height: 600).background(Color.white)
            let r = ImageRenderer(content: view)
            r.scale = 2
            guard let cg = r.cgImage else { return XCTFail("no picture for card=\(card) running=\(running)") }
            if let dir = ProcessInfo.processInfo.environment["DIAGNOSIS_SNAPSHOT_DIR"],
               let png = NSBitmapImageRep(cgImage: cg).representation(using: .png, properties: [:]) {
                try? FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
                try? png.write(to: URL(fileURLWithPath: dir).appendingPathComponent("failed-\(card ? "card" : running ? "diagnosing" : "buttons").png"))
            }
        }
    }
}
