// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import mason.v1.ActiveModel
import mason.v1.RemoteRun
import mason.v1.RemoteSummary

/**
 * The text of the home page's "Active models" section: how many agents are working and how many
 * wait (on a check, here or on GitHub, or on a slot), and what is out on GitHub. The Swift
 * monitor has the same functions in `ActiveSummary.swift`, with the same words.
 */
object ActiveSummary {
    /**
     * "7 agents: 2 working, 5 waiting on GitHub checks"; null with no agent (the page then says
     * "Nothing is running."). An entry from a daemon that sends no state counts as working.
     */
    fun header(active: List<ActiveModel>): String? {
        if (active.isEmpty()) return null
        val parts = listOf(
            active.count { it.state.isEmpty() || it.state == "working" } to "working",
            active.count { it.state == "waiting_check" && it.place == "github" } to "waiting on GitHub checks",
            active.count { it.state == "waiting_check" && it.place != "github" } to "waiting on checks here",
            active.count { it.state == "waiting_slot" } to "waiting for a slot",
        )
        val text = parts.filter { it.first > 0 }.joinToString(", ") { "${it.first} ${it.second}" }
        return "${active.size} agent${if (active.size == 1) "" else "s"}: $text"
    }

    /** What one agent is doing, for its row. */
    fun state(a: ActiveModel): String = when (a.state) {
        "waiting_check" -> if (a.place == "github") "waiting on a GitHub check" else "waiting on a check here"
        "waiting_slot" -> "waiting for ${a.resource.ifEmpty { "a slot" }}"
        else -> "working"
    }

    /** "13 of 16 runners: 7 checks, 4 test steps, 2 debts; macOS 2 of 4"; null when nothing is out. */
    fun github(r: RemoteSummary?): String? {
        if (r == null || (r.runs.isEmpty() && r.checks + r.test_steps + r.batches + r.debts == 0)) return null
        fun count(n: Int, one: String, many: String): String? = if (n == 0) null else "$n ${if (n == 1) one else many}"
        val parts = listOfNotNull(
            count(r.checks, "check", "checks"), count(r.test_steps, "test step", "test steps"),
            count(r.batches, "batch", "batches"), count(r.debts, "debt", "debts"),
        ).joinToString(", ")
        val head = r.pools.firstOrNull { it.name == "gha" }?.let { "${it.used} of ${it.limit} runners" } ?: "runners"
        val mac = r.pools.firstOrNull { it.name == "gha-macos" && it.used > 0 }?.let { "; macOS ${it.used} of ${it.limit}" } ?: ""
        return "$head: $parts$mac"
    }

    /** One run of the detail page: "check #760 on ubuntu-latest". */
    fun runLine(r: RemoteRun): String {
        val kind = r.kind.replace('_', ' ')
        val task = if (r.task == 0L) r.label else "#${r.task}"
        return "$kind $task${if (r.runs_on.isEmpty()) "" else " on ${r.runs_on}"}"
    }
}
