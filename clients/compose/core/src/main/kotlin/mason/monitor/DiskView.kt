// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import mason.v1.DiskHistory

/**
 * The home page's disk section, as data: one line per volume of the daemon's `DiskHistory`
 * (free bytes over time, y from 0 to the largest total shown). Pure, like `CpuView.kt`.
 */

/** The graph offers the same four spans as the CPU graph. */
typealias DiskSpan = CpuSpan

/** The CPU and disk sections sit side by side on home when the page is at least this wide (dp); below it they stack. */
const val HOME_TWO_COLUMN_MIN_DP = 900

/** The CPU section's share of the row, against [HOME_DISK_WEIGHT] for the disk section (about 3:2). */
const val HOME_CPU_WEIGHT = 3f
const val HOME_DISK_WEIGHT = 2f

/** How many colour slots a volume can hold: the theme's ten task colours. */
const val DISK_COLOURS = CPU_TASK_COLOURS

/**
 * Keeps a volume's colour while it is listed, so hiding another volume or a reorder does not
 * recolour it. A new volume takes the lowest slot nobody holds; one that vanishes frees its slot.
 */
class DiskColors {
    private val held = linkedMapOf<String, Int>()

    fun assign(ids: Collection<String>): Map<String, Int> {
        held.keys.retainAll(ids.toSet())
        for (id in ids) {
            if (id in held) continue
            held[id] = (0 until DISK_COLOURS).firstOrNull { it !in held.values } ?: (held.size % DISK_COLOURS)
        }
        return held.toMap()
    }
}

/** One volume's line: free bytes at each time of [DiskChart.times], null where the volume was not read. */
data class DiskSeries(
    val id: String,
    val name: String,
    val mount: String,
    val roles: List<String>,
    /** A theme colour key, "1".."10" (what `Theme.cpu` takes). */
    val color: String,
    val free: List<Long?>,
    val total: List<Long?>,
) {
    /** "name: 412 GB free of 1.8 TB" at [index]; "name: not read" for a gap. */
    fun tip(index: Int): String {
        val f = free[index]
        val t = total[index]
        return if (f == null || t == null) "$name: not read" else "$name: ${Format.freeOf(f, t)}"
    }
}

/** The history ready to draw: [maxTotal] is the top of the y scale (never 0). */
data class DiskChart(val spanMs: Long, val times: List<Long>, val series: List<DiskSeries>, val maxTotal: Long)

/**
 * The chart for [h] without the volumes in [hidden]. A series per volume that is shown, in the
 * daemon's order. [colors] (volume id to slot) is what the model keeps stable; without it the slots
 * go by the daemon's order of all volumes, hidden ones included, so hiding never recolours.
 */
fun diskChart(h: DiskHistory, hidden: Set<String>, colors: Map<String, Int>? = null): DiskChart {
    val slots = colors ?: DiskColors().assign(h.volumes.map { it.id })
    val shown = h.volumes.filter { it.id !in hidden }
    val series = shown.map { v ->
        val readings = h.points.map { p -> p.readings.firstOrNull { it.volume == v.id } }
        DiskSeries(
            id = v.id,
            name = v.name,
            mount = v.mount,
            roles = v.roles,
            color = ((slots[v.id] ?: 0) + 1).toString(),
            free = readings.map { it?.free_bytes },
            total = readings.map { it?.total_bytes },
        )
    }
    val top = series.flatMap { s -> s.total.filterNotNull() }.maxOrNull() ?: 1L
    return DiskChart(h.span_ms, h.points.map { it.at_ms }, series, maxOf(1L, top))
}

/** The index of the point nearest to [fraction] (0..1) of the chart's width; null for an empty chart. */
fun diskChartIndex(c: DiskChart, fraction: Double): Int? {
    val first = c.times.firstOrNull() ?: return null
    val last = c.times.last()
    val at = first + fraction.coerceIn(0.0, 1.0) * (last - first)
    return c.times.indices.minByOrNull { kotlin.math.abs(c.times[it] - at) }
}

/** The newest free and total bytes of volume [id] in [h]; null if it was never read. */
fun diskLatest(h: DiskHistory, id: String): Pair<Long, Long>? {
    for (p in h.points.asReversed()) {
        val r = p.readings.firstOrNull { it.volume == id } ?: continue
        return r.free_bytes to r.total_bytes
    }
    return null
}

/** The legend's tip for a volume: its mount and what lives on it, "/Volumes/sp: data, cache". */
fun diskVolumeTip(mount: String, roles: List<String>): String {
    val what = roles.joinToString(", ") { if (it == "project") "projects" else it }
    return if (what.isEmpty()) mount else "$mount: $what"
}

/** The hidden volumes as the preference stores them, one id per line. */
fun encodeHiddenVolumes(ids: Set<String>): String = ids.sorted().joinToString("\n")

fun decodeHiddenVolumes(s: String?): Set<String> = (s ?: "").split("\n").filter { it.isNotEmpty() }.toSet()

/** [hidden] without the ids [h] no longer lists. */
fun pruneHiddenVolumes(hidden: Set<String>, h: DiskHistory): Set<String> {
    val listed = h.volumes.map { it.id }.toSet()
    return hidden.filter { it in listed }.toSet()
}

/**
 * What the pipeline's worktrees take, the same totals as `mason disk`: a line for all of it,
 * then one per kind and task state. Empty until the daemon has measured them.
 */
fun diskWorktreeLines(h: DiskHistory): List<String> {
    if (h.worktrees.isEmpty()) return emptyList()
    val all = h.worktrees.sumOf { it.bytes }
    val next = h.worktrees.sumOf { it.free_next_bytes }
    return listOf("Worktrees ${Format.bytes(all)} · next sweep frees ${Format.bytes(next)}") +
        h.worktrees.map { "${it.kind}, ${it.state}: ${it.count}, ${Format.bytes(it.bytes)}" }
}
