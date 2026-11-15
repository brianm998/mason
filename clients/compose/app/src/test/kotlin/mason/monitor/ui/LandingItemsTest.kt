// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor.ui

import mason.v1.TaskLanding
import kotlin.test.Test
import kotlin.test.assertEquals

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
}
