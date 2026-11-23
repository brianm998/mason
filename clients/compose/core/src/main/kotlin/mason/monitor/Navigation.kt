// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import java.net.URI

/** What the main view shows: exactly one of these at a time. */
sealed class Page {
    data object Home : Page()
    data class Task(val id: Long) : Page()
    data object Landing : Page()
    data object Usage : Page()
    data object Pipeline : Page()
    data object Health : Page()
    data object Settings : Page()
    /** A detail page for one section of home: see [sectionNames]. */
    data class Section(val name: String) : Page()

    /** The `mason://` address of a page, for deep links from notifications. */
    val url: String
        get() = when (this) {
            Home -> "mason://home"
            is Task -> "mason://task/$id"
            Landing -> "mason://landing"
            Usage -> "mason://usage"
            Pipeline -> "mason://pipeline"
            Health -> "mason://health"
            Settings -> "mason://settings"
            is Section -> "mason://section/$name"
        }

    companion object {
        /** The sections that have a page of their own (`mason://section/<name>`). */
        val sectionNames = listOf("active_models", "landing", "queued", "cpu", "disk", "github", "compilation",
            "exploring", "developing", "testing", "review", "needs_attention", "proposed", "failed")

        /** The page for a section of home; `landing` is the existing Landing page. */
        fun sectionPage(name: String): Page? = when {
            name !in sectionNames -> null
            name == "landing" -> Landing
            else -> Section(name)
        }

        /** `mason://task/12`, `mason://landing`, ... `null` for anything else. */
        fun from(url: String): Page? {
            val u = runCatching { URI(url) }.getOrNull() ?: return null
            if (u.scheme != "mason") return null
            val host = u.host ?: return null
            val rest = (u.path ?: "").split('/').filter { it.isNotEmpty() }
            return when (host) {
                "home" -> if (rest.isEmpty()) Home else null
                "task" -> rest.singleOrNull()?.toLongOrNull()?.takeIf { it >= 0 }?.let(::Task)
                "landing" -> if (rest.isEmpty()) Landing else null
                "usage" -> if (rest.isEmpty()) Usage else null
                "pipeline" -> if (rest.isEmpty()) Pipeline else null
                "health" -> if (rest.isEmpty()) Health else null
                "settings" -> if (rest.isEmpty()) Settings else null
                "section" -> rest.singleOrNull()?.let(::sectionPage)
                else -> null
            }
        }
    }
}

/**
 * Browser-style history: back, forward, home. Selecting anything replaces what
 * was shown and clears what could be gone forward to. Each entry carries the
 * vertical scroll position it was left at, so back and forward put a page where
 * it was (the same page twice in the stack keeps two positions).
 */
data class NavHistory(
    val current: Page = Page.Home,
    val back: List<Page> = emptyList(),
    val forward: List<Page> = emptyList(),
    /** Saved positions, in step with [back] and [forward]. */
    val backScroll: List<Int> = emptyList(),
    val forwardScroll: List<Int> = emptyList(),
    /** Where the current entry should open: what was saved when it was left, 0 after [go]. */
    val restore: Int = 0,
    /** Counts arrivals, so a view can tell one visit from the next. */
    val visit: Int = 0,
) {
    val canGoBack get() = back.isNotEmpty()
    val canGoForward get() = forward.isNotEmpty()

    /** [scroll] is where the current page is scrolled now; it is saved on its entry as it is left. */
    fun go(p: Page, scroll: Int = 0): NavHistory =
        if (p == current) this else copy(
            current = p, back = back + current, backScroll = backScroll + scroll,
            forward = emptyList(), forwardScroll = emptyList(), restore = 0, visit = visit + 1,
        )

    fun goBack(scroll: Int = 0): NavHistory =
        if (back.isEmpty()) this else copy(
            current = back.last(), back = back.dropLast(1), restore = backScroll.last(),
            backScroll = backScroll.dropLast(1), forward = forward + current,
            forwardScroll = forwardScroll + scroll, visit = visit + 1,
        )

    fun goForward(scroll: Int = 0): NavHistory =
        if (forward.isEmpty()) this else copy(
            current = forward.last(), forward = forward.dropLast(1), restore = forwardScroll.last(),
            forwardScroll = forwardScroll.dropLast(1), back = back + current,
            backScroll = backScroll + scroll, visit = visit + 1,
        )

    /** Home clears the selection, and is itself a step in the history. */
    fun goHome(scroll: Int = 0): NavHistory = go(Page.Home, scroll)
}
