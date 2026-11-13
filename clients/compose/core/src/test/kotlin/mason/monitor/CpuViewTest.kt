// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import mason.v1.CpuShare
import mason.v1.CpuUsage
import mason.v1.HomeView
import mason.v1.TaskDetailView
import mason.v1.View
import kotlin.test.*

class CpuViewTest {
    private fun task(id: Long, permille: Int) = CpuShare(task = id, kind = "task", label = "#$id", title = "Title $id", permille = permille)
    private fun fixed(kind: String, permille: Int) = CpuShare(kind = kind, label = kind, permille = permille)

    private fun usage(vararg s: CpuShare, busy: Int = 840, sampled: Long = 100_000) =
        CpuUsage(busy_permille = busy, cores = 18, sampled_ms = sampled, shares = s.toList())

    private val five = arrayOf(task(287, 234), task(5, 200), task(9, 150), fixed("masond", 40), fixed("other", 5))

    @Test
    fun segmentsAndLegendKeepTheDaemonsOrder() {
        val l = cpuLayout(usage(*five), 101_000, mapOf(287L to 0, 5L to 1, 9L to 2))!!
        assertEquals("CPU 84% busy · 18 cores", l.header)
        assertNull(l.hidden)
        assertEquals(listOf("#287", "#5", "#9", "masond", "other"), l.rows.map { it.label })
        // "other" is under 1% of the bar: no segment, but it keeps its row.
        assertEquals(listOf("#287", "#5", "#9", "masond"), l.segments.map { it.label })
    }

    @Test
    fun widthsAreProportionalWithIdleAsTheRemainder() {
        val l = cpuLayout(usage(*five), 101_000, emptyMap())!!
        assertEquals(234, l.segments[0].permille)
        assertEquals(1000 - (234 + 200 + 150 + 40 + 5), l.idlePermille)
    }

    @Test
    fun aShareUnderOnePercentHasNoSegmentButKeepsItsLegendRow() {
        val l = cpuLayout(usage(task(1, 500), task(2, 9)), 101_000, emptyMap())!!
        assertEquals(listOf("#1"), l.segments.map { it.label })
        assertEquals(listOf("#1", "#2"), l.rows.map { it.label })
    }

    @Test
    fun rowsPastEightFoldIntoOneMoreRowWithTheRightTotal() {
        val shares = (1L..11L).map { task(it, 100 - it.toInt()) }
        val l = cpuLayout(usage(*shares.toTypedArray()), 101_000, emptyMap())!!
        assertEquals(8, l.rows.size)
        assertEquals(3, l.moreCount)
        assertEquals(91 + 90 + 89, l.morePermille)
        assertEquals("3 more, 27.0% together", l.moreText)
    }

    @Test
    fun aTaskKeepsItsColourWhenAnotherOvertakesItAndAFreedColourIsReused() {
        val c = CpuColors()
        val first = c.assign(listOf(10L, 20L))
        assertEquals(mapOf(10L to 0, 20L to 1), first)
        // 20 now leads, and 5 joins: 10 and 20 keep theirs, 5 takes the lowest free one.
        val second = c.assign(listOf(20L, 10L, 5L))
        assertEquals(0, second[10L])
        assertEquals(1, second[20L])
        assertEquals(2, second[5L])
        // 10 leaves; its colour goes to the next newcomer.
        val third = c.assign(listOf(20L, 5L, 30L))
        assertEquals(0, third[30L])
        assertEquals(1, third[20L])
        assertEquals(2, third[5L])
    }

    @Test
    fun fixedBucketsUseTheirOwnColoursAndNames() {
        val l = cpuLayout(usage(fixed("masond", 100), fixed("other", 300), task(3, 200)), 101_000, mapOf(3L to 4))!!
        val by = l.rows.associateBy { it.label }
        assertEquals("masond", by["masond"]!!.color)
        assertEquals("other", by["other"]!!.color)
        assertEquals("5", by["#3"]!!.color)
        assertEquals("MASON daemon", by["masond"]!!.name)
        assertEquals("Everything else", by["other"]!!.name)
    }

    @Test
    fun staleAndMissingReadingsHideTheSegments() {
        val stale = cpuLayout(usage(*five, sampled = 100_000), 100_000 + CPU_STALE_MS + 1, emptyMap())!!
        assertNotNull(stale.hidden)
        assertTrue(stale.segments.isEmpty())
        val missing = cpuLayout(usage(*five, busy = -1), 101_000, emptyMap())!!
        assertNotNull(missing.hidden)
        assertTrue(missing.segments.isEmpty())
        assertNull(cpuLayout(null, 101_000, emptyMap()))
    }

    @Test
    fun theTaskPageSaysHowMuchOrThatItIsNotRunning() {
        assertEquals("CPU: 23.4% of the machine", cpuTaskLine(234))
        assertEquals("CPU: not running", cpuTaskLine(-1))
    }

    @Test
    fun theModelKeepsColoursAcrossAReorderedReading() {
        val m = MonitorModel()
        m.apply(View(home = HomeView(cpu = usage(task(1, 300), task(2, 200)))))
        assertEquals(0, m.cpuColors[1L])
        assertEquals(1, m.cpuColors[2L])
        m.apply(View(home = HomeView(cpu = usage(task(2, 400), task(1, 100), task(3, 50)))))
        assertEquals(0, m.cpuColors[1L])
        assertEquals(1, m.cpuColors[2L])
        assertEquals(2, m.cpuColors[3L])
        assertEquals(true, TaskDetailView(cpu_permille = 12).cpu_permille == 12)
    }
}
