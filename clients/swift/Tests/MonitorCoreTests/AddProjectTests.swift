// SPDX-License-Identifier: GPL-3.0-or-later

import XCTest
import MasonProtocol
@testable import MonitorCore

/// The Add Project wizard's state, driven with a stand-in for the daemon's onboarding endpoints.
@MainActor
final class AddProjectTests: XCTestCase {
    static let questions: [(id: String, options: [String], def: String)] = [
        ("tracker", ["local", "github"], "local"),
        ("agent", ["claude", "codex"], "claude"),
        ("alerts", [], ""),
    ]

    /// What the daemon would answer to a plan with these answers.
    static func planJson(_ answers: [String: String]) -> String {
        let next = questions.first { answers[$0.id] == nil }
        var o: [String: Any] = [
            "detected": ["id": "web", "branch": "main", "languages": ["rust"], "ci": ["github"],
                         "remote": ["host": "github.com", "repo": "o/web"], "agents": [["Claude", "/bin/claude"]]],
            "kind": "init", "errors": [] as [String],
            "files": next == nil ? [["path": ".mason/mason.toml", "contents": "x = 1\n"]] : [],
        ]
        if let n = next { o["next"] = ["id": n.id, "text": "Which \(n.id)?", "options": n.options, "default": n.def, "flag": "--\(n.id)"] }
        return String(data: try! JSONSerialization.data(withJSONObject: o), encoding: .utf8)!
    }

    @MainActor final class Fake {
        var sent: [Mason_V1_Command] = []
        let model = MonitorModel()
        var applyJson = "{\"id\":\"web\",\"registered\":true,\"progress\":[\"wrote .mason\"],\"proof_task\":41}"
        var applyOk = true
        var applyError = ""
        init() {
            model.sender = { [unowned self] c in self.sent.append(c); self.answer(c) }
            var w = Mason_V1_ServerMessage()
            w.welcome = .with { $0.protocol = 2; $0.owner = true }
            model.apply(w)
        }
        func answer(_ c: Mason_V1_Command) {
            guard c.id.hasPrefix("onb-") else { return }
            var r = Mason_V1_CommandResult()
            r.id = c.id
            r.ok = true
            switch c.id {
            case "onb-candidates": r.json = "{\"candidates\":[{\"path\":\"/r/web\",\"name\":\"web\",\"has_mason\":false,\"remote\":null}],\"errors\":[]}"
            case "onb-plan":
                let b = (try? JSONSerialization.jsonObject(with: Data(c.raw.json.utf8))) as? [String: Any] ?? [:]
                r.json = AddProjectTests.planJson(b["answers"] as? [String: String] ?? [:])
            default: r.ok = applyOk; r.json = applyJson; r.error = applyError
            }
            var sm = Mason_V1_ServerMessage()
            sm.result = r
            model.apply(sm)
        }
        var lastBody: [String: Any] {
            (try? JSONSerialization.jsonObject(with: Data(sent.last!.raw.json.utf8))) as? [String: Any] ?? [:]
        }
    }

    func testOnlyTheOwnerCanOpenIt() {
        let m = MonitorModel()
        var w = Mason_V1_ServerMessage()
        w.welcome = .with { $0.protocol = 2; $0.owner = false }
        m.apply(w)
        m.openAddProject()
        XCTAssertNil(m.addProject)
        XCTAssertFalse(m.canAddProject)
    }

    func testAnswersAccumulateIntoTheFlagMap() {
        let f = Fake()
        f.model.openAddProject()
        XCTAssertEqual(f.sent.last?.raw.path, "/onboarding/candidates")
        XCTAssertEqual(f.model.addProject?.candidates.map(\.path), ["/r/web"])
        f.model.addProject?.path = "/r/web"
        f.model.planAddProject()
        XCTAssertEqual(f.model.addProject?.step, .detected)
        XCTAssertEqual(f.model.addProject?.plan?.agents, ["claude"])
        XCTAssertEqual(f.model.addProject?.plan?.remote, "github.com/o/web")
        f.model.addProject?.continueFromDetected()
        XCTAssertEqual(f.model.addProject?.step, .questions)
        f.model.answerAddProject("github")
        f.model.answerAddProject("codex")
        XCTAssertEqual(f.model.addProject?.answers, ["tracker": "github", "agent": "codex"])
        XCTAssertEqual(f.model.addProject?.question?.id, "alerts")
        XCTAssertTrue(f.model.addProject?.question?.isFreeText ?? false)
        XCTAssertEqual((f.lastBody["answers"] as? [String: String])?["agent"], "codex")
    }

    func testUseRecommendedFillsEveryRemainingDefault() {
        let f = Fake()
        f.model.openAddProject()
        f.model.addProject?.path = "/r/web"
        f.model.planAddProject()
        f.model.addProject?.continueFromDetected()
        f.model.answerAddProject("github")
        f.model.addProjectUseRecommendedForRest()
        XCTAssertEqual(f.model.addProject?.answers, ["tracker": "github", "agent": "claude", "alerts": ""])
        XCTAssertEqual(f.model.addProject?.step, .review)
        XCTAssertEqual(f.model.addProject?.plan?.files.map(\.path), [".mason/mason.toml"])
    }

    func testReviewDefaultsAndApplyBody() {
        let f = Fake()
        f.model.openAddProject()
        f.model.addProject?.path = "/r/web"
        f.model.planAddProject()
        f.model.addProject?.continueFromDetected()
        f.model.addProjectUseRecommendedForRest()
        XCTAssertEqual(f.model.addProject?.land, "no")
        XCTAssertEqual(f.model.addProject?.proveTask, true)
        XCTAssertEqual(AddProjectWizard.landChoices, ["no", "merge", "push", "pr"])
        f.model.addProject?.land = "pr"
        f.model.addProject?.proveTask = false
        f.model.applyAddProject()
        let a = f.lastBody["answers"] as? [String: Any]
        XCTAssertEqual(a?["land"] as? String, "pr")
        XCTAssertEqual(a?["no_prove"] as? Bool, true)
    }

    func testApplySuccessSelectsTheProject() {
        let f = Fake()
        f.model.openAddProject()
        f.model.addProject?.path = "/r/web"
        f.model.planAddProject()
        f.model.addProject?.continueFromDetected()
        f.model.addProjectUseRecommendedForRest()
        f.model.applyAddProject()
        XCTAssertEqual(f.sent.last?.raw.path, "/onboarding/apply")
        XCTAssertNil(f.lastBody["answers"].flatMap { ($0 as? [String: Any])?["no_prove"] })
        XCTAssertEqual(f.model.project, "web")
        XCTAssertEqual(f.model.addProject?.progress, ["wrote .mason"])
        XCTAssertEqual(f.model.addProject?.proofTask, 41)
        f.model.finishAddProject(openProofTask: true)
        XCTAssertNil(f.model.addProject)
        XCTAssertEqual(f.model.page, .task(41))
    }

    func testPlanAndApplyWarningsAndNotesAreReadBack() {
        let p = OnboardingPlan(json: #"{"detected":{},"warnings":["no claude on PATH"],"files":[],"errors":[]}"#)
        XCTAssertEqual(p?.warnings, ["no claude on PATH"])
        var w = AddProjectWizard()
        w.startApply()
        w.applied(ok: true, json: #"{"id":"web","registered":true,"progress":["x"],"warnings":["the test step runs true"],"notes":["chose main"]}"#, error: "")
        XCTAssertEqual(w.warnings, ["the test step runs true"])
        XCTAssertEqual(w.notes, ["chose main"])
        XCTAssertEqual(w.appliedId, "web")
    }

    func testApplyRefusalStaysOnReview() {
        let f = Fake()
        f.applyOk = false
        f.applyError = "the working tree has uncommitted changes"
        f.model.openAddProject()
        f.model.addProject?.path = "/r/web"
        f.model.planAddProject()
        f.model.addProject?.continueFromDetected()
        f.model.addProjectUseRecommendedForRest()
        f.model.applyAddProject()
        XCTAssertEqual(f.model.addProject?.step, .review)
        XCTAssertEqual(f.model.addProject?.refusal, "the working tree has uncommitted changes")
        XCTAssertEqual(f.model.project, "")
    }

    func testBackReturnsToTheRepository() {
        var w = AddProjectWizard()
        w.path = "/r/web"
        w.received(plan: OnboardingPlan(json: Self.planJson([:]))!)
        XCTAssertEqual(w.step, .detected)
        w.back()
        XCTAssertEqual(w.step, .repository)
    }
}
