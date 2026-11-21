// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import mason.v1.*
import kotlin.test.*

/** #767: the words of home's GitHub section. */
class GitHubRunsTest {
    @Test
    fun theHeaderAndTheEmptyState() {
        assertEquals("MASON 7 of 20 runners, 13 waiting", GitHubRuns.header(GithubRunList(pool_used = 7, pool_limit = 20, waiting = 13)))
        assertEquals("Nothing on GitHub.", GitHubRuns.EMPTY)
    }

    @Test
    fun aRowForAQueuedCheckARunningShardedDebtAndAWaitingLint() {
        val check = GithubRun(
            owner = "#1", title = "Check me", task_state = "testing", kind = "agent check", suite = "unit",
            runs_on = "ubuntu-latest", state = "queued", elapsed_s = 40, updated_ago_s = 5,
        )
        assertEquals("#1 Check me · testing · agent check unit on ubuntu-latest · queued on GitHub 40 s · updated 5 s ago", GitHubRuns.row(check))
        val debt = GithubRun(
            owner = "debt #2", title = "Pay", task_state = "testing", kind = "debt payment", suite = "workspace", shards = 8,
            runs_on = "ubuntu-latest", state = "running", elapsed_s = 240, progress = "3 of 8 jobs done, step run", updated_ago_s = 20,
        )
        assertEquals(
            "debt #2 Pay · testing · debt payment workspace on ubuntu-latest, 8 shards · running 4 min · 3 of 8 jobs done, step run · updated 20 s ago",
            GitHubRuns.row(debt),
        )
        val lint = GithubRun(owner = "b480", kind = "landing lint", runs_on = "ubuntu-latest", state = "waiting", elapsed_s = 30, updated_ago_s = 30)
        assertEquals("b480 · landing lint on ubuntu-latest · waiting for a runner, 30 s · updated 30 s ago", GitHubRuns.row(lint))
    }

    @Test
    fun aJobLine() {
        assertEquals("run (3): running, step run", GitHubRuns.job(GithubJob(name = "run (3)", state = "running", step = "run")))
        assertEquals("run (3): done (success)", GitHubRuns.job(GithubJob(name = "run (3)", state = "done", conclusion = "success")))
    }
}
