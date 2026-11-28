// SPDX-License-Identifier: GPL-3.0-or-later

import MasonProtocol
import XCTest

@testable import MonitorCore

/// s527: the badge, the blocker, the chain and the lease rows, from a fixture of the 2026-11-16
/// stall (an exclusive `mason run` with no task, behind #510's compile slot).
final class QueueWaitTests: XCTestCase {
    static func lease() -> Mason_V1_WaitParty {
        var p = Mason_V1_WaitParty()
        p.label = "starpipe heavy"
        p.lease = true
        p.origin = "starpipe in /work/star"
        p.needs = "exclusive"
        p.exclusive = true
        p.sinceMs = 1_000_000
        return p
    }

    static func holder() -> Mason_V1_WaitParty {
        var p = Mason_V1_WaitParty()
        p.label = "#510"
        p.task = 510
        p.needs = "compile"
        p.holding = true
        return p
    }

    static func behindExclusive() -> Mason_V1_WaitInfo {
        var w = Mason_V1_WaitInfo()
        w.cause = "behind-exclusive"
        w.resource = "exclusive"
        w.position = 12
        w.blocker = lease()
        w.holders = [holder()]
        var l0 = Mason_V1_WaitLink()
        l0.party = lease()
        l0.cause = "full"
        l0.sentence = "Waiting for the machine: 1st in line, held by #510 for 8 min"
        var l1 = Mason_V1_WaitLink()
        l1.party = holder()
        w.chain = [l0, l1]
        return w
    }

    func testBadgeNamesTheCauseForEveryCode() {
        for c in ["full", "behind", "behind-exclusive", "exclusive", "cpu", "settling", "disk-stall",
                  "syspolicyd", "memory", "disk-floor", "too-big", "unknown"] {
            XCTAssertFalse(QueueWait.badge(c).isEmpty, c)
        }
        XCTAssertEqual(QueueWait.badge("behind-exclusive"), "behind exclusive")
    }

    func testTheBlockerIsNamedNotJustThePlaceInLine() {
        let w = Self.behindExclusive()
        XCTAssertEqual(QueueWait.blocker(w), "behind an exclusive request `starpipe heavy`")
        var full = Mason_V1_WaitInfo()
        full.cause = "full"
        full.resource = "compile"
        full.holders = [Self.holder()]
        XCTAssertEqual(QueueWait.blocker(full), "waiting for #510's compile slot")
        var gate = Mason_V1_WaitInfo()
        gate.cause = "cpu"
        XCTAssertEqual(QueueWait.blocker(gate), "")
    }

    func testTheChainRunsFromThisTaskToTheRunningHolder() {
        let lines = QueueWait.chain(Self.behindExclusive(), task: "#88")
        XCTAssertEqual(lines, [
            "#88 waits for `starpipe heavy` (starpipe in /work/star)",
            "`starpipe heavy` (starpipe in /work/star): Waiting for the machine: 1st in line, held by #510 for 8 min",
            "#510 is running",
        ])
    }

    func testALeaseRowSaysWhoWhereWhatAndHowLong() {
        let line = QueueWait.lease(Self.lease(), now: 1_000_000 + 9 * 60_000)
        XCTAssertEqual(line, "starpipe heavy, from starpipe in /work/star, wants exclusive, waiting 9 min")
        var held = Self.lease()
        held.holding = true
        XCTAssertTrue(QueueWait.lease(held, now: 1_000_000 + 60_000).contains("holds exclusive, for 1 min"))
    }

    /// Task 1020: a long hold names a count and three tasks, a short one names them all.
    func testLongBlockerListIsShortenedInTheLine() {
        func row(_ n: Int) -> Mason_V1_TaskRow {
            var r = Mason_V1_TaskRow()
            r.blockers = (0..<n).map { UInt64(990 + $0) }
            r.whyWaiting = "Proposed, held behind " + r.blockers.map { "#\($0)" }.joined(separator: ", ")
            return r
        }
        XCTAssertEqual(QueueWait.line(row(29)), "Proposed, held behind 29 tasks (#990, #991, #992, ...)")
        XCTAssertEqual(QueueWait.line(row(3)), "Proposed, held behind #990, #991, #992")
    }

    /// Task 1260: a suite waiting for a node's slot says why, who holds it, its place in line and
    /// the estimate; the monitor shows the daemon's sentence under the cause's badge.
    func testANodeWaitShowsReasonHoldersPlaceAndEstimate() {
        var r = Mason_V1_TaskRow()
        r.wait.cause = "full"
        r.wait.resource = "node:abc"
        r.wait.position = 2
        r.whyWaiting = "Waiting for a node: node full: every job slot is in use; held by #1136 cargo-workspace for 4 min; 2nd in line; no estimate"
        let line = QueueWait.line(r)
        XCTAssertTrue(line.hasPrefix("[\(QueueWait.badge("full"))] Waiting for a node: node full"), line)
        XCTAssertTrue(line.contains("held by #1136 cargo-workspace for 4 min"), line)
        XCTAssertTrue(line.contains("2nd in line"), line)
        XCTAssertTrue(line.hasSuffix("no estimate"), line)
    }
}
