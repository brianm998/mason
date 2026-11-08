// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

/**
 * The phone layout (Part 17, narrow screens), with nothing drawn. The sidebar is the first
 * screen: count tiles scroll sideways above a Home row, then the state sections and the other
 * pages. Choosing a row pushes its detail; Back pops. Same rules as `PhoneLayout.swift`.
 */
enum class PhoneBlock { Tiles, Home, Sections, Landing, Usage }

sealed class PhoneScreen {
    object Sidebar : PhoneScreen()
    data class Detail(val page: Page) : PhoneScreen()
}

object PhoneLayout {
    val sidebarOrder = listOf(PhoneBlock.Tiles, PhoneBlock.Home, PhoneBlock.Sections, PhoneBlock.Landing, PhoneBlock.Usage)
    val firstScreen: PhoneScreen = PhoneScreen.Sidebar

    /** Not on a phone: deliberate differences recorded in `clients/features.toml`. */
    val phoneHasNo = listOf("graph editing", "log deep-dives", "agent sessions")
}

data class PhoneNav(val stack: List<Page> = emptyList()) {
    val screen: PhoneScreen get() = stack.lastOrNull()?.let { PhoneScreen.Detail(it) } ?: PhoneScreen.Sidebar

    fun select(p: Page) = if (stack.lastOrNull() == p) this else copy(stack = stack + p)
    fun back() = copy(stack = stack.dropLast(1))
}
