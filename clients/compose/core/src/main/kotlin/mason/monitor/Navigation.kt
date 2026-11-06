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

    /** The `mason://` address of a page, for deep links from notifications. */
    val url: String
        get() = when (this) {
            Home -> "mason://home"
            is Task -> "mason://task/$id"
            Landing -> "mason://landing"
            Usage -> "mason://usage"
            Pipeline -> "mason://pipeline"
            Health -> "mason://health"
        }

    companion object {
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
                else -> null
            }
        }
    }
}

/**
 * Browser-style history: back, forward, home. Selecting anything replaces what
 * was shown and clears what could be gone forward to.
 */
data class NavHistory(
    val current: Page = Page.Home,
    val back: List<Page> = emptyList(),
    val forward: List<Page> = emptyList(),
) {
    val canGoBack get() = back.isNotEmpty()
    val canGoForward get() = forward.isNotEmpty()

    fun go(p: Page): NavHistory =
        if (p == current) this else copy(current = p, back = back + current, forward = emptyList())

    fun goBack(): NavHistory =
        if (back.isEmpty()) this else copy(current = back.last(), back = back.dropLast(1), forward = forward + current)

    fun goForward(): NavHistory =
        if (forward.isEmpty()) this else copy(current = forward.last(), forward = forward.dropLast(1), back = back + current)

    /** Home clears the selection, and is itself a step in the history. */
    fun goHome(): NavHistory = go(Page.Home)
}
