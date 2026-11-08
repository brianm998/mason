// SPDX-License-Identifier: GPL-3.0-or-later

import Foundation
import MasonProtocol

/// A task's plan as text, and the rule for which parts start open. Pure, so tests cover it.
public enum PlanText {
    /// Steps, files and criteria start expanded when the plan has fewer than this many steps.
    public static let expandLimit = 10

    public static func expandedByDefault(stepCount: Int) -> Bool { stepCount < expandLimit }

    /// The label shown before step `i`: its own number as the agent wrote it ("0.", "1."), or a bullet.
    public static func marker(_ v: Mason_V1_PlanView, _ i: Int) -> String? {
        guard v.stepsAreList else { return nil }
        return i < v.stepNumbers.count ? "\(v.stepNumbers[i])." : "•"
    }

    /// The readable plan as plain Markdown: headings, numbered steps, bullets. A plan that could
    /// not be read is returned as written.
    public static func copyAsText(_ v: Mason_V1_PlanView) -> String {
        if !v.note.isEmpty { return v.raw }
        var out: [String] = []
        if !v.summaryParagraphs.isEmpty {
            out.append("## Summary\n\n" + v.summaryParagraphs.joined(separator: "\n\n"))
        }
        if !v.steps.isEmpty || !v.lead.isEmpty {
            var s = "## Steps\n"
            if !v.lead.isEmpty { s += "\n" + v.lead + "\n" }
            if v.stepsAreList {
                s += "\n"
                for (i, step) in v.steps.enumerated() {
                    let m = i < v.stepNumbers.count ? "\(v.stepNumbers[i])." : "-"
                    let pad = String(repeating: " ", count: m.count + 1)
                    let lines = step.split(separator: "\n", omittingEmptySubsequences: false)
                    s += "\(m) \(lines[0])\n"
                    for l in lines.dropFirst() { s += l.isEmpty ? "\n" : "\(pad)\(l)\n" }
                }
            } else {
                s += "\n" + v.steps.joined(separator: "\n\n") + "\n"
            }
            out.append(s.trimmingCharacters(in: .newlines))
        }
        if !v.files.isEmpty {
            out.append("## Files (\(v.files.count))\n\n" + v.files.map { "- `\($0)`" }.joined(separator: "\n"))
        }
        if !v.criteria.isEmpty {
            out.append("## Acceptance criteria (\(v.criteria.count))\n\n" + v.criteria.map { "- [ ] \($0)" }.joined(separator: "\n"))
        }
        if !v.other.isEmpty {
            out.append("## Other\n\n" + v.other.map { "### \($0.label)\n\n\($0.value)" }.joined(separator: "\n\n"))
        }
        return out.joined(separator: "\n\n") + "\n"
    }
}
