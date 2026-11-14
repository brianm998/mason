// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import mason.v1.SidebarSection
import mason.v1.TaskRow

/**
 * One thing in the sidebar's scrolling part. Home and search are not here: they are pinned
 * above the scrolling part and never scroll.
 */
sealed class SidebarEntry {
    data class Header(val section: SidebarSection) : SidebarEntry()
    data class Row(val row: TaskRow) : SidebarEntry()
    data object Rule : SidebarEntry()
    data class PageLink(val title: String, val page: Page) : SidebarEntry()

    /** What the entry is, not where it is: a lazy list keyed by it keeps a row when a task above moves. */
    val key: String
        get() = when (this) {
            is Header -> "h:${section.state}"
            is Row -> "r:${row.id}"
            Rule -> "rule"
            is PageLink -> "p:$title"
        }
}

/** The pinned area's test tags in reading order (the search toggles follow `search`). */
val SIDEBAR_PINNED_IDS = listOf("sidebar:home", "search")

/** The scrolling part, in reading order: sections with their rows, then the page links. */
fun MonitorModel.sidebarEntries(): List<SidebarEntry> = buildList {
    sidebar?.sections?.forEach { sec ->
        add(SidebarEntry.Header(sec))
        if (isExpanded(sec)) filteredRows(sec).forEach { add(SidebarEntry.Row(it)) }
    }
    add(SidebarEntry.Rule)
    add(SidebarEntry.PageLink("Landing", Page.Landing))
    add(SidebarEntry.PageLink("LLM usage", Page.Usage))
    add(SidebarEntry.PageLink("Pipeline", Page.Pipeline))
    add(SidebarEntry.PageLink("Health", Page.Health))
}

/** Where a Home tile's section is in the scrolling part, or -1 when it is not there. */
fun MonitorModel.scrollIndex(state: String): Int =
    sidebarEntries().indexOfFirst { it is SidebarEntry.Header && it.section.state == state }
