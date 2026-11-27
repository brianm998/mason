// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import mason.v1.Command
import mason.v1.RunnerPool
import mason.v1.SetLimit

/**
 * The GitHub runner limit control (#1261): the words and the bounds for a `gha` or `gha-macos`
 * pool, the same rules as the Mac monitor. `gha_ceiling`, `gha-debt` and `gha_full_share` stay
 * read-only (CLI only).
 */
object RunnerLimit {
    val POOLS = listOf("gha", "gha-macos")

    fun title(name: String) = if (name == "gha-macos") "GitHub macOS runners" else "GitHub runners"

    /** The limit after a step of [delta], kept between 0 and the account ceiling; null when that is the limit already. */
    fun stepped(p: RunnerPool, delta: Int): Int? {
        val next = maxOf(0, minOf(p.ceiling, p.limit + delta))
        return if (next == p.limit) null else next
    }

    fun canDecrease(p: RunnerPool) = p.limit > 0
    fun canIncrease(p: RunnerPool) = p.limit < p.ceiling

    /** What the limit allows: the smaller of the limit and the ceiling. */
    fun effective(p: RunnerPool) = minOf(p.limit, p.ceiling)

    fun head(p: RunnerPool) = "${title(p.name)}: ${p.used} in use of at most"

    /** "MASON 9, other workflows 4; account ceiling 20; 2 waiting", and the real limit when the ceiling caps it. */
    fun detail(p: RunnerPool): String {
        val parts = mutableListOf("MASON ${p.mason}, other workflows ${p.other}", "account ceiling ${p.ceiling}")
        if (p.limit > p.ceiling) parts += "the ceiling caps the limit to ${p.ceiling}"
        parts += if (p.waiting == 0) "none waiting" else "${p.waiting} waiting"
        return parts.joinToString("; ")
    }

    /** The standing notes: at 0 nothing goes to GitHub; lowering stops new grants only. */
    fun notes(p: RunnerPool): List<String> = buildList {
        if (effective(p) == 0) add(if (p.name == "gha-macos") "At 0 no macOS job goes to GitHub." else "At 0 no check goes to GitHub.")
        add("Lowering the limit stops new grants only; jobs already running finish.")
    }
}

/** The runner pools the daemon sent (`gha`, `gha-macos`); empty from an older daemon. */
val MonitorModel.runnerPools: List<RunnerPool>
    get() {
        val sent = home?.runner_pools ?: emptyList()
        return RunnerLimit.POOLS.mapNotNull { n -> sent.firstOrNull { it.name == n } }
    }

/** Set a runner pool's limit (the daemon says if it refuses; the reason shows in `lastResult`). */
fun MonitorModel.setRunnerLimit(name: String, units: Int) =
    send(Command(set_limit = SetLimit(resource = name, units = units)))

/** The minus (-1) and plus (+1) of a runner pool: between 0 and the account ceiling; at a bound nothing is sent. */
fun MonitorModel.stepRunnerLimit(name: String, delta: Int) {
    val p = runnerPools.firstOrNull { it.name == name } ?: return
    val next = RunnerLimit.stepped(p, delta) ?: return
    setRunnerLimit(name, next)
}
