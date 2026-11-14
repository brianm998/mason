// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

/**
 * How the decision card is driven from the keyboard, as plain logic so it is tested
 * without a window: the arrow keys move a selection over the options, Return picks the
 * selected one, and the "Other" box sends on Enter and takes a new line on Shift-Enter.
 * (The same rules as the SwiftUI client.)
 */
data class OptionSelection(val count: Int, val index: Int? = null) {
    fun down(): OptionSelection =
        if (count == 0) this else copy(index = index?.let { minOf(it + 1, count - 1) } ?: 0)

    fun up(): OptionSelection =
        if (count == 0) this else copy(index = index?.let { maxOf(it - 1, 0) } ?: (count - 1))

    /** The answer Return sends: the highlighted option's number (1-based), as the command line takes it. */
    val answer: String? get() = index?.let { (it + 1).toString() }

    companion object {
        fun starting(count: Int, recommended: Int?): OptionSelection =
            OptionSelection(count, recommended?.takeIf { it in 0 until count })
    }
}

/**
 * The keyboard rules for a multi-select card: arrows move the highlight, Space toggles the
 * highlighted option, Return sends the checked options as the command line takes them
 * ("1,3", in card order). (The same rules as the SwiftUI client.)
 */
data class MultiSelection(val count: Int, val index: Int? = null, val checked: Set<Int> = emptySet()) {
    fun down(): MultiSelection =
        if (count == 0) this else copy(index = index?.let { minOf(it + 1, count - 1) } ?: 0)

    fun up(): MultiSelection =
        if (count == 0) this else copy(index = index?.let { maxOf(it - 1, 0) } ?: (count - 1))

    /** Space: toggle the highlighted option. */
    fun toggle(): MultiSelection = index?.let { toggle(it) } ?: this

    /** A click on a checkbox: toggle that option and highlight it. */
    fun toggle(i: Int): MultiSelection =
        if (i !in 0 until count) this
        else copy(index = i, checked = if (i in checked) checked - i else checked + i)

    /** The answer Return sends: the checked options' numbers (1-based), sorted; null when none are checked. */
    val answer: String? get() = checked.sorted().map { it + 1 }.joinToString(",").ifEmpty { null }

    companion object {
        fun starting(count: Int, recommended: Int?): MultiSelection =
            MultiSelection(count, recommended?.takeIf { it in 0 until count })
    }
}

enum class OtherKey { SEND, NEWLINE }

/** What Return does in the "Other" box. */
fun otherBoxKey(shift: Boolean): OtherKey = if (shift) OtherKey.NEWLINE else OtherKey.SEND

/** The text the Other box sends: trimmed, and nothing when it is blank. */
fun otherAnswer(text: String): String? = text.trim().ifEmpty { null }
