// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import kotlin.test.*
import mason.monitor.LinkSegment.Plain
import mason.monitor.LinkSegment.Task
import mason.monitor.LinkSegment.Url
import mason.v1.CiLink

class LinkifyTest {
    private fun ci(label: String, url: String, kind: String = "check") = CiLink(label = label, url = url, kind = kind)
    private fun seg(s: String, known: Set<Long> = emptySet(), links: List<CiLink> = emptyList()) =
        Linkify.segments(s, { it in known }, links)

    @Test fun aKnownTaskIsALinkAndAnUnknownOneIsPlain() {
        assertEquals(listOf(Plain("waiting for "), Task(12, "#12"), Plain(" now")), seg("waiting for #12 now", setOf(12)))
        assertEquals(listOf(Plain("waiting for #12 now")), seg("waiting for #12 now"))
    }

    @Test fun aPullRequestWithALinkIsAUrlNeverATask() {
        val l = listOf(ci("PR #7", "https://github.com/o/r/pull/7", "pull_request"))
        assertEquals(
            listOf(Url("https://github.com/o/r/pull/7", "PR #7"), Plain(" (mason/pr/x): checks failed: lint")),
            seg("PR #7 (mason/pr/x): checks failed: lint", setOf(7), l),
        )
    }

    @Test fun aPullRequestWithoutALinkIsPlainEvenWhenTheNumberIsATask() {
        assertEquals(listOf(Plain("PR #7: checks green")), seg("PR #7: checks green", setOf(7)))
        assertEquals(listOf(Plain("see the pull request #7")), seg("see the pull request #7", setOf(7)))
    }

    @Test fun aFailedChecksNameIsAUrlLink() {
        val l = listOf(ci("lint", "https://example.com/job/1"))
        assertEquals(
            listOf(Plain("checks failed: "), Url("https://example.com/job/1", "lint"), Plain(", build")),
            seg("checks failed: lint, build", links = l),
        )
        assertEquals(listOf(Plain("linting")), seg("linting", links = l))
    }

    @Test fun aBareHttpsUrlIsAUrlLink() {
        assertEquals(
            listOf(Plain("see "), Url("https://example.com/x", "https://example.com/x"), Plain(".")),
            seg("see https://example.com/x."),
        )
    }

    @Test fun batchRunAndDebtNumbersStayPlain() {
        assertEquals(listOf(Plain("Batch #3 run 4 debt #4")), seg("Batch #3 run 4 debt #4", setOf(3, 4)))
    }

    @Test fun aNonHttpUrlInACiLinkIsIgnored() {
        val l = listOf(ci("PR #7", "fake://pr/7", "pull_request"), ci("lint", "javascript:alert(1)"))
        assertEquals(listOf(Plain("PR #7 lint")), seg("PR #7 lint", setOf(7), l))
    }

    @Test fun thePullRequestChecksWaitLineLinksThePullRequestNotATask() {
        val l = listOf(ci("PR #9", "https://github.com/o/r/pull/9", "pull_request"))
        assertEquals(
            listOf(Plain("waiting for the pull request's checks ("), Url("https://github.com/o/r/pull/9", "#9"), Plain(")")),
            seg("waiting for the pull request's checks (#9)", setOf(9), l),
        )
        assertEquals(
            listOf(Plain("waiting for the pull request's checks (#9)")),
            seg("waiting for the pull request's checks (#9)", setOf(9)),
        )
    }

    @Test fun aLongerPullRequestNumberIsNotMatchedByAShorterLabel() {
        val l = listOf(ci("PR #7", "https://github.com/o/r/pull/7", "pull_request"))
        assertEquals(listOf(Plain("PR #70 failed")), seg("PR #70 failed", links = l))
    }
}
