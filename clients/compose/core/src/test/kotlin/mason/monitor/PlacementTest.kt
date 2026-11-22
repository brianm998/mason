// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import mason.v1.*
import kotlin.test.*

/** #834: the placement control builds the right command for each level. No clocks. */
class PlacementTest {
    private fun placement(mode: String, source: String) = Placement(mode = mode, source = source, own = true, settable = true)

    private fun fixture(owner: Boolean = true): Pair<MonitorModel, MutableList<Command>> {
        val m = MonitorModel()
        m.connected = true
        m.isOwner = owner
        m.home = HomeView(placement = placement("local", "global"))
        m.header = HeaderView(project_placements = listOf(ProjectPlacement(project = "scratch", placement = placement("favor-remote", "project"))))
        m.taskDetail = TaskDetailView(row = TaskRow(id = 7), placement = placement("remote", "task"))
        val sent = mutableListOf<Command>()
        m.sender = { sent += it }
        return m to sent
    }

    @Test fun eachLevelBuildsItsCommand() {
        val (m, sent) = fixture()
        m.askPlacement(PlacementScope.Global, "remote"); m.confirmPlacement()
        m.askPlacement(PlacementScope.Project("scratch"), "favor-local"); m.confirmPlacement()
        m.askPlacement(PlacementScope.Task(7), "local"); m.confirmPlacement()
        val got = sent.map { it.set_placement!! }.map { listOf(it.scope, it.project, it.task.toString(), it.mode) }
        assertEquals(
            listOf(listOf("global", "", "0", "remote"), listOf("project", "scratch", "0", "favor-local"), listOf("task", "", "7", "local")),
            got,
        )
    }

    @Test fun followSendsDefault() {
        val (m, sent) = fixture()
        assertEquals("Follow MASON", PlacementScope.Project("scratch").followLabel)
        assertEquals("Follow project", PlacementScope.Task(7).followLabel)
        assertNull(PlacementScope.Global.followLabel)
        m.askPlacement(PlacementScope.Project("scratch"), "default"); m.confirmPlacement()
        m.askPlacement(PlacementScope.Task(7), "default"); m.confirmPlacement()
        assertEquals(listOf("default", "default"), sent.map { it.set_placement!!.mode })
    }

    @Test fun nothingIsSentUntilConfirmed() {
        val (m, sent) = fixture()
        m.askPlacement(PlacementScope.Global, "local")
        assertTrue(sent.isEmpty())
        m.cancelPlacement()
        m.confirmPlacement()
        assertTrue(sent.isEmpty())
        assertNull(m.placementAsk)
    }

    @Test fun textNamesTheModeAndTheLevelThatSetIt() {
        val (m, _) = fixture()
        assertEquals("favor-remote, from the project", m.placementText(PlacementScope.Project("scratch")))
        assertEquals("local, from the global switch", m.placementText(PlacementScope.Global))
        assertEquals("remote, from the task", m.placementText(PlacementScope.Task(7)))
    }

    @Test fun onlyAnOwnerDeviceThatCanActGetsTheControl() {
        assertFalse(fixture(owner = false).first.canSetPlacement(PlacementScope.Global))
        val (m, _) = fixture()
        assertTrue(m.canSetPlacement(PlacementScope.Global))
        assertTrue(m.canFollow(PlacementScope.Project("scratch")))
        assertFalse(m.canFollow(PlacementScope.Global))
        m.readOnlyReason = "old monitor"
        assertFalse(m.canSetPlacement(PlacementScope.Task(7)))
    }

    @Test fun pendingLabelShowsForTheLevelThatWasSet() {
        val (m, _) = fixture()
        m.askPlacement(PlacementScope.Project("scratch"), "local"); m.confirmPlacement()
        assertEquals("Changing where runs go…", m.placementPendingLabel(PlacementScope.Project("scratch")))
        assertNull(m.placementPendingLabel(PlacementScope.Global))
    }

    @Test fun rowMarkerOnlyForAnOverride() {
        assertNull(runOnMarker(TaskRow()))
        assertEquals("runs on remote", runOnMarker(TaskRow(run_on_override = "remote")))
    }
}
