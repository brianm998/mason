// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

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
