// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor.ui

import mason.v1.BatchCi
import mason.v1.WorkflowJobs
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.test.assertTrue

class BatchCiTest {
    @Test
    fun countsAndFailedAreShownPerWorkflow() {
        val ci = BatchCi(
            ci_ref = "mason/ci/star-b1-1",
            url = "https://github.com/o/r/actions/runs/9",
            workflows = listOf(WorkflowJobs(workflow = "pr-gate", queued = 1, running = 2, passed = 7, failed = 1)),
        )
        assertEquals("CI: pr-gate: 11 jobs · 1 queued · 2 running · 7 passed · 1 failed", batchCiText(ci))
        assertTrue(batchCiFailed(ci))
        assertEquals("https://github.com/o/r/actions/runs/9", batchCiUrl(ci))
    }

    @Test
    fun noJobStartedReadsQueuedForARunner() {
        val ci = BatchCi(workflows = listOf(WorkflowJobs(workflow = "pr-gate", queued = 3)))
        assertEquals("CI: pr-gate: queued for a runner", batchCiText(ci))
        assertEquals("CI: queued for a runner", batchCiText(BatchCi()))
        assertTrue(!batchCiFailed(ci))
    }

    @Test
    fun onlyHttpAddressesOpen() {
        assertNull(batchCiUrl(BatchCi(url = "file:///etc/passwd")))
        assertNull(batchCiUrl(BatchCi(url = "")))
    }
}
