// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

class UiWatchdogTest {
    @Test
    fun pingsOutsideTheWindowDropOut() {
        val s = LatenessStats(thresholdMs = 100.0, windowMs = 600_000.0)
        s.record(900.0, atMs = 0.0)
        s.record(150.0, atMs = 300_000.0)
        s.record(5.0, atMs = 700_000.0)
        assertEquals(LatenessWindow(pings = 2, late = 1, worstMs = 150.0, totalLateMs = 150.0), s.window(now = 700_000.0))
        assertEquals(LatenessWindow(pings = 3, late = 2, worstMs = 900.0, totalLateMs = 1050.0), s.sinceLaunch)
        assertEquals(0, s.window(now = 2_000_000.0).pings)
    }

    @Test
    fun releaseConfigWithoutTheEnvVarAndStartCreatesTheThread() {
        assertEquals(WatchdogConfig.RELEASE, WatchdogConfig.current { null })
        assertEquals(WatchdogConfig.DEBUG, WatchdogConfig.current { if (it == "MASON_WATCHDOG") "1" else null })
        assertEquals(250, WatchdogConfig.RELEASE.intervalMs)
        assertEquals(100.0, WatchdogConfig.RELEASE.thresholdMs)
        val w = UiWatchdog({ }, WatchdogConfig.RELEASE)
        w.start()
        try { assertTrue(w.isRunning) } finally { w.stop() }
    }

    @Test
    fun theLineHasTheSameWordsAsTheSwiftMonitor() {
        assertEquals(
            "UI thread: worst 640 ms late in the last 10 minutes, 31 late pings",
            LatenessWindow(pings = 2000, late = 31, worstMs = 640.0, totalLateMs = 5000.0).line(10),
        )
        assertEquals("UI thread: worst 0 ms late in the last 10 minutes, 1 late ping", LatenessWindow(late = 1).line(10))
    }

    private class Fake {
        var now = 0.0
        val held = ArrayDeque<() -> Unit>()
        var state = AppVisibility.FRONTMOST
        val lines = mutableListOf<String>()
        val dog = UiWatchdog({ held.addLast(it) }, WatchdogConfig.RELEASE, { now }, { lines += it }, { state })
    }

    @Test
    fun aBlockedUiThreadIsOneSampleAndOneLine() {
        val f = Fake()
        repeat(14) { f.dog.tick(); f.now += 250.0 }
        assertEquals(1, f.held.size)
        f.now = 3500.0
        f.held.removeFirst()()
        assertEquals(listOf("ui thread blocked for 3500 ms"), f.lines)
        assertEquals(LatenessWindow(1, 1, 3500.0, 3500.0), f.dog.recent())
        f.dog.tick()
        assertEquals(1, f.held.size)
    }

    @Test
    fun aHiddenPingCountsInTheBackgroundWindowOnly() {
        val f = Fake()
        f.state = AppVisibility.HIDDEN
        f.dog.tick(); f.now = 1500.0
        f.held.removeFirst()()
        assertEquals(LatenessWindow(), f.dog.recent())
        assertEquals(LatenessWindow(1, 1, 1500.0, 1500.0), f.dog.recentBackground())
        assertEquals(listOf("ui thread blocked for 1500 ms (app in background)"), f.lines)
    }

    @Test
    fun theBackgroundLineHasTheSameWordsAsTheSwiftMonitor() {
        assertEquals(
            "In the background: worst 3500 ms late in the last 10 minutes, 2 late pings",
            LatenessWindow(pings = 3, late = 2, worstMs = 3500.0, totalLateMs = 5000.0).backgroundLine(10),
        )
    }
}
