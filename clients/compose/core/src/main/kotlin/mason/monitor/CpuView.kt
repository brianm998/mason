// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import mason.v1.CpuHistory
import mason.v1.CpuUsage

/**
 * The home page's CPU section, as data: the bar's segments, the legend rows and the header,
 * from the daemon's `CpuUsage` (shares in permille of the whole machine, highest first).
 * The same rules are in the Swift client's `CpuView.swift`.
 */
const val CPU_TASK_COLOURS = 10

/** A reading older than this (three sampling periods of 5 s) is stale. */
const val CPU_STALE_MS = 15_000L

/** The legend shows this many rows, then folds the rest into one. */
const val CPU_LEGEND_TOP = 8

/** A segment narrower than this share of the bar (1%, in permille) is not drawn. */
const val CPU_MIN_SEGMENT_PERMILLE = 10

/** One share, ready to draw. [color] is a token name suffix: "1".."10", "masond", "monitor", "sccache", "other". */
data class CpuRow(
    val label: String,
    val name: String,
    val task: Long,
    val title: String,
    /** The task's project; empty for a fixed bucket. */
    val project: String,
    val permille: Int,
    val color: String,
) {
    /** "#287 (star)" for a task, the fixed name otherwise: what the legend and tips lead with. */
    val heading: String get() = if (project.isEmpty()) name else "$name ($project)"

    /** "#287 (star) Scheduler: … 23.4%", for a hover or a tap. */
    val tip: String get() = (if (title.isEmpty()) heading else "$heading $title") + " " + Format.permille(permille)
}

data class CpuLayout(
    /** "CPU 84% busy · 18 cores". */
    val header: String,
    /** Why nothing is drawn (stale or missing reading); null when it is. */
    val hidden: String?,
    /** The bar's segments, left to right (each as wide as its permille); empty while hidden. */
    val segments: List<CpuRow>,
    /** The legend's first rows, in the same order. */
    val rows: List<CpuRow>,
    /** The rows past the top 8, shown when the folded row is expanded. */
    val more: List<CpuRow>,
    /** What is left of the bar: idle, in permille. */
    val idlePermille: Int,
) {
    val moreCount: Int get() = more.size
    val morePermille: Int get() = more.sumOf { it.permille }
    val moreText: String get() = "$moreCount more, ${Format.permille(morePermille)} together"
}

/**
 * Keeps a task's colour while it is among the shares, so it does not change when the order does.
 * A new task takes the lowest colour no other task holds (not a hash, which collides); a task
 * that leaves frees its colour. With more tasks than colours the extra ones share by task id.
 */
class CpuColors {
    private val held = mutableMapOf<Long, Int>()

    fun assign(tasks: Collection<Long>): Map<Long, Int> {
        held.keys.retainAll(tasks.toSet())
        for (t in tasks.toSortedSet()) {
            if (t in held) continue
            held[t] = (0 until CPU_TASK_COLOURS).firstOrNull { it !in held.values } ?: (t % CPU_TASK_COLOURS).toInt()
        }
        return held.toMap()
    }
}

private fun fixedName(kind: String): String = when (kind) {
    "masond" -> "MASON daemon"
    "monitor" -> "Monitors"
    "sccache" -> "Shared compiles (sccache)"
    "other" -> "Everything else"
    else -> kind
}

/**
 * The layout for [u] at the daemon's time [nowMs]; null when the daemon sends no CPU data
 * (an older daemon), which hides the section.
 */
fun cpuLayout(u: CpuUsage?, nowMs: Long, colors: Map<Long, Int>): CpuLayout? {
    if (u == null) return null
    val header = if (u.busy_permille < 0) "CPU" else "CPU ${(u.busy_permille + 5) / 10}% busy · ${u.cores} cores"
    val hidden = when {
        u.busy_permille < 0 || u.sampled_ms == 0L -> "CPU use is not being read."
        nowMs - u.sampled_ms > CPU_STALE_MS -> "The CPU reading is ${Format.duration(nowMs - u.sampled_ms)} old."
        else -> null
    }
    if (hidden != null) return CpuLayout(header, hidden, emptyList(), emptyList(), emptyList(), 1000)
    val all = u.shares.map { s ->
        val task = s.kind == "task"
        CpuRow(
            label = s.label,
            name = if (task) s.label else fixedName(s.kind),
            task = if (task) s.task else 0,
            title = if (task) s.title else "",
            project = if (task) s.project else "",
            permille = s.permille,
            color = if (task) ((colors[s.task] ?: (s.task % CPU_TASK_COLOURS).toInt()) + 1).toString() else s.kind,
        )
    }
    return CpuLayout(
        header = header,
        hidden = null,
        segments = all.filter { it.permille >= CPU_MIN_SEGMENT_PERMILLE },
        rows = all.take(CPU_LEGEND_TOP),
        more = all.drop(CPU_LEGEND_TOP),
        idlePermille = maxOf(0, 1000 - all.sumOf { it.permille }),
    )
}

/** The task page's line: "CPU: 23.4% of the machine", or "CPU: not running". */
fun cpuTaskLine(permille: Int): String =
    if (permille < 0) "CPU: not running" else "CPU: ${Format.permille(permille)} of the machine"

/** The spans the graph offers. */
enum class CpuSpan(val ms: Long, val label: String) {
    FiveMinutes(300_000, "5 min"), Hour(3_600_000, "1 h"), SixHours(21_600_000, "6 h"), Day(86_400_000, "24 h"),
}

/** One series of the graph: a share's permille at each time, in the same colour as its bar segment. */
data class CpuSeries(
    val key: String,
    val heading: String,
    val title: String,
    val color: String,
    /** One value per entry of [CpuChart.times] (0 where the share was absent). */
    val values: List<Int>,
) {
    /** "#287 (star) Scheduler: … 23.4%" at [index]. */
    fun tip(index: Int): String = (if (title.isEmpty()) heading else "$heading $title") + " " + Format.permille(values[index])
}

/** The CPU history, ready to draw: points in time order and a series per share, biggest total first (at the bottom). */
data class CpuChart(val spanMs: Long, val times: List<Long>, val series: List<CpuSeries>, val maxStack: Int)

private fun shareKey(kind: String, task: Long) = if (kind == "task") "task:$task" else kind

/**
 * The chart for [h]. Titles and projects come from [usage] (the history carries neither); a task in
 * [colors] keeps the colour its bar segment has, any other the id-based fallback the bar uses.
 * Pure, so the app runs it off the main thread.
 */
fun cpuChart(h: CpuHistory, usage: CpuUsage?, colors: Map<Long, Int>): CpuChart {
    val info = usage?.shares.orEmpty().associateBy { shareKey(it.kind, it.task) }
    val first = linkedMapOf<String, mason.v1.CpuShare>()
    val totals = mutableMapOf<String, Long>()
    for (p in h.points) for (s in p.shares) {
        val k = shareKey(s.kind, s.task)
        first.putIfAbsent(k, s)
        totals[k] = (totals[k] ?: 0L) + s.permille
    }
    val order = first.keys.sortedWith(compareBy({ -(totals[it] ?: 0L) }, { it }))
    val stack = IntArray(h.points.size)
    val series = order.map { k ->
        val s = first.getValue(k)
        val task = s.kind == "task"
        val values = h.points.mapIndexed { i, p ->
            val v = p.shares.firstOrNull { shareKey(it.kind, it.task) == k }?.permille ?: 0
            stack[i] += v
            v
        }
        val name = if (task) s.label else fixedName(s.kind)
        val project = if (task) info[k]?.project.orEmpty() else ""
        CpuSeries(
            key = k,
            heading = if (project.isEmpty()) name else "$name ($project)",
            title = if (task) info[k]?.title.orEmpty() else "",
            color = if (task) ((colors[s.task] ?: (s.task % CPU_TASK_COLOURS).toInt()) + 1).toString() else s.kind,
            values = values,
        )
    }
    return CpuChart(h.span_ms, h.points.map { it.at_ms }, series, maxOf(1, stack.maxOrNull() ?: 1))
}

/** The index of the point nearest to [fraction] (0..1) of the chart's width; null for an empty chart. */
fun cpuChartIndex(c: CpuChart, fraction: Double): Int? {
    val first = c.times.firstOrNull() ?: return null
    val last = c.times.last()
    val at = first + fraction.coerceIn(0.0, 1.0) * (last - first)
    return c.times.indices.minByOrNull { kotlin.math.abs(c.times[it] - at) }
}
