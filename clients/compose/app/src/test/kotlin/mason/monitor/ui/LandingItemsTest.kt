// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor.ui

import mason.monitor.LandNowTarget
import mason.monitor.MonitorModel
import mason.monitor.askLandNow
import mason.monitor.canLandNow
import mason.monitor.cancelLandNow
import mason.monitor.confirmLandNow
import mason.monitor.landNowBatches
import mason.monitor.landNowLabel
import mason.v1.BatchLine
import mason.v1.Command
import mason.v1.TaskLanding
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertTrue

class LandingItemsTest {
    private fun item(task: Long, batch: Long = 0, position: Int = 0, why: String = "") =
        if (batch > 0) {
            TaskLanding(task = task, key = "#$task", title = "t$task", status = "in_batch", batch = batch,
                phase = "running suites", phase_index = 2, phase_total = 4, phase_ms = 120_000)
        } else {
            TaskLanding(task = task, key = "#$task", title = "t$task", status = "queued", position = position, why = why)
        }

    @Test
    fun groupsLandingAboveQueuedInQueueOrder() {
        val g = landingGroups(listOf(item(3, position = 2, why = "x"), item(1, batch = 7), item(4, position = 1, why = "x")))
        assertEquals(listOf(1L), g.first.map { it.task })
        assertEquals(listOf(4L, 3L), g.second.map { it.task })
    }

    @Test
    fun text() {
        assertEquals("#1 t1: batch #7, running suites, phase 2 of 4 ▰▰▱▱, 2 min", landingItemText(item(1, batch = 7)))
        assertEquals("#3 t3: queued #1, behind 1 batch", landingItemText(item(3, position = 1, why = "behind 1 batch")))
    }

    @Test
    fun batchGroupsUnderOneHeaderAndMembersCarryNoBatchText() {
        fun member(t: Long, b: Long) = item(t, batch = b).copy(standing = "verifying")
        val h = BatchLine(
            id = 7, running = true, phase = "red", state_text = "red in lint, splitting (round 1, 2 tasks left)",
            severity = "warn", phase_ms = 17 * 60_000, typical_ms = 6 * 60_000,
            members = listOf(1L, 2L).map { mason.v1.BatchMember(task = it, standing = "verifying") },
            allowed_actions = listOf("land_now"),
        )
        val loose = item(9, batch = 0).copy(status = "in_batch")
        val l = landingLayout(listOf(loose, member(1, 7), member(2, 7)), listOf(h))
        assertEquals(listOf(7L), l.groups.map { it.batch })
        assertEquals(listOf(1L, 2L), l.groups[0].members.map { it.task })
        assertEquals(listOf(9L), l.loose.map { it.task })
        assertEquals(
            "Batch #7 · 2 tasks · red in lint, splitting (round 1, 2 tasks left) · 17 min (usually ~6 min)",
            batchHeaderText(l.groups[0]),
        )
        l.groups[0].members.forEach { assertFalse(landingItemText(it).contains("batch #"), landingItemText(it)) }
        assertEquals("#1 t1: verifying", landingItemText(member(1, 7)))
        assertFalse(batchHeaderText(LandingBatchGroup(7, h.copy(typical_ms = 0), emptyList())).contains("usually"))
        assertEquals(listOf(7L), landNowBatches(listOf(h)).map { it.id })
    }

    private fun owner(): MonitorModel {
        val m = MonitorModel()
        m.connected = true
        m.isOwner = true
        return m
    }

    private fun offered(i: TaskLanding) = i.copy(allowed_actions = listOf("land_now"))

    private fun batch(id: Long, running: Boolean, vararg actions: String) =
        BatchLine(id = id, running = running, phase = "verifying", tasks = listOf(1, 2), allowed_actions = actions.toList())

    @Test
    fun landNowIsOfferedOnlyWhereAllowedAndTheDeviceCanAct() {
        val m = owner()
        assertTrue(m.canLandNow(offered(item(1, batch = 7))))
        assertTrue(m.canLandNow(offered(item(3, position = 1))))
        assertFalse(m.canLandNow(item(1, batch = 7)), "the daemon did not offer it (promoting or landed)")
        assertTrue(m.canLandNow(batch(7, true, "land_now")))
        assertFalse(m.canLandNow(batch(7, true)))
        assertFalse(m.canLandNow(batch(7, false, "land_now")))
        assertEquals(listOf(7L), landNowBatches(listOf(batch(7, true, "land_now"), batch(8, true))).map { it.id })
        m.isOwner = false
        assertFalse(m.canLandNow(offered(item(1, batch = 7))), "not the owner's device")
        assertFalse(m.canLandNow(batch(7, true, "land_now")))
    }

    @Test
    fun landNowAsksFirstThenSendsTheRightCommand() {
        val m = owner()
        val sent = mutableListOf<Command>()
        m.sender = { c -> sent += c }
        m.askLandNow(LandNowTarget.Task(5))
        assertTrue(sent.isEmpty(), "nothing is sent before the confirm")
        assertEquals(
            "Land #5 now? Its remaining tests run after the merge; a red result raises an alert.",
            LandNowTarget.Task(5).confirmText,
        )
        m.cancelLandNow()
        m.confirmLandNow()
        assertTrue(sent.isEmpty(), "a cancelled ask sends nothing")
        m.askLandNow(LandNowTarget.Task(5))
        m.confirmLandNow()
        assertEquals(5L, sent.single().land_now?.task)
        assertEquals("Landing…", m.landNowLabel(LandNowTarget.Task(5)))
        m.askLandNow(LandNowTarget.Batch(9))
        m.confirmLandNow()
        assertEquals(9L, sent[1].land_now_batch?.batch)
        assertEquals("Landing…", m.landNowLabel(LandNowTarget.Batch(9)))
        assertNull(m.landNowLabel(LandNowTarget.Batch(5)), "a batch's pending state is not a task's")
    }
}
