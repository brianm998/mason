// SPDX-License-Identifier: GPL-3.0-or-later

import Foundation
import MasonProtocol

/// What a task is waiting on, as the daemon sends it (s527): the cause as a badge, the blocker
/// named, the chain behind it, and the leases that belong to no task. The same wording in the
/// Compose core (`QueueWait.kt`).
public enum QueueWait {
    /// The small badge on a waiting row. Never empty for a known cause.
    public static func badge(_ cause: String) -> String {
        switch cause {
        case "full": return "slot full"
        case "behind": return "in line"
        case "behind-exclusive": return "behind exclusive"
        case "exclusive": return "machine busy"
        case "cpu": return "CPU"
        case "settling": return "settling"
        case "disk-stall": return "build disk slow"
        case "syspolicyd": return "syspolicyd"
        case "memory": return "memory low"
        case "disk-floor": return "disk low"
        case "too-big": return "too big"
        case "unknown": return "unknown resource"
        default: return cause
        }
    }

    /// A party in words: "#510", or "`starpipe heavy` (starpipe in /x/star)" for a lease.
    public static func party(_ p: Mason_V1_WaitParty) -> String {
        if p.task > 0 { return "#\(p.task)" }
        let origin = p.origin.isEmpty ? "" : " (\(p.origin))"
        return "`\(p.label)`\(origin)"
    }

    /// Who the task is behind, for a row: "behind an exclusive request `starpipe heavy`",
    /// "waiting for #510's compile slot", or "" when a gate holds it.
    public static func blocker(_ w: Mason_V1_WaitInfo) -> String {
        if w.hasBlocker {
            let b = w.blocker
            let what = b.exclusive ? "an exclusive request " : ""
            return "behind \(what)\(b.task > 0 ? "#\(b.task)" : "`\(b.label)`")"
        }
        if let h = w.holders.first {
            let more = w.holders.count > 1 ? " and \(w.holders.count - 1) more" : ""
            if h.task > 0 { return "waiting for #\(h.task)'s \(w.resource) slot\(more)" }
            return "waiting for `\(h.label)`\(more)"
        }
        return ""
    }

    /// The task page's chain: this task waits for X; X waits for Y.
    public static func chain(_ w: Mason_V1_WaitInfo, task: String) -> [String] {
        var lines: [String] = []
        for (i, link) in w.chain.enumerated() {
            let p = party(link.party)
            if i == 0 { lines.append("\(task) waits for \(p)") }
            if link.sentence.isEmpty {
                lines.append("\(p) is \(link.party.holding ? "running" : "starting")")
            } else {
                lines.append("\(p): \(link.sentence)")
            }
        }
        return lines
    }

    /// What follows a lease's label: "from starpipe in /x/star, wants exclusive, waiting 9 min".
    public static func leaseDetail(_ p: Mason_V1_WaitParty, now: UInt64) -> String {
        let age = Format.duration(ms: now > p.sinceMs ? now - p.sinceMs : 0)
        var parts: [String] = []
        if !p.origin.isEmpty { parts.append("from \(p.origin)") }
        if !p.needs.isEmpty { parts.append("\(p.holding ? "holds" : "wants") \(p.needs)") }
        parts.append(p.holding ? "for \(age)" : "waiting \(age)")
        return parts.joined(separator: ", ")
    }

    /// One lease that belongs to no task, on the banner:
    /// "starpipe heavy, from starpipe in /x/star, wants exclusive, waiting 9 min".
    public static func lease(_ p: Mason_V1_WaitParty, now: UInt64) -> String {
        "\(p.label), \(leaseDetail(p, now: now))"
    }

    /// A row's second line: "[badge] sentence" while it waits on the scheduler, else its activity.
    public static func line(_ r: Mason_V1_TaskRow) -> String {
        let base = lineWithoutDiagnosis(r)
        guard let d = diagnosisBadge(r.diagnosis) else { return base }
        return base.isEmpty ? d : "\(base) · \(d)"
    }

    private static func lineWithoutDiagnosis(_ r: Mason_V1_TaskRow) -> String {
        let why = shorten(r.whyWaiting, blockers: r.blockers)
        if r.hasWait && !why.isEmpty { return "[\(badge(r.wait.cause))] \(why)" }
        return why.isEmpty ? shorten(r.activity, blockers: r.blockers) : why
    }

    /// Blockers named in full up to this many; a longer list reads "N tasks (#a, #b, #c, ...)".
    public static let namedBlockers = 3

    /// `text` with the full blocker list ("#a, #b, ...") replaced by a count and the first three.
    static func shorten(_ text: String, blockers: [UInt64]) -> String {
        guard blockers.count > namedBlockers else { return text }
        let all = blockers.map { "#\($0)" }.joined(separator: ", ")
        let first = blockers.prefix(namedBlockers).map { "#\($0)" }.joined(separator: ", ")
        return text.replacingOccurrences(of: all, with: "\(blockers.count) tasks (\(first), ...)")
    }
}
