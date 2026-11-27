// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import mason.v1.DiskHistory
import mason.v1.DiskPoint
import mason.v1.DiskReading
import mason.v1.DiskVolume
import mason.v1.View
import mason.v1.WorktreeUsage
import kotlin.test.*

class DiskViewTest {
    private val a = DiskVolume(id = "a", name = "sp", mount = "/Volumes/sp", roles = listOf("data", "worktrees"))
    private val b = DiskVolume(id = "b", name = "Macintosh HD", mount = "/", roles = listOf("project", "cache"))

    private fun r(v: String, free: Long, total: Long) = DiskReading(volume = v, free_bytes = free, total_bytes = total)

    private val two = DiskHistory(
        span_ms = 300_000, step_ms = 5000, volumes = listOf(a, b),
        points = listOf(
            DiskPoint(at_ms = 1000, readings = listOf(r("a", 400_000_000_000, 1_800_000_000_000), r("b", 100_000_000_000, 500_000_000_000))),
            DiskPoint(at_ms = 6000, readings = listOf(r("a", 399_000_000_000, 1_800_000_000_000))),
            DiskPoint(at_ms = 11000, readings = listOf(r("a", 398_000_000_000, 1_800_000_000_000), r("b", 90_000_000_000, 500_000_000_000))),
        ),
    )

    @Test
    fun oneVolumeIsOneSeriesScaledToItsTotal() {
        val h = two.copy(volumes = listOf(a), points = two.points.map { it.copy(readings = it.readings.filter { x -> x.volume == "a" }) })
        val c = diskChart(h, emptySet())
        assertEquals(listOf(1000L, 6000L, 11000L), c.times)
        assertEquals(listOf("a"), c.series.map { it.id })
        assertEquals(listOf<Long?>(400_000_000_000, 399_000_000_000, 398_000_000_000), c.series[0].free)
        assertEquals(1_800_000_000_000, c.maxTotal)
    }

    @Test
    fun severalVolumesGetOneSeriesEachAndAGapWhereAbsent() {
        val c = diskChart(two, emptySet())
        assertEquals(listOf("a", "b"), c.series.map { it.id })
        assertEquals(listOf<Long?>(100_000_000_000, null, 90_000_000_000), c.series[1].free)
        assertEquals(1_800_000_000_000, c.maxTotal)
        assertEquals("Macintosh HD: not read", c.series[1].tip(1))
        assertEquals("sp: 399 GB free of 1.8 TB", c.series[0].tip(1))
    }

    @Test
    fun hidingAVolumeRemovesItsSeriesAndRescalesY() {
        val c = diskChart(two, setOf("a"))
        assertEquals(listOf("b"), c.series.map { it.id })
        assertEquals(500_000_000_000, c.maxTotal)
        val none = diskChart(two, setOf("a", "b"))
        assertTrue(none.series.isEmpty())
        assertEquals(1, none.maxTotal)
    }

    @Test
    fun coloursStayPutWhenOneIsHidden() {
        val all = diskChart(two, emptySet()).series.associate { it.id to it.color }
        val hiddenA = diskChart(two, setOf("a")).series.associate { it.id to it.color }
        assertEquals(all["b"], hiddenA["b"])
        assertNotEquals(all["a"], all["b"])
        val p = DiskColors()
        assertEquals(mapOf("a" to 0, "b" to 1), p.assign(listOf("a", "b")))
        assertEquals(mapOf("b" to 1, "c" to 0), p.assign(listOf("b", "c")))
    }

    @Test
    fun chartIndexIsTheNearestPoint() {
        val c = diskChart(two, emptySet())
        assertEquals(0, diskChartIndex(c, 0.0))
        assertEquals(1, diskChartIndex(c, 0.5))
        assertEquals(2, diskChartIndex(c, 5.0))
        assertNull(diskChartIndex(diskChart(DiskHistory(), emptySet()), 0.5))
    }

    @Test
    fun bytesUseDecimalUnitsLikeFinder() {
        assertEquals("0 B", Format.bytes(0))
        assertEquals("999 B", Format.bytes(999))
        assertEquals("1.0 KB", Format.bytes(1000))
        assertEquals("999 GB", Format.bytes(999_000_000_000))
        assertEquals("1.0 TB", Format.bytes(999_600_000_000))
        assertEquals("1.0 TB", Format.bytes(1_000_000_000_000))
        assertEquals("9.9 GB", Format.bytes(9_900_000_000))
        assertEquals("412 GB", Format.bytes(412_300_000_000))
        assertEquals("1.8 TB", Format.bytes(1_800_000_000_000))
        assertEquals("412 GB free of 1.8 TB", Format.freeOf(412_000_000_000, 1_800_000_000_000))
    }

    @Test
    fun theLatestReadingSkipsAGap() {
        assertEquals(90_000_000_000 to 500_000_000_000, diskLatest(two, "b"))
        assertEquals(398_000_000_000 to 1_800_000_000_000, diskLatest(two, "a"))
        assertNull(diskLatest(two, "zzz"))
        assertEquals("/Volumes/sp: data, worktrees", diskVolumeTip(a.mount, a.roles))
        assertEquals("/: projects, cache", diskVolumeTip(b.mount, b.roles))
    }

    @Test
    fun theWorktreeTotalsAreListedAfterTheirSum() {
        assertEquals(emptyList(), diskWorktreeLines(two))
        val h = two.copy(
            worktrees = listOf(
                WorktreeUsage(kind = "task worktree", state = "canceled", count = 43, bytes = 188_000_000_000, free_next_bytes = 188_000_000_000),
                WorktreeUsage(kind = "landing scratch", state = "idle", count = 12, bytes = 26_000_000_000),
            ),
        )
        assertEquals(
            listOf(
                "Worktrees 214 GB · next sweep frees 188 GB",
                "task worktree, canceled: 43, 188 GB",
                "landing scratch, idle: 12, 26 GB",
            ),
            diskWorktreeLines(h),
        )
    }

    @Test
    fun hiddenVolumesSurviveARestartAndAVanishedIdIsDropped() {
        val prefs = MemoryPreferences()
        val m = MonitorModel(prefs)
        m.apply(View(disk_history = two))
        m.toggleVolume("b")
        assertEquals(setOf("b"), m.hiddenVolumes)
        assertEquals(setOf("b"), MonitorModel(prefs).hiddenVolumes)
        m.toggleVolume("b")
        assertEquals(emptySet(), MonitorModel(prefs).hiddenVolumes)

        m.toggleVolume("a"); m.toggleVolume("b")
        val again = MonitorModel(prefs)
        again.apply(View(disk_history = two.copy(volumes = listOf(a))))
        assertEquals(setOf("a"), again.hiddenVolumes)
        assertEquals(setOf("a"), MonitorModel(prefs).hiddenVolumes)
    }

    @Test
    fun anOlderDaemonSendsNoDiskHistoryAndTheSectionStaysHidden() {
        val m = MonitorModel()
        assertFalse(m.diskSeen)
        m.apply(View(disk_history = two))
        assertTrue(m.diskSeen)
        m.pickDiskSpan(DiskSpan.Day)
        assertNull(m.diskHistory)
        assertTrue(m.diskSeen)
    }
}
