// SPDX-License-Identifier: GPL-3.0-or-later

import MasonProtocol
import XCTest

@testable import MonitorCore

/// #767: the words of home's GitHub section.
final class GitHubRunsTests: XCTestCase {
    private func run(_ f: (inout GithubRun) -> Void) -> GithubRun {
        var r = GithubRun()
        r.runsOn = "ubuntu-latest"
        f(&r)
        return r
    }

    func testTheHeaderAndTheEmptyState() {
        var g = GithubRunList()
        g.poolUsed = 7
        g.poolLimit = 20
        g.waiting = 13
        XCTAssertEqual(GitHubRuns.header(g), "MASON 7 of 20 runners, 13 waiting")
        XCTAssertEqual(GitHubRuns.empty, "Nothing on GitHub.")
    }

    func testARowForAQueuedCheckARunningShardedDebtAndAWaitingLint() {
        let check = run {
            $0.owner = "#1"; $0.title = "Check me"; $0.taskState = "testing"
            $0.kind = "agent check"; $0.suite = "unit"; $0.state = "queued"
            $0.elapsedS = 40; $0.updatedAgoS = 5
        }
        XCTAssertEqual(GitHubRuns.row(check),
                       "#1 Check me · testing · agent check unit on ubuntu-latest · queued on GitHub 40 s · updated 5 s ago")
        let debt = run {
            $0.owner = "debt #2"; $0.title = "Pay"; $0.taskState = "testing"
            $0.kind = "debt payment"; $0.suite = "workspace"; $0.shards = 8; $0.state = "running"
            $0.elapsedS = 240; $0.progress = "3 of 8 jobs done, step run"; $0.updatedAgoS = 20
        }
        XCTAssertEqual(GitHubRuns.row(debt),
                       "debt #2 Pay · testing · debt payment workspace on ubuntu-latest, 8 shards · running 4 min · 3 of 8 jobs done, step run · updated 20 s ago")
        let lint = run {
            $0.owner = "b480"; $0.kind = "landing lint"; $0.state = "waiting"
            $0.elapsedS = 30; $0.updatedAgoS = 30
        }
        XCTAssertEqual(GitHubRuns.row(lint),
                       "b480 · landing lint on ubuntu-latest · waiting for a runner, 30 s · updated 30 s ago")
    }

    func testAJobLine() {
        var j = GithubJob()
        j.name = "run (3)"; j.state = "running"; j.step = "run"
        XCTAssertEqual(GitHubRuns.job(j), "run (3): running, step run")
        j.state = "done"; j.conclusion = "success"; j.step = ""
        XCTAssertEqual(GitHubRuns.job(j), "run (3): done (success)")
    }
}
