// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import mason.v1.BatchLine
import mason.v1.TaskLanding
import mason.v1.TaskRow

/** What a Land now button on the landing queue is about: one task, or a whole running batch. */
sealed class LandNowTarget {
    data class Task(val id: Long) : LandNowTarget()
    data class Batch(val id: Long) : LandNowTarget()

    /** The one-line confirm shown before anything is sent. */
    val confirmText: String
        get() = when (this) {
            is Task -> "Land #$id now? Its remaining tests run after the merge; a red result raises an alert."
            is Batch -> "Land batch #$id now? Its remaining tests run after the merge; a red result raises an alert."
        }

    /** Where the pending command is kept (see [PendingCommand.batchKey]) and under what action. */
    val pendingKey: Pair<Long, String>
        get() = when (this) {
            is Task -> id to "land_now"
            is Batch -> PendingCommand.batchKey(id) to "land_now_batch"
        }
}

/** Land now is the owner's, on a device that can act, and only where the daemon offers it. */
fun MonitorModel.canLandNow(i: TaskLanding): Boolean = canAct && "land_now" in i.allowed_actions

fun MonitorModel.canLandNow(b: BatchLine): Boolean = canAct && b.running && "land_now" in b.allowed_actions

/** The running batches that offer Land now. */
fun landNowBatches(batches: List<BatchLine>): List<BatchLine> = batches.filter { it.running && "land_now" in it.allowed_actions }

/** What the button says while its command is pending; null when none is. */
fun MonitorModel.landNowLabel(t: LandNowTarget): String? = t.pendingKey.let { (task, action) -> pendingLabel(task, action) }

/** The button was pressed: ask first. */
fun MonitorModel.askLandNow(t: LandNowTarget) { landNowAsk = t }

fun MonitorModel.cancelLandNow() { landNowAsk = null }

/** "Land now" in the confirm: send the command for what was asked. */
fun MonitorModel.confirmLandNow() {
    val t = landNowAsk ?: return
    landNowAsk = null
    when (t) {
        is LandNowTarget.Task -> landNow(t.id)
        is LandNowTarget.Batch -> landNowBatch(t.id)
    }
}

/** Why Land now for all cannot be used, or null when it can: the same rule as Skip testing. */
val MonitorModel.landDisabledReason: String?
    get() = readOnlyReason?.let { "Read-only: $it" } ?: if (!isOwner) "Only the owner can land tasks now" else null

/** The rows of a landing page that offer Land now. */
fun MonitorModel.landableRows(rows: List<TaskRow>): List<TaskRow> = rows.filter { "land_now" in it.allowed_actions }

/**
 * Land now for every row that allows it. Land now on one member lands its whole running batch, so
 * one `land_now_batch` goes out per running batch holding a landable row, and one `land_now` for
 * each landable task in none of those batches.
 */
fun MonitorModel.landAllNow(rows: List<TaskRow>) {
    landDisabledReason?.let { landNotice = it; return }
    val targets = landableRows(rows)
    if (targets.isEmpty()) {
        landNotice = "Nothing to land: no task here allows it"
        return
    }
    val running = landNowBatches(landing?.batches.orEmpty())
    val batches = mutableListOf<Long>()
    val singles = mutableListOf<Long>()
    for (r in targets) {
        val b = running.firstOrNull { r.id in it.tasks }
        if (b == null) singles += r.id else if (b.id !in batches) batches += b.id
    }
    batches.forEach { landNowBatch(it) }
    singles.forEach { landNow(it) }
    val n = targets.size
    landNotice = "Land now sent for $n task${if (n == 1) "" else "s"} (${batches.size} batch${if (batches.size == 1) "" else "es"})"
}
