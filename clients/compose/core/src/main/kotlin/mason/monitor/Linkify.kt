// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import mason.v1.CiLink

/** One run of text from [Linkify.segments]. */
sealed class LinkSegment {
    abstract val text: String
    /** Plain text. */
    data class Plain(override val text: String) : LinkSegment()
    /** A reference to a task the monitor knows: open its page. */
    data class Task(val id: Long, override val text: String) : LinkSegment()
    /** A page to open in the default browser. */
    data class Url(val url: String, override val text: String) : LinkSegment()
}

/**
 * Turns a string into plain text, task links and URL links. Pure, so tests cover it.
 *
 * - `#N` is a task link only when `isKnownTask(N)` and the text before it does not make it
 *   something else: `PR #N`, `pull request #N`, `Batch #N`, `debt #N`, `issue #N`, and the
 *   `(#N)` of "the pull request's checks (#N)" are not tasks.
 * - A [CiLink]'s label (`PR #7`, a failed check's name) becomes a URL link when its URL is
 *   http(s); the label of a link with another kind of URL stays plain text.
 * - A bare `http(s)://` URL becomes a URL link.
 */
object Linkify {
    /** Words that, right before a `#N`, say it is not a task. */
    private val notTask = listOf("pr", "pull request", "batch", "debt", "issue", "run")

    fun isWebUrl(s: String): Boolean {
        val l = s.lowercase()
        return (l.startsWith("https://") || l.startsWith("http://")) && l.length > 8 && s.none { it.isWhitespace() }
    }

    private fun isWord(c: Char) = c.isLetterOrDigit() || c == '_'
    private fun isAsciiDigit(c: Char) = c in '0'..'9'

    fun segments(text: String, isKnownTask: (Long) -> Boolean, ciLinks: List<CiLink> = emptyList()): List<LinkSegment> {
        val labels = ciLinks.filter { it.label.isNotEmpty() && isWebUrl(it.url) }.sortedByDescending { it.label.length }
        val out = mutableListOf<LinkSegment>()
        val plain = StringBuilder()
        fun flush() { if (plain.isNotEmpty()) { out += LinkSegment.Plain(plain.toString()); plain.clear() } }
        var i = 0
        scan@ while (i < text.length) {
            val c = text[i]
            val prevWord = i > 0 && isWord(text[i - 1])
            // A CI label, exactly as the notes write it.
            if (!prevWord) {
                for (l in labels) {
                    if (!text.startsWith(l.label, i)) continue
                    val end = i + l.label.length
                    if (end < text.length && isWord(text[end]) && isWord(l.label.last())) continue
                    flush()
                    out += LinkSegment.Url(l.url, l.label)
                    i = end
                    continue@scan
                }
            }
            // A bare URL.
            if ((c == 'h' || c == 'H') && !prevWord && text.startsWith("http", i, ignoreCase = true)) {
                var j = i
                while (j < text.length && !text[j].isWhitespace() && text[j] !in "<>\"'") j++
                while (j > i && text[j - 1] in ".,;:!?)]}") j--
                val s = text.substring(i, j)
                if (isWebUrl(s)) {
                    flush()
                    out += LinkSegment.Url(s, s)
                    i = j
                    continue@scan
                }
            }
            // A task number.
            if (c == '#' && !prevWord && i + 1 < text.length && isAsciiDigit(text[i + 1])) {
                var j = i + 1
                while (j < text.length && isAsciiDigit(text[j])) j++
                val digits = text.substring(i + 1, j)
                val before = text.substring(0, i).lowercase()
                val notATask = notTask.any { w ->
                    before.endsWith("$w ") && (before.length == w.length + 1 || !isWord(before[before.length - w.length - 2]))
                } || before.endsWith("pull request's checks (") || before.endsWith("(pr ") || before.endsWith("#")
                val n = digits.toLongOrNull()
                if (!notATask && n != null && isKnownTask(n)) {
                    flush()
                    out += LinkSegment.Task(n, "#$digits")
                    i = j
                    continue@scan
                }
                // "the pull request's checks (#7)": the pull request, when its URL is known.
                if (before.endsWith("pull request's checks (")) {
                    val l = labels.firstOrNull { it.label == "PR #$digits" }
                    if (l != null) {
                        flush()
                        out += LinkSegment.Url(l.url, "#$digits")
                        i = j
                        continue@scan
                    }
                }
                plain.append('#').append(digits)
                i = j
                continue@scan
            }
            plain.append(c)
            i++
        }
        flush()
        return out
    }
}
