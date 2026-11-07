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

enum class OtherKey { SEND, NEWLINE }

/** What Return does in the "Other" box. */
fun otherBoxKey(shift: Boolean): OtherKey = if (shift) OtherKey.NEWLINE else OtherKey.SEND

/** The text the Other box sends: trimmed, and nothing when it is blank. */
fun otherAnswer(text: String): String? = text.trim().ifEmpty { null }
