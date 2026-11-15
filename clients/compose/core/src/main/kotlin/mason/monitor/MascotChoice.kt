// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import mason.v1.StateCount

/**
 * Which mascot picture a screen shows, chosen only from the daemon's own state. The ids are
 * names in clients/mascot.toml; the views load them with `Mascot.illustration`, which gives
 * null for an id it does not know.
 */
data class MascotChoice(val id: String, val large: Boolean)

/** The text shown with the picture when a sidebar search matches nothing. */
const val SEARCH_NO_MATCH_TEXT = "Nothing matched."

/** The Home status picture. Slow and quiet tasks do not change it (slow is not an alarm). */
val MonitorModel.homeMascot: MascotChoice
    get() {
        if (!connected) return MascotChoice("not_connected", false)
        if (attention > 0) return MascotChoice("needs_attention", false)
        val h = home
        if (h != null && h.active.isNotEmpty()) return MascotChoice("developing", false)
        if (h != null && h.stages.all { it.rows.isEmpty() } && h.attention.isEmpty()) return MascotChoice("no_tasks", true)
        return MascotChoice("all_clear", false)
    }

/** True when a search is on and no section has a matching row. */
val MonitorModel.searchMatchesNothing: Boolean
    get() = search.isActive && sidebar?.sections.orEmpty().all { filteredRows(it).isEmpty() }

/** The active states, earliest stage first; a tie for the most tasks goes to the later one. */
val ACTIVE_STATES_IN_ORDER = listOf(
    "queued", "exploring", "developing", "testing", "review", "landing", "needs_attention", "failed", "proposed",
)

/**
 * The active state (history is left out) holding the most tasks, from the header's own counts,
 * so it follows the project filter; null with no tasks or while disconnected.
 */
val MonitorModel.busiestState: Pair<String, Int>?
    get() {
        if (!connected) return null
        val counts = header?.counts ?: return null
        return counts.filter { it.count > 0 && it.state in ACTIVE_STATES_IN_ORDER }
            .maxWithOrNull(compareBy<StateCount>({ it.count }, { ACTIVE_STATES_IN_ORDER.indexOf(it.state) }))
            ?.let { it.state to it.count }
    }

/** The top bar's picture: the busiest state's illustration, the idle art with no tasks. */
val MonitorModel.topBarMascot: MascotChoice?
    get() {
        if (!connected || header == null) return null
        return MascotChoice(busiestState?.first ?: "all_clear", false)
    }

/** "Most tasks: testing (7)"; "No active tasks" for the idle art. */
val MonitorModel.topBarMascotTooltip: String
    get() {
        val b = busiestState ?: return "No active tasks"
        val label = header?.counts?.firstOrNull { it.state == b.first }?.let { tileLabel(it) } ?: b.first
        return "Most tasks: $label (${b.second})"
    }

/** Clicking the top bar picture reveals that state's sidebar section. */
fun MonitorModel.topBarMascotClicked() {
    busiestState?.let { reveal(it.first) }
}
