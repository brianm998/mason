// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import mason.v1.WaitInfo
import mason.v1.WaitLink
import mason.v1.WaitParty
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

/** s527: the badge, the blocker, the chain and the lease rows, from a fixture of the 2026-11-16 stall. */
class QueueWaitTest {
    private val lease = WaitParty(
        label = "starpipe heavy", lease = true, origin = "starpipe in /work/star",
        needs = "exclusive", exclusive = true, since_ms = 1_000_000,
    )
    private val holder = WaitParty(label = "#510", task = 510, needs = "compile", holding = true)
    private val behind = WaitInfo(
        cause = "behind-exclusive", resource = "exclusive", position = 12, blocker = lease, holders = listOf(holder),
        chain = listOf(
            WaitLink(party = lease, cause = "full", sentence = "Waiting for the machine: 1st in line, held by #510 for 8 min"),
            WaitLink(party = holder),
        ),
    )

    @Test
    fun badgeNamesTheCauseForEveryCode() {
        for (c in listOf("full", "behind", "behind-exclusive", "exclusive", "cpu", "settling", "disk-stall",
            "syspolicyd", "memory", "disk-floor", "too-big", "unknown")) assertTrue(QueueWait.badge(c).isNotEmpty(), c)
        assertEquals("behind exclusive", QueueWait.badge("behind-exclusive"))
    }

    @Test
    fun theBlockerIsNamedNotJustThePlaceInLine() {
        assertEquals("behind an exclusive request `starpipe heavy`", QueueWait.blocker(behind))
        assertEquals("waiting for #510's compile slot", QueueWait.blocker(WaitInfo(cause = "full", resource = "compile", holders = listOf(holder))))
        assertEquals("", QueueWait.blocker(WaitInfo(cause = "cpu")))
    }

    @Test
    fun theChainRunsFromThisTaskToTheRunningHolder() {
        assertEquals(
            listOf(
                "#88 waits for `starpipe heavy` (starpipe in /work/star)",
                "`starpipe heavy` (starpipe in /work/star): Waiting for the machine: 1st in line, held by #510 for 8 min",
                "#510 is running",
            ),
            QueueWait.chain(behind, "#88"),
        )
    }

    @Test
    fun aLeaseRowSaysWhoWhereWhatAndHowLong() {
        assertEquals("starpipe heavy, from starpipe in /work/star, wants exclusive, waiting 9 min",
            QueueWait.lease(lease, 1_000_000 + 9 * 60_000))
        assertEquals("holds exclusive, for 1 min", QueueWait.leaseDetail(lease.copy(holding = true), 1_000_000 + 60_000).substringAfter("star, "))
    }
}
