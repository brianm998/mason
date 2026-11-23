// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

/** #951: a notification row is one short line; the rest goes behind its "?" button. */
object NoticeText {
    /** The longest one-line summary shown whole, in characters. */
    const val LIMIT = 80

    /** A text as one short line and the rest. `detail` is the whole text when something was cut. */
    data class Lines(val text: String, val detail: String) {
        /** The "?" button shows on a row with more to say than its line. */
        val hasDetails: Boolean get() = detail.isNotEmpty()
    }

    fun oneLine(text: String, detail: String = ""): Lines {
        val flat = text.trim().split(Regex("\\s+")).filter { it.isNotEmpty() }.joinToString(" ")
        if (flat.length <= LIMIT) return Lines(flat, if (detail.isEmpty() && flat != text) text.trim() else detail)
        var cut = flat.take(LIMIT - 1)
        val i = cut.lastIndexOf(' ')
        if (i > LIMIT / 2) cut = cut.substring(0, i)
        cut = cut.trimEnd(' ', ',', ';', ':')
        return Lines("$cut…", detail.ifEmpty { text.trim() })
    }

    /** Every task a text names (`#12`), once each, in order: the "Open task #N" actions. */
    fun taskNumbers(vararg texts: String): List<Long> =
        Regex("#(\\d+)").findAll(texts.joinToString("\n")).mapNotNull { it.groupValues[1].toLongOrNull() }.distinct().toList()
}
