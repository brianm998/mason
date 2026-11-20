// SPDX-License-Identifier: GPL-3.0-or-later

import Foundation
import XCTest

/// s661: every string that can name a task goes through `LinkedText` or `TaskLink`, never a bare
/// `Text(`. This scans the sources (no view is drawn, nothing is opened) for `Text(` calls that
/// interpolate `#\(` or pass a wait, batch, lease or hold string, and fails on any not allowed
/// below. Each allowed entry says why it names no task, or why it cannot link.
final class TaskReferenceLinkScanTests: XCTestCase {
    /// (file, a fragment of the line, why it is allowed). The Compose client's scan keeps the same reasons.
    static let allowed: [(String, String, String)] = [
        ("Components.swift", "Edit proposal #", "the title of the editor for that very task; already on its page"),
        ("Components.swift", "upstream pull request #", "a pull request number, not a task"),
        ("Components.swift", "\"issue #", "a tracker issue number, not a task"),
        ("Pages.swift", "Loading task #", "the page of that task is already open"),
        ("Pages.swift", "Text(\"#\\(x.id) \")", "a debt or finding id, not a task"),
        ("Pages.swift", "Event log at #", "the event-log sequence number"),
        ("Phone.swift", "Text(row.activity)", "the bare row without a model (previews, layout tests); the phone list passes the model"),
        ("Phone.swift", "Text(row.whyWaiting)", "the bare row without a model (previews, layout tests); the phone list passes the model"),
        ("Sidebar.swift", "Text(\"\\(row.project), \\(activity)\")", "a sidebar row is one lazy, equatable selection row that opens its task; the task page links its wait line and the row's blockers are TaskLinks"),
        ("Sidebar.swift", "issue #", "a tracker issue number, not a task"),
        ("TopBar.swift", "Text(\"#\\(issue.number)\")", "a tracker issue number, not a task"),
    ]

    private static let risky = try! NSRegularExpression(
        pattern: #"#\\\(|whyWaiting|batchText\(|QueueWait\.lease|leaseDetail|\.line\(|\bchain\b|\bactivity\b"#)

    private func sourcesDir() -> URL {
        URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("Sources/MonitorUI")
    }

    /// The offending lines: a `Text(` (not `LinkedText(`) with a risky argument, and not allowed.
    func offences() throws -> [String] {
        var out: [String] = []
        let dir = sourcesDir()
        for f in try FileManager.default.contentsOfDirectory(atPath: dir.path).sorted() where f.hasSuffix(".swift") {
            let text = try String(contentsOf: dir.appendingPathComponent(f), encoding: .utf8)
            for (n, line) in text.components(separatedBy: "\n").enumerated() {
                let t = line.trimmingCharacters(in: .whitespaces)
                if t.hasPrefix("//") { continue }
                var from = line.startIndex
                while let r = line.range(of: "Text(", range: from..<line.endIndex) {
                    from = r.upperBound
                    if r.lowerBound > line.startIndex, line[line.index(before: r.lowerBound)].isLetter { continue }
                    let arg = String(line[r.upperBound...])
                    let whole = NSRange(arg.startIndex..., in: arg)
                    guard Self.risky.firstMatch(in: arg, range: whole) != nil else { continue }
                    if Self.allowed.contains(where: { $0.0 == f && line.contains($0.1) }) { continue }
                    out.append("\(f):\(n + 1): \(t)")
                }
            }
        }
        return out
    }

    func testNoBareTextNamesATask() throws {
        let bad = try offences()
        XCTAssertTrue(bad.isEmpty, "show these through LinkedText or TaskLink, or allow them with a reason:\n" + bad.joined(separator: "\n"))
    }

    func testEveryAllowedEntryGivesAReasonAndStillMatches() throws {
        let dir = sourcesDir()
        for (f, frag, why) in Self.allowed {
            XCTAssertFalse(why.isEmpty)
            let text = try String(contentsOf: dir.appendingPathComponent(f), encoding: .utf8)
            XCTAssertTrue(text.contains(frag), "stale allowlist entry \(f): \(frag)")
        }
    }
}
