// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import mason.v1.TaskRow
import mason.v1.WaitInfo
import mason.v1.WaitParty

/**
 * What a task is waiting on, as the daemon sends it (s527): the cause as a badge, the blocker
 * named, the chain behind it, and the leases that belong to no task. The same wording as the
 * Swift core (`QueueWait.swift`).
 */
object QueueWait {
    /** The small badge on a waiting row. */
    fun badge(cause: String): String = when (cause) {
        "full" -> "slot full"
        "behind" -> "in line"
        "behind-exclusive" -> "behind exclusive"
        "exclusive" -> "machine busy"
        "cpu" -> "CPU"
        "settling" -> "settling"
        "disk-stall" -> "build disk slow"
        "syspolicyd" -> "syspolicyd"
        "memory" -> "memory low"
        "disk-floor" -> "disk low"
        "too-big" -> "too big"
        "unknown" -> "unknown resource"
        else -> cause
    }

    /** A party in words: "#510", or "`starpipe heavy` (starpipe in /x/star)" for a lease. */
    fun party(p: WaitParty): String {
        if (p.task > 0) return "#${p.task}"
        val origin = if (p.origin.isEmpty()) "" else " (${p.origin})"
        return "`${p.label}`$origin"
    }

    /** Who the task is behind: "behind an exclusive request `starpipe heavy`", "waiting for #510's compile slot", or "". */
    fun blocker(w: WaitInfo): String {
        val b = w.blocker
        if (b != null) {
            val what = if (b.exclusive) "an exclusive request " else ""
            return "behind $what${if (b.task > 0) "#${b.task}" else "`${b.label}`"}"
        }
        val h = w.holders.firstOrNull() ?: return ""
        val more = if (w.holders.size > 1) " and ${w.holders.size - 1} more" else ""
        return if (h.task > 0) "waiting for #${h.task}'s ${w.resource} slot$more" else "waiting for `${h.label}`$more"
    }

    /** The task page's chain: this task waits for X; X waits for Y. */
    fun chain(w: WaitInfo, task: String): List<String> {
        val lines = mutableListOf<String>()
        w.chain.forEachIndexed { i, link ->
            val party = link.party ?: return@forEachIndexed
            val p = party(party)
            if (i == 0) lines += "$task waits for $p"
            lines += if (link.sentence.isEmpty()) "$p is ${if (party.holding) "running" else "starting"}" else "$p: ${link.sentence}"
        }
        return lines
    }

    /** What follows a lease's label: "from starpipe in /x/star, wants exclusive, waiting 9 min". */
    fun leaseDetail(p: WaitParty, now: Long): String {
        val age = Format.duration(maxOf(0L, now - p.since_ms))
        val parts = mutableListOf<String>()
        if (p.origin.isNotEmpty()) parts += "from ${p.origin}"
        if (p.needs.isNotEmpty()) parts += "${if (p.holding) "holds" else "wants"} ${p.needs}"
        parts += if (p.holding) "for $age" else "waiting $age"
        return parts.joinToString(", ")
    }

    /** One lease that belongs to no task, on the banner. */
    fun lease(p: WaitParty, now: Long): String = "${p.label}, ${leaseDetail(p, now)}"

    /** A row's second line: "[badge] sentence" while it waits on the scheduler, else its activity. */
    fun line(r: TaskRow): String {
        val base = lineWithoutDiagnosis(r)
        val d = diagnosisBadge(r.diagnosis) ?: return base
        return if (base.isEmpty()) d else "$base · $d"
    }

    private fun lineWithoutDiagnosis(r: TaskRow): String {
        val w = r.wait
        return if (w != null && r.why_waiting.isNotEmpty()) "[${badge(w.cause)}] ${r.why_waiting}" else r.why_waiting.ifEmpty { r.activity }
    }
}
