// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import mason.v1.*
import kotlin.test.*

/** Task #522: a click is recorded before the command is written, and settled by the answer and then by a view in which the task changed. No clocks. */
class PendingCommandTest {
    private fun row(id: Long, state: String, vararg actions: String) = TaskRow(id = id, key = "#$id", title = "t", state = state, allowed_actions = actions.toList())
    private fun sidebar(vararg rows: TaskRow) = ServerMessage(view = ViewUpdate(view = View(sidebar = SidebarView(sections = listOf(SidebarSection(state = rows.first().state, rows = rows.toList()))))))
    private fun result(id: String, ok: Boolean, error: String = "") = ServerMessage(result = CommandResult(id = id, ok = ok, error = error))

    private class Fixture(val m: MonitorModel, val sent: MutableList<Command>)

    private fun fixture(): Fixture {
        val m = MonitorModel()
        m.connected = true
        m.isOwner = true
        m.apply(sidebar(row(7, "proposed", "approve", "reject")))
        val sent = mutableListOf<Command>()
        m.sender = { c ->
            // The entry and its label exist before the command leaves.
            assertEquals(7L, m.pending[c.id]?.task)
            assertEquals("Approving…", m.pendingLabel(7, "approve"))
            assertTrue(m.isBusy(7))
            sent += c
        }
        return Fixture(m, sent)
    }

    @Test fun approveIsPendingBeforeTheSenderIsCalled() {
        val f = fixture()
        f.m.approve(7)
        assertEquals(1, f.sent.size)
        assertTrue(f.sent[0].id.startsWith("act:approve:7:"))
    }

    @Test fun aSecondClickWhilePendingSendsNothing() {
        val f = fixture()
        f.m.approve(7); f.m.approve(7); f.m.reject(7)
        assertEquals(1, f.sent.size)
    }

    @Test fun aFailureClearsTheEntryAndShowsTheErrorOnThatTask() {
        val f = fixture()
        f.m.approve(7)
        f.m.apply(result(f.sent[0].id, false, "owner only"))
        assertTrue(f.m.pending.isEmpty())
        assertEquals("owner only", f.m.actionError(7))
        assertNull(f.m.actionError(8))
        assertFalse(f.m.isBusy(7))
    }

    @Test fun aSuccessChangesTheLabelAndKeepsTheControlsDisabledUntilTheTaskChanges() {
        val f = fixture()
        f.m.approve(7)
        f.m.apply(result(f.sent[0].id, true))
        assertEquals("Approved, waiting for the pipeline…", f.m.pendingLabel(7, "approve"))
        assertTrue(f.m.isBusy(7))
        f.m.apply(sidebar(row(7, "proposed", "approve", "reject")))
        assertTrue(f.m.isBusy(7), "same state and actions: still waiting")
        f.m.apply(sidebar(row(7, "queued", "cancel")))
        assertTrue(f.m.pending.isEmpty())
    }

    @Test fun aViewThatChangedBeforeTheResultEndsItAtOnce() {
        val f = fixture()
        f.m.approve(7)
        f.m.apply(sidebar(row(7, "queued", "cancel")))
        f.m.apply(result(f.sent[0].id, true))
        assertTrue(f.m.pending.isEmpty())
    }

    @Test fun anAcceptedEntryEndsAfterThreeViewsWithoutAChange() {
        val f = fixture()
        f.m.approve(7)
        f.m.apply(result(f.sent[0].id, true))
        repeat(PendingCommand.VIEWS_TO_WAIT - 1) {
            f.m.apply(sidebar(row(7, "proposed", "approve", "reject")))
            assertTrue(f.m.isBusy(7))
        }
        f.m.apply(sidebar(row(7, "proposed", "approve", "reject")))
        assertFalse(f.m.isBusy(7), "the action is offered again")
    }

    @Test fun disconnectMarksNotSentAndWelcomeClears() {
        val f = fixture()
        f.m.approve(7)
        f.m.disconnected()
        assertEquals("Not sent: reconnecting", f.m.pendingLabel(7, "approve"))
        f.m.apply(ServerMessage(welcome = Welcome(resume = ResumeKind.UP_TO_DATE)))
        assertTrue(f.m.pending.isEmpty())
    }

    @Test fun readOnlyRecordsNothing() {
        val f = fixture()
        f.m.readOnlyReason = "old protocol"
        f.m.approve(7)
        assertTrue(f.sent.isEmpty())
        assertTrue(f.m.pending.isEmpty())
    }

    @Test fun anOpenedTaskThatIsInTheSidebarHasItsRowBeforeTheTaskView() {
        val f = fixture()
        val r = f.m.sidebarRow(7)
        assertEquals("t", r?.title)
        assertEquals(listOf("approve", "reject"), r?.allowed_actions)
        assertNull(f.m.sidebarRow(99))
    }
}
