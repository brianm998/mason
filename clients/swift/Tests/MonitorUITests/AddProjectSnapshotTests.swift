// SPDX-License-Identifier: GPL-3.0-or-later

import AppKit
import MonitorCore
import SwiftUI
import XCTest

@testable import MonitorUI

/// Each step of the Add Project wizard drawn offscreen (ImageRenderer; nothing is put on screen),
/// and the controls that belong to the desktop only. No golden files: the steps are compared with
/// each other.
@MainActor
final class AddProjectSnapshotTests: XCTestCase {
    private func wizard(at step: AddProjectWizard.Step) -> AddProjectWizard {
        var w = AddProjectWizard()
        w.candidates = [OnboardingCandidate(path: "/r/web", name: "web", hasMason: false, remote: "git@github.com:o/web.git")]
        w.path = "/r/web"
        w.received(plan: OnboardingPlan(json: Self.planJson([:]))!)
        if step == .detected { return w }
        if step == .repository { w.back(); return w }
        w.continueFromDetected()
        if step == .questions { return w }
        w.received(plan: OnboardingPlan(json: Self.planJson(["tracker": "a", "agent": "b", "alerts": ""]))!)
        w.step = step == .apply ? .apply : .review
        if step == .apply { w.progress = ["wrote .mason/mason.toml"]; w.appliedId = "web"; w.proofTask = 41 }
        return w
    }

    static func planJson(_ answers: [String: String]) -> String {
        let qs = ["tracker", "agent", "alerts"]
        var o: [String: Any] = [
            "detected": ["id": "web", "branch": "main", "languages": ["rust"], "ci": ["github"],
                         "remote": ["host": "github.com", "repo": "o/web"], "agents": [["Claude", "/bin/claude"]]],
            "kind": "init", "errors": [] as [String],
            "files": [["path": ".mason/mason.toml", "contents": "x = 1\n"]],
        ]
        if let q = qs.first(where: { answers[$0] == nil }) {
            o["next"] = ["id": q, "text": "Which \(q)?", "options": q == "alerts" ? [] : ["a", "b"], "default": "a", "flag": "--\(q)"]
        }
        return String(data: try! JSONSerialization.data(withJSONObject: o), encoding: .utf8)!
    }

    private func render(_ w: AddProjectWizard) -> [UInt8]? {
        let model = MonitorModel()
        model.addProject = w
        let r = ImageRenderer(content: AddProjectContent(model: model, wizard: w).environment(\.theme, Theme(dark: false, scale: 1)))
        r.scale = 1
        guard let cg = r.cgImage, cg.width > 100, cg.height > 100 else { return nil }
        return NSBitmapImageRep(cgImage: cg).representation(using: .png, properties: [:]).map { [UInt8]($0) }
    }

    func testEveryStepRendersAndDiffers() {
        var seen: [[UInt8]] = []
        for s in AddProjectWizard.Step.allCases {
            guard let img = render(wizard(at: s)) else { return XCTFail("\(s) did not render") }
            XCTAssertFalse(seen.contains(img), "\(s) looks like an earlier step")
            seen.append(img)
        }
    }

    func testRefusalIsDrawn() {
        var w = wizard(at: .review)
        let plain = render(w)
        w.refusal = "the working tree has uncommitted changes"
        XCTAssertNotEqual(plain, render(w))
    }
}
