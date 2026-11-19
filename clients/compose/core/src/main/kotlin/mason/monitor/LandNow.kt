// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import mason.v1.BatchLine
import mason.v1.TaskLanding

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
