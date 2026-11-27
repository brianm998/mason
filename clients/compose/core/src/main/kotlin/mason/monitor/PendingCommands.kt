// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import mason.v1.Command
import mason.v1.CommandResult
import mason.v1.TaskRow

/** How far a pending command has got. */
enum class PendingStage {
    /** Written (or about to be); no answer yet. */
    SENDING,
    /** masond accepted it; waiting for a view in which the task has changed. */
    ACCEPTED,
    /** The connection went away before an answer. */
    NOT_SENT,
}

/**
 * A command the owner sent that masond has not finished with (task #522). The model records it
 * before the command is written, so the control that was pressed redraws on the same frame.
 */
data class PendingCommand(
    val id: String,
    /** The row's action name: "approve", "retry", "cancel", "land_now", ... */
    val action: String,
    /** The task it is about. */
    val task: Long,
    val sentMs: Long,
    val stage: PendingStage = PendingStage.SENDING,
    /** The task's state and allowed actions when the command was sent. */
    val baseline: String = "",
    val viewsSeen: Int = 0,
) {
    /** What the control says while this is pending. */
    val label: String
        get() = when (stage) {
            PendingStage.SENDING -> verbs(action).first
            PendingStage.ACCEPTED -> verbs(action).second + ", waiting for the pipeline…"
            PendingStage.NOT_SENT -> "Not sent: reconnecting"
        }

    companion object {
        /** How many later sidebar or task views are waited for before the action is offered again. */
        const val VIEWS_TO_WAIT = 3

        /** A batch's place among the task ids a pending command is kept under: batch and task ids are both small numbers, so a batch's is lifted clear of every task's. */
        fun batchKey(batch: Long): Long = (1L shl 62) or batch

        fun verbs(action: String): Pair<String, String> = when (action) {
            "approve" -> "Approving…" to "Approved"
            "reject" -> "Rejecting…" to "Rejected"
            "retry" -> "Retrying…" to "Retried"
            "cancel" -> "Cancelling…" to "Cancelled"
            "defer" -> "Deferring…" to "Deferred"
            "undefer" -> "Restoring…" to "Restored"
            "revive" -> "Reviving…" to "Revived"
            "explore_anyway" -> "Exploring…" to "Asked to explore"
            "land_now", "land_now_batch" -> "Landing…" to "Landing requested"
            "force" -> "Forcing…" to "Forced"
            "skip_test" -> "Skipping…" to "Skipped"
            "skip_all_tests" -> "Skipping all testing…" to "Immune to testing"
            "answer" -> "Sending answer…" to "Answer sent"
            "diagnose" -> "Asking…" to "Diagnosis started"
            "block" -> "Adding blocker…" to "Blocker added"
            "unblock" -> "Releasing…" to "Released"
            "pause" -> "Pausing…" to "Paused"
            "resume" -> "Resuming…" to "Resumed"
            "set_placement" -> "Changing where runs go…" to "Changed"
            "set_limit" -> "Saving…" to "Saved"
            "add_task" -> "Filing…" to "Filed"
            else -> action.replace('_', ' ').let { "$it…" to it }
        }

        /** The action and task a command is about; null for one that is not about one task (pause, limits, add task, raw), which is not tracked. */
        fun describe(c: Command): Pair<String, Long>? = when {
            c.cancel != null -> "cancel" to c.cancel!!.task
            c.retry != null -> "retry" to c.retry!!.task
            c.approve != null -> "approve" to c.approve!!.task
            c.approve_hold != null -> "approve" to c.approve_hold!!.task
            c.reject != null -> "reject" to c.reject!!.task
            c.explore_anyway != null -> "explore_anyway" to c.explore_anyway!!.task
            c.answer != null -> "answer" to c.answer!!.task
            c.diagnose != null -> "diagnose" to c.diagnose!!.task
            c.skip != null -> (if (c.skip!!.all_tests == true) "skip_all_tests" else "skip_test") to c.skip!!.task
            c.land_now != null -> "land_now" to c.land_now!!.task
            c.land_now_batch != null -> "land_now_batch" to batchKey(c.land_now_batch!!.batch)
            c.force != null -> "force" to c.force!!.task
            c.unblock != null -> "unblock" to c.unblock!!.task
            c.block != null -> "block" to c.block!!.task
            c.revive != null -> "revive" to c.revive!!.task
            c.defer != null -> "defer" to c.defer!!.task
            c.undefer != null -> "undefer" to c.undefer!!.task
            c.set_placement != null -> "set_placement" to c.set_placement!!.let { p ->
                placementKey(when (p.scope) { "task" -> PlacementScope.Task(p.task); "project" -> PlacementScope.Project(p.project); else -> PlacementScope.Global })
            }
            else -> null
        }
    }
}

/** The pending command for this action on this task, if any. */
fun MonitorModel.pendingCommand(task: Long, action: String): PendingCommand? =
    pending.values.firstOrNull { it.task == task && it.action == action }

/** What a control for this action should say instead of its name; null when nothing is pending. */
fun MonitorModel.pendingLabel(task: Long, action: String): String? = pendingCommand(task, action)?.label

/** True while any command about this task is pending: its other controls are disabled too. */
fun MonitorModel.isBusy(task: Long): Boolean = pending.values.any { it.task == task }

/** The daemon's refusal of the last command about this task, shown beside its controls. */
fun MonitorModel.actionError(task: Long): String? = actionErrors[task]

/** The sidebar's row for a task, so its page can be drawn before masond sends the task view. */
fun MonitorModel.sidebarRow(task: Long): TaskRow? =
    sidebar?.sections?.firstNotNullOfOrNull { s -> s.rows.firstOrNull { it.id == task } }

internal fun MonitorModel.signature(task: Long): String {
    val r = sidebarRow(task) ?: taskDetail?.row?.takeIf { it.id == task } ?: return ""
    return r.state + "|" + r.allowed_actions.joinToString(",")
}

/** Records the command as pending and returns it with its id; null when one for the same control is in flight. */
internal fun MonitorModel.beginPending(c: Command): Command? {
    if (c.id.isNotEmpty()) return c
    val (action, task) = PendingCommand.describe(c) ?: return c
    if (pending.values.any { it.task == task }) return null
    nextActionId += 1
    val id = "act:$action:$task:$nextActionId"
    actionErrors = actionErrors - task
    pending = pending + (id to PendingCommand(id, action, task, System.currentTimeMillis(), baseline = signature(task)))
    return c.copy(id = id)
}

internal fun MonitorModel.settle(r: CommandResult) {
    val p = pending[r.id] ?: return
    pending = when {
        !r.ok -> {
            actionErrors = actionErrors + (p.task to r.error)
            pending - r.id
        }
        p.task != 0L && signature(p.task) != p.baseline -> pending - r.id
        else -> pending + (r.id to p.copy(stage = PendingStage.ACCEPTED))
    }
}

/** A sidebar or task view arrived: an accepted command ends when its task changed, or after VIEWS_TO_WAIT views that did not change it. */
internal fun MonitorModel.settleAfterView() {
    var next = pending
    for ((id, p) in pending) {
        if (p.stage != PendingStage.ACCEPTED) continue
        val seen = p.viewsSeen + 1
        next = if ((p.task != 0L && signature(p.task) != p.baseline) || seen >= PendingCommand.VIEWS_TO_WAIT) next - id
        else next + (id to p.copy(viewsSeen = seen))
    }
    pending = next
}

internal fun MonitorModel.markPendingNotSent() {
    pending = pending.mapValues { (_, p) -> if (p.stage == PendingStage.SENDING) p.copy(stage = PendingStage.NOT_SENT) else p }
}
