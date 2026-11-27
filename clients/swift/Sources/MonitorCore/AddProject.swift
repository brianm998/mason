// SPDX-License-Identifier: GPL-3.0-or-later

import Foundation

/// A git repository the daemon offers to add (`GET /onboarding/candidates`).
public struct OnboardingCandidate: Equatable, Sendable {
    public var path: String
    public var name: String
    public var hasMason: Bool
    public var remote: String?

    public init(path: String, name: String, hasMason: Bool, remote: String? = nil) {
        self.path = path; self.name = name; self.hasMason = hasMason; self.remote = remote
    }
}

/// One question from `POST /onboarding/plan` `next`.
public struct OnboardingQuestion: Equatable, Sendable {
    public var id: String
    public var text: String
    /// Empty: free text.
    public var options: [String]
    public var recommended: String
    /// `--tracker`: the answer's key in the flag map is `tracker`.
    public var flag: String

    public init(id: String, text: String, options: [String], recommended: String, flag: String) {
        self.id = id; self.text = text; self.options = options; self.recommended = recommended; self.flag = flag
    }

    public var key: String { String(flag.drop(while: { $0 == "-" })).replacingOccurrences(of: "-", with: "_") }
    public var isFreeText: Bool { options.isEmpty }
}

/// A file the plan would write.
public struct OnboardingFile: Equatable, Sendable {
    public var path: String
    public var contents: String

    public init(path: String, contents: String) { self.path = path; self.contents = contents }
}

/// What `POST /onboarding/plan` said.
public struct OnboardingPlan: Equatable, Sendable {
    public var id = ""
    public var remote = ""
    public var base = ""
    public var languages: [String] = []
    public var ci: [String] = []
    public var agents: [String] = []
    /// "init" or "adopt".
    public var kind = "init"
    public var next: OnboardingQuestion?
    public var files: [OnboardingFile] = []
    public var errors: [String] = []
    /// What the plan warns of (an agent CLI not found, a token missing).
    public var warnings: [String] = []

    public init() {}

    public init?(json: String) {
        guard let o = (try? JSONSerialization.jsonObject(with: Data(json.utf8))) as? [String: Any] else { return nil }
        let d = o["detected"] as? [String: Any] ?? [:]
        id = d["id"] as? String ?? ""
        if let r = d["remote"] as? [String: Any] {
            remote = [r["host"] as? String, r["repo"] as? String].compactMap { $0 }.joined(separator: "/")
        }
        base = d["branch"] as? String ?? ""
        languages = d["languages"] as? [String] ?? []
        ci = d["ci"] as? [String] ?? []
        // Each agent is serialized as a (name, path) pair.
        agents = (d["agents"] as? [Any] ?? []).compactMap { a in
            if let p = a as? [Any], let n = p.first as? String { return n.lowercased() }
            return a as? String
        }
        kind = o["kind"] as? String ?? "init"
        if let q = o["next"] as? [String: Any], let qid = q["id"] as? String {
            next = OnboardingQuestion(
                id: qid, text: q["text"] as? String ?? qid, options: q["options"] as? [String] ?? [],
                recommended: q["default"] as? String ?? "", flag: q["flag"] as? String ?? "--\(qid)")
        }
        files = (o["files"] as? [[String: Any]] ?? []).map {
            OnboardingFile(path: $0["path"] as? String ?? "", contents: $0["contents"] as? String ?? "")
        }
        errors = o["errors"] as? [String] ?? []
        warnings = o["warnings"] as? [String] ?? []
    }
}

/// The wizard of Add Project, as plain state: the views draw it and the model feeds it the
/// daemon's answers, so none of it needs a view to be tested. The steps and their names are the
/// Compose desktop wizard's too.
public struct AddProjectWizard: Equatable, Sendable {
    public enum Step: String, CaseIterable, Sendable {
        case repository = "Repository"
        case detected = "Detected"
        case questions = "Questions"
        case review = "Review"
        case apply = "Apply"
    }

    /// The landing answers, as `mason init --land` takes them.
    public static let landChoices = ["no", "merge", "push", "pr"]

    public var step = Step.repository
    public var candidates: [OnboardingCandidate] = []
    public var candidateErrors: [String] = []
    public var path = ""
    /// The flag map sent with every plan and the apply, by flag field name.
    public var answers: [String: String] = [:]
    public var plan: OnboardingPlan?
    /// True while a request to the daemon is out.
    public var busy = false
    /// "Use recommended for the rest" is filling in answers.
    public var fillingRest = false
    /// Why the last plan or apply was refused.
    public var refusal: String?
    public var land = "no"
    public var proveTask = true
    public var progress: [String] = []
    public var appliedId: String?
    public var proofTask: UInt64?
    /// What the apply said it did not do (a failed registration, a proof task not filed).
    public var warnings: [String] = []
    /// What the apply noted (choices made for the owner); kept until the sheet is closed.
    public var notes: [String] = []

    public init() {}

    public var canPlan: Bool { !path.trimmingCharacters(in: .whitespaces).isEmpty && !busy }

    /// The question being asked, when the plan has one left.
    public var question: OnboardingQuestion? { plan?.next }

    /// The body for `/onboarding/plan` or `/onboarding/apply`. The landing choice and the proof
    /// task belong to the review page: they are sent only from there on.
    public func body(forApply: Bool) -> String {
        var a = answers
        if forApply {
            a["land"] = land
            if !proveTask { a["no_prove"] = "true" }
        }
        var flags: [String: Any] = [:]
        for (k, v) in a { flags[k] = (k == "no_prove") ? true : v }
        let o: [String: Any] = ["path": path.trimmingCharacters(in: .whitespaces), "answers": flags]
        let data = (try? JSONSerialization.data(withJSONObject: o, options: [.sortedKeys])) ?? Data("{}".utf8)
        return String(data: data, encoding: .utf8) ?? "{}"
    }

    // MARK: moving

    /// The plan came back (for the repository on the Repository page, or after an answer).
    /// Returns true when the plan must be asked for again (answers were filled in for the rest).
    @discardableResult
    public mutating func received(plan p: OnboardingPlan) -> Bool {
        busy = false
        refusal = nil
        if !p.errors.isEmpty && p.files.isEmpty && p.next == nil && plan == nil {
            refusal = p.errors.joined(separator: "\n")
            return false
        }
        plan = p
        if fillingRest, let q = p.next, answers[q.key] == nil {
            answers[q.key] = q.recommended
            return true
        }
        fillingRest = false
        if step == .repository { step = .detected }
        else if step == .questions, p.next == nil { step = .review }
        return false
    }

    public mutating func planRefused(_ why: String) {
        busy = false
        refusal = why
    }

    /// Detected: on to the questions, or the review when nothing is left to ask.
    public mutating func continueFromDetected() {
        step = (plan?.next == nil) ? .review : .questions
    }

    /// Answer the question being asked. The caller asks for the next plan.
    public mutating func answer(_ value: String) {
        guard let q = question else { return }
        answers[q.key] = value
    }

    /// "Use recommended for the rest": answer the current question with its recommended option and
    /// keep answering each following one the same way as plans come back (`received(plan:)`).
    public mutating func useRecommendedForRest() {
        guard let q = question else { return }
        fillingRest = true
        answers[q.key] = q.recommended
    }

    public mutating func back() {
        refusal = nil
        fillingRest = false
        switch step {
        case .repository, .apply: break
        case .detected: step = .repository; plan = nil; answers = [:]
        case .questions: step = .detected
        case .review: step = (plan?.next == nil && answers.isEmpty) ? .detected : .questions
        }
    }

    public mutating func startApply() {
        step = .apply
        busy = true
        refusal = nil
        progress = []
        warnings = []
        notes = []
    }

    /// `/onboarding/apply` answered. On success the new project is selected by the caller (see
    /// `MonitorModel.handleOnboarding`); on refusal the wizard goes back to Review with the reason.
    public mutating func applied(ok: Bool, json: String, error: String) {
        busy = false
        let o = (try? JSONSerialization.jsonObject(with: Data(json.utf8))) as? [String: Any]
        guard ok, let o else {
            refusal = error.isEmpty ? "The daemon refused." : error
            step = .review
            return
        }
        progress = o["progress"] as? [String] ?? []
        warnings = o["warnings"] as? [String] ?? []
        notes = o["notes"] as? [String] ?? []
        if let e = o["error"] as? String { warnings.append(e) }
        if let e = o["proof_error"] as? String { warnings.append("The proof task was not filed: \(e)") }
        if let t = o["proof_task"] as? UInt64 { proofTask = t }
        else if let t = o["proof_task"] as? Int { proofTask = UInt64(t) }
        if (o["registered"] as? Bool) == true || o["error"] == nil {
            appliedId = o["id"] as? String
        } else {
            refusal = warnings.joined(separator: "\n")
            step = .review
        }
    }
}

extension OnboardingCandidate {
    static func list(json: String) -> (candidates: [OnboardingCandidate], errors: [String]) {
        guard let o = (try? JSONSerialization.jsonObject(with: Data(json.utf8))) as? [String: Any] else { return ([], []) }
        let c = (o["candidates"] as? [[String: Any]] ?? []).compactMap { v -> OnboardingCandidate? in
            guard let p = v["path"] as? String else { return nil }
            return OnboardingCandidate(path: p, name: v["name"] as? String ?? p, hasMason: v["has_mason"] as? Bool ?? false, remote: v["remote"] as? String)
        }
        let e = (o["errors"] as? [[String: Any]] ?? []).map { "\($0["path"] as? String ?? ""): \($0["error"] as? String ?? "")" }
        return (c, e)
    }
}
