// SPDX-License-Identifier: GPL-3.0-or-later

import Foundation
import MasonProtocol

public typealias RemoteRuns = Mason_V1_RemoteRuns
public typealias RemoteRun = Mason_V1_RemoteRun
public typealias RemoteJob = Mason_V1_RemoteJob

/// The text of home's GitHub section. The daemon sends every number; this only words them, the
/// same way in both clients (`GitHubRuns.kt`).
public enum GitHubRuns {
    public static let empty = "Nothing on GitHub."

    /// "MASON 7 of 20 runners, 13 waiting".
    public static func header(_ g: RemoteRuns) -> String {
        "MASON \(g.poolUsed) of \(g.poolLimit) runners, \(g.waiting) waiting"
    }

    /// "3 min", "40 s".
    public static func duration(_ s: UInt64) -> String {
        s < 90 ? "\(s) s" : "\(s / 60) min"
    }

    /// "#12 Check me".
    public static func task(_ r: RemoteRun) -> String {
        r.title.isEmpty ? r.owner : "\(r.owner) \(r.title)"
    }

    /// "agent check unit on ubuntu-latest, 8 shards".
    public static func kind(_ r: RemoteRun) -> String {
        var t = r.suite.isEmpty ? r.kind : "\(r.kind) \(r.suite)"
        if !r.runsOn.isEmpty { t += " on \(r.runsOn)" }
        if r.shards > 1 { t += ", \(r.shards) shards" }
        return t
    }

    /// "waiting for a runner", "queued on GitHub 40 s", "running 4 min", "green".
    public static func state(_ r: RemoteRun) -> String {
        switch r.state {
        case "waiting": return "waiting for a runner, \(duration(r.elapsedS))"
        case "queued": return "queued on GitHub \(duration(r.elapsedS))"
        case "running": return "running \(duration(r.elapsedS))"
        default: return r.state
        }
    }

    /// The whole row as one line, for tests and accessibility.
    public static func row(_ r: RemoteRun) -> String {
        var parts = [task(r)]
        if !r.taskState.isEmpty { parts.append(r.taskState) }
        parts.append(kind(r))
        parts.append(state(r))
        if !r.progress.isEmpty { parts.append(r.progress) }
        parts.append("updated \(duration(r.updatedAgoS)) ago")
        return parts.joined(separator: " · ")
    }

    /// A job's line on the section's page: "run (3): running, step run".
    public static func job(_ j: RemoteJob) -> String {
        var t = "\(j.name): \(j.state)"
        if !j.conclusion.isEmpty { t += " (\(j.conclusion))" }
        if !j.step.isEmpty { t += ", step \(j.step)" }
        return t
    }
}
