// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import mason.v1.Compilation
import mason.v1.ProgressCell
import mason.v1.ProgressSource
import mason.v1.TokenLine

object Format {
    /** Time in state: "42 s", "9 min", "3h10". Always shown, never hidden. */
    fun duration(ms: Long): String {
        val s = ms / 1000
        return when {
            s < 60 -> "$s s"
            s < 3600 -> "${s / 60} min"
            else -> {
                val m = (s % 3600) / 60
                if (m == 0L) "${s / 3600}h" else "${s / 3600}h${m.toString().padStart(2, '0')}"
            }
        }
    }

    /** A share in permille as a percentage with one decimal: 234 is "23.4%", 5 is "0.5%". */
    fun permille(p: Int): String = "${p / 10}.${p % 10}%"

    /**
     * A token count in few characters: "988", "12.2k", "1.2M". Whole-number arithmetic, so it rounds
     * exactly as the daemon's own reading (`mason_core::progress_text::compact`) does.
     */
    fun compactCount(n: Long): String {
        if (n < 1000) return n.toString()
        val k = (n + 50) / 100
        if (k < 10_000) return "${k / 10}.${k % 10}k"
        val m = (n + 50_000) / 100_000
        return "${m / 10}.${m % 10}M"
    }

    /**
     * "12.2k in, 988 out": "in" is input plus cache read plus cache write; a side nothing was
     * reported for is left out. Empty when `t` is null or reports nothing.
     */
    fun tokensCompact(t: TokenLine?): String {
        if (t == null) return ""
        val parts = mutableListOf<String>()
        if (t.has_input || t.has_cache_read || t.has_cache_write) parts += compactCount(t.input + t.cache_read + t.cache_write) + " in"
        if (t.has_output) parts += compactCount(t.output) + " out"
        return parts.joinToString(", ")
    }

    /** Each reported figure in full: "input 4", "output 988", "cache read 7,664", ..., "cost $0.0123". */
    fun tokensDetail(t: TokenLine?): List<String> {
        if (t == null) return emptyList()
        val v = mutableListOf<String>()
        if (t.has_input) v += "input ${group(t.input)}"
        if (t.has_output) v += "output ${group(t.output)}"
        if (t.has_cache_read) v += "cache read ${group(t.cache_read)}"
        if (t.has_cache_write) v += "cache write ${group(t.cache_write)}"
        if (t.has_cost) v += "cost $" + String.format(java.util.Locale.ROOT, "%.4f", t.cost_usd)
        return v
    }

    /** 1240 as "1,240" (always a comma, whatever the locale: the same text in every client). */
    fun group(n: Long): String {
        val s = n.toString()
        val out = StringBuilder()
        s.forEachIndexed { i, c ->
            if (i > 0 && (s.length - i) % 3 == 0) out.append(',')
            out.append(c)
        }
        return out.toString()
    }

    /** The whole percent `done` is of a real `total`: rounded down, at most 100, none for 0. */
    fun percent(done: Long, total: Long): Int? = if (total == 0L) null else (minOf(done, total) * 100 / total).toInt()

    /**
     * What a count and its total say, by the rule for numbers: "212 of 340 tests, 62%" for a real
     * total; "1,240 lines, usually 1,800 to 2,400" for a typical one (never a percentage); "37 tool
     * calls" for none. Empty when there is nothing to say yet.
     */
    fun countText(unit: String, done: Long, doneSource: ProgressSource, total: Long, totalSource: ProgressSource, totalLow: Long): String {
        val u = if (unit.isEmpty()) "" else " $unit"
        when (totalSource) {
            ProgressSource.SOURCE_REPORTED, ProgressSource.SOURCE_COUNTED,
            ProgressSource.SOURCE_EXPECTED, ProgressSource.SOURCE_MINIMUM ->
                if (total > 0) return "${group(done)} of ${group(total)}$u, ${percent(done, total) ?: 0}%"
            ProgressSource.SOURCE_HISTORY ->
                if (doneSource != ProgressSource.SOURCE_NONE) {
                    val range = if (totalLow in 1 until total) "${group(totalLow)} to ${group(total)}" else group(total)
                    return "${group(done)}$u, usually $range"
                }
            else -> {}
        }
        return if (doneSource != ProgressSource.SOURCE_NONE) "${group(done)}$u" else ""
    }

    /** A real total (reported, counted, the last green run's count, or a suite's minimum), not a typical one. */
    fun isReal(s: ProgressSource): Boolean = when (s) {
        ProgressSource.SOURCE_REPORTED, ProgressSource.SOURCE_COUNTED,
        ProgressSource.SOURCE_EXPECTED, ProgressSource.SOURCE_MINIMUM -> true
        else -> false
    }

    /** Where a figure came from, in words. */
    fun sourceWords(s: ProgressSource, samples: Int): String = when (s) {
        ProgressSource.SOURCE_REPORTED -> "from the agent"
        ProgressSource.SOURCE_COUNTED -> "counted from output"
        ProgressSource.SOURCE_HISTORY -> "usually, from $samples past runs"
        ProgressSource.SOURCE_EXPECTED -> "the last green run's count"
        ProgressSource.SOURCE_MINIMUM -> "the suite's minimum"
        else -> ""
    }

    /** Quiet past `afterMs`: nothing has moved for longer than the step's usual gap. */
    fun isStale(now: Long, moved: Long, afterMs: Long): Boolean = moved > 0 && maxOf(now - moved, 0) > afterMs

    /** "updated 4 s ago", or "no output for 6 min" when stale. Empty when nothing moved yet. */
    fun timeText(now: Long, moved: Long, afterMs: Long): String {
        if (moved == 0L) return ""
        val span = duration(maxOf(now - moved, 0))
        return if (isStale(now, moved, afterMs)) "no output for $span" else "updated $span ago"
    }
}

/** How a running cell is filled. */
sealed class CellFill {
    /** A real total: filled in proportion (0..1). */
    data class Proportional(val fraction: Float) : CellFill()

    /** Only a typical total: filled the same way, lighter. */
    data class Typical(val fraction: Float) : CellFill()

    /** Nothing to measure against: the outline alone. */
    object Outline : CellFill()
}

val ProgressCell.hasRealTotal: Boolean
    get() = Format.isReal(total_source) && total > 0

/** The whole percent, only from a real total. */
val ProgressCell.percent: Int? get() = if (hasRealTotal) Format.percent(done.toLong(), total.toLong()) else null

/** The tokens the step's LLM sessions have used so far, "12.2k in, 988 out"; empty when none were reported. */
val ProgressCell.tokensCompact: String
    get() = Format.tokensCompact(tokens)

val ProgressCell.countText: String
    get() = Format.countText(unit, done.toLong(), done_source, total.toLong(), total_source, total_low.toLong())

val ProgressCell.fill: CellFill
    get() = when {
        hasRealTotal -> CellFill.Proportional(minOf(done, total).toFloat() / total)
        total_source == ProgressSource.SOURCE_HISTORY && total > 0 -> CellFill.Typical(minOf(1f, done.toFloat() / total))
        else -> CellFill.Outline
    }

fun ProgressCell.isStale(now: Long): Boolean = Format.isStale(now, updated_at_ms, stale_after_ms)

fun ProgressCell.timeText(now: Long): String = Format.timeText(now, updated_at_ms, stale_after_ms)

/** The step that is running now and has a live record. */
val List<ProgressCell>.running: ProgressCell?
    get() = firstOrNull { it.state == mason.v1.CellState.PART && it.run != 0L }

/**
 * The cell's accessibility label: "develop, running, 3 of 7 plan steps, 42 percent". The ticking
 * time is deliberately not in it.
 */
val ProgressCell.spokenLabel: String
    get() {
        val word = when (state) {
            mason.v1.CellState.FILLED -> "finished"
            mason.v1.CellState.PART -> "running"
            mason.v1.CellState.HATCHED -> "waiting"
            else -> "not started"
        }
        var s = "$step, $word"
        if (state == mason.v1.CellState.PART) {
            val c = countText
            if (c.isNotEmpty()) {
                val p = percent
                s += ", " + if (p != null) c.replace(", $p%", ", $p percent") else c
            }
        }
        return s
    }

/** Everything a running step's progress says, without any clock text (what conformance compares). */
fun ProgressCell.progressTexts(now: Long): List<String> {
    val t = mutableListOf<String>()
    val c = countText
    if (c.isNotEmpty()) t += c
    if (message.isNotEmpty()) t += message
    if (done_source != ProgressSource.SOURCE_NONE) t += "count " + Format.sourceWords(done_source, history_samples)
    if (total_source != ProgressSource.SOURCE_NONE) t += "total " + Format.sourceWords(total_source, history_samples)
    if (lines > 0 || bytes > 0) t += "output ${Format.group(lines)} lines, ${Format.group(bytes)} bytes"
    tokens?.let { k ->
        val c = Format.tokensCompact(k)
        if (c.isNotEmpty()) t += "tokens $c"
        t += Format.tokensDetail(k)
    }
    subs.forEach {
        t += "${it.name}: " + Format.countText(it.unit, it.done.toLong(), it.done_source, it.total.toLong(), it.total_source, 0)
    }
    todo.forEach { t += "${it.status}: ${it.text}" }
    if (isStale(now)) t += "silent"
    return t
}

/** The CPU line of the home page's Compilation section. */
val Compilation.cpuLine: String
    get() {
        val gate = "a further compile starts below $cpu_max_pct%"
        return cpuReadingLine(if (cpu_busy_pct < 0) null else cpu_busy_pct, cpu_source, gate)
    }

/**
 * The CPU line shared by the Health page and the home page. A fallback source is named, so a
 * held compile is not read as a broken gauge.
 */
fun cpuReadingLine(busy: Int?, source: String, gate: String): String = when {
    busy == null -> "CPU not readable; $gate."
    source == "loadavg" -> "CPU $busy% busy (load average; the CPU reading is missing); $gate."
    source == "assumed-busy" -> "CPU assumed $busy% busy (nothing is readable, so new compiles wait); $gate."
    else -> "CPU $busy% busy; $gate."
}
