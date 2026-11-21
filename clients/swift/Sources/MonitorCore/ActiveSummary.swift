// SPDX-License-Identifier: GPL-3.0-or-later

import Foundation
import MasonProtocol

/// The text of the home page's "Active models" section: how many agents are working and how many
/// wait (on a check, here or on GitHub, or on a slot), and what is out on GitHub. The Compose
/// monitor has the same functions in `ActiveSummary.kt`, with the same words.
public enum ActiveSummary {
    /// "7 agents: 2 working, 5 waiting on GitHub checks"; nil with no agent (the page then says
    /// "Nothing is running."). An entry from a daemon that sends no state counts as working.
    public static func header(_ active: [Mason_V1_ActiveModel]) -> String? {
        if active.isEmpty { return nil }
        func n(_ f: (Mason_V1_ActiveModel) -> Bool) -> Int { active.filter(f).count }
        let parts: [(Int, String)] = [
            (n { $0.state.isEmpty || $0.state == "working" }, "working"),
            (n { $0.state == "waiting_check" && $0.place == "github" }, "waiting on GitHub checks"),
            (n { $0.state == "waiting_check" && $0.place != "github" }, "waiting on checks here"),
            (n { $0.state == "waiting_slot" }, "waiting for a slot"),
        ]
        let text = parts.filter { $0.0 > 0 }.map { "\($0.0) \($0.1)" }.joined(separator: ", ")
        return "\(active.count) agent\(active.count == 1 ? "" : "s"): \(text)"
    }

    /// What one agent is doing, for its row.
    public static func state(_ a: Mason_V1_ActiveModel) -> String {
        switch a.state {
        case "waiting_check": return a.place == "github" ? "waiting on a GitHub check" : "waiting on a check here"
        case "waiting_slot": return "waiting for \(a.resource.isEmpty ? "a slot" : a.resource)"
        default: return "working"
        }
    }

    /// "13 of 16 runners: 7 checks, 4 test steps, 2 debts; macOS 2 of 4"; nil when nothing is
    /// out on GitHub.
    public static func github(_ r: Mason_V1_RemoteSummary?) -> String? {
        guard let r = r, !r.runs.isEmpty || r.checks + r.testSteps + r.batches + r.debts > 0 else { return nil }
        func count(_ n: UInt32, _ one: String, _ many: String) -> String? {
            n == 0 ? nil : "\(n) \(n == 1 ? one : many)"
        }
        let parts = [
            count(r.checks, "check", "checks"), count(r.testSteps, "test step", "test steps"),
            count(r.batches, "batch", "batches"), count(r.debts, "debt", "debts"),
        ].compactMap { $0 }.joined(separator: ", ")
        let head = r.pools.first { $0.name == "gha" }.map { "\($0.used) of \($0.limit) runners" } ?? "runners"
        let mac = r.pools.first { $0.name == "gha-macos" && $0.used > 0 }
            .map { "; macOS \($0.used) of \($0.limit)" } ?? ""
        return "\(head): \(parts)\(mac)"
    }

    /// One run of the detail page: "check #760 on ubuntu-latest".
    public static func runLine(_ r: Mason_V1_RemoteRun) -> String {
        let kind = r.kind.replacingOccurrences(of: "_", with: " ")
        let task = r.task == 0 ? r.label : "#\(r.task)"
        return "\(kind) \(task)\(r.runsOn.isEmpty ? "" : " on \(r.runsOn)")"
    }
}
