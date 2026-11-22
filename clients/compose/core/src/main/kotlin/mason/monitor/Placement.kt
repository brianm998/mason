// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import mason.v1.Command
import mason.v1.Placement
import mason.v1.SetPlacement
import mason.v1.TaskRow

/** Where runs go can be set at three levels (#834): all of MASON, one project, one task. */
sealed class PlacementScope {
    object Global : PlacementScope()
    data class Project(val name: String) : PlacementScope()
    data class Task(val id: Long) : PlacementScope()

    /** "global", "project" or "task": the `SetPlacement.scope` the daemon reads. */
    val wire: String
        get() = when (this) {
            Global -> "global"
            is Project -> "project"
            is Task -> "task"
        }

    /** What clearing the choice is called; null where there is nothing above to follow. */
    val followLabel: String?
        get() = when (this) {
            Global -> null
            is Project -> "Follow MASON"
            is Task -> "Follow project"
        }

    internal val noun: String
        get() = when (this) {
            Global -> "all of MASON"
            is Project -> "project $name"
            is Task -> "#$id"
        }
}

/** The four modes, in the order a control lists them. */
val placementModes = listOf("local", "remote", "favor-local", "favor-remote")

/** The mode the owner picked, or "default" (follow the level above). */
data class PlacementAsk(val scope: PlacementScope, val mode: String) {
    /** The one-line confirm shown before anything is sent. */
    val confirmText: String
        get() = if (mode == "default") {
            "Make ${scope.noun} ${scope.followLabel?.lowercase() ?: "follow its default"}? Runs not yet started move; a run in progress is not moved."
        } else {
            "Run ${scope.noun} on $mode? Runs not yet started move; a run in progress is not moved."
        }

    /** The command the confirm sends. */
    val command: Command
        get() = Command(
            set_placement = SetPlacement(
                scope = scope.wire,
                project = (scope as? PlacementScope.Project)?.name ?: "",
                task = (scope as? PlacementScope.Task)?.id ?: 0L,
                mode = mode,
            ),
        )

    /** Where the pending command is kept (see [placementKey]) and under what action. */
    val pendingKey: Pair<Long, String> get() = placementKey(scope) to "set_placement"
}

/** A project's or MASON's place among the task ids a pending command is kept under: a task id stays itself; the others are lifted clear of every task's (and a batch's). */
fun placementKey(s: PlacementScope): Long = when (s) {
    is PlacementScope.Task -> s.id
    PlacementScope.Global -> 1L shl 61
    is PlacementScope.Project -> {
        var h = 1469598103934665603UL
        for (b in s.name.encodeToByteArray()) h = (h xor (b.toULong() and 0xFFUL)) * 1099511628211UL
        (1L shl 61) or (h and 0xFFFFFFFFUL).toLong() or (1L shl 32)
    }
}

/** The placement the daemon last sent for this level; null before a view has it. */
fun MonitorModel.placement(s: PlacementScope): Placement? = when (s) {
    PlacementScope.Global -> home?.placement
    is PlacementScope.Project -> header?.project_placements?.firstOrNull { it.project == s.name }?.placement
    is PlacementScope.Task -> taskDetail?.takeIf { it.row?.id == s.id }?.placement
}

/** "favor-remote, from the project": the mode in force and the level that set it. */
fun placementText(p: Placement): String = "${p.mode}, from the ${if (p.source == "global") "global switch" else p.source}"

fun MonitorModel.placementText(s: PlacementScope): String? = placement(s)?.let { placementText(it) }

/** The control is the owner's, on a device that can act, where the daemon sent a placement and the level can be set. */
fun MonitorModel.canSetPlacement(s: PlacementScope): Boolean = canAct && placement(s)?.settable == true

/** "Follow MASON" / "Follow project" is offered only while the level has a choice of its own. */
fun MonitorModel.canFollow(s: PlacementScope): Boolean = canSetPlacement(s) && s.followLabel != null && placement(s)?.own == true

/** What the control says while its command is pending; null when none is. */
fun MonitorModel.placementPendingLabel(s: PlacementScope): String? = pendingLabel(placementKey(s), "set_placement")

/** A mode (or "default") was chosen: ask first. */
fun MonitorModel.askPlacement(s: PlacementScope, mode: String) { placementAsk = PlacementAsk(s, mode) }

fun MonitorModel.cancelPlacement() { placementAsk = null }

/** "Change" in the confirm: send the command for what was asked. */
fun MonitorModel.confirmPlacement() {
    val a = placementAsk ?: return
    placementAsk = null
    send(a.command)
}

/** The task row's small marker: the task overrides its project's mode. */
fun runOnMarker(r: TaskRow): String? = r.run_on_override.takeIf { it.isNotEmpty() }?.let { "runs on $it" }
