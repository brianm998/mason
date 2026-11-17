// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

/** The numbers for one window of the watchdog's pings. */
data class LatenessWindow(val pings: Int = 0, val late: Int = 0, val worstMs: Double = 0.0, val totalLateMs: Double = 0.0) {
    /** The Health page's line; the Swift monitor prints the same text. */
    fun line(windowMinutes: Int?): String {
        val span = if (windowMinutes != null) "in the last $windowMinutes minutes" else "since launch"
        return "UI thread: worst ${worstMs.toInt()} ms late $span, $late late ping${if (late == 1) "" else "s"}"
    }
}

/**
 * How late the UI thread was, from the watchdog's pings. Pure, so it can be tested: the clock is passed in
 * and the rolling window drops pings older than [windowMs].
 */
class LatenessStats(val thresholdMs: Double, val windowMs: Double = 600_000.0) {
    var samples = 0; private set
    var late = 0; private set
    var worstMs = 0.0; private set
    var totalLateMs = 0.0; private set
    private val recent = ArrayDeque<Pair<Double, Double>>()

    /** Record one ping that came back [ms] late at clock time [atMs]; true when over the threshold. */
    fun record(ms: Double, atMs: Double = 0.0): Boolean {
        samples += 1
        worstMs = maxOf(worstMs, ms)
        recent.addLast(atMs to ms)
        while (recent.isNotEmpty() && atMs - recent.first().first > windowMs) recent.removeFirst()
        if (ms <= thresholdMs) return false
        late += 1
        totalLateMs += ms
        return true
    }

    val sinceLaunch: LatenessWindow get() = LatenessWindow(samples, late, worstMs, totalLateMs)

    /** The pings in the last [windowMs] before [now]. */
    fun window(now: Double): LatenessWindow {
        var w = LatenessWindow()
        for ((at, ms) in recent) {
            if (now - at > windowMs) continue
            w = w.copy(
                pings = w.pings + 1,
                worstMs = maxOf(w.worstMs, ms),
                late = w.late + if (ms > thresholdMs) 1 else 0,
                totalLateMs = w.totalLateMs + if (ms > thresholdMs) ms else 0.0,
            )
        }
        return w
    }

    val summary: String
        get() = "ui thread: $samples pings, $late late (> ${thresholdMs.toInt()} ms), worst ${worstMs.toInt()} ms, ${totalLateMs.toInt()} ms late in all"
}

/** How often the watchdog pings and what it calls late. Release: 250 ms / 100 ms; debug (`MASON_WATCHDOG=1`): 20 ms / 50 ms. */
data class WatchdogConfig(val intervalMs: Long, val thresholdMs: Double, val logAboveMs: Double) {
    companion object {
        val RELEASE = WatchdogConfig(250, 100.0, 1000.0)
        val DEBUG = WatchdogConfig(20, 50.0, 50.0)
        fun current(env: (String) -> String? = System::getenv) = if (env("MASON_WATCHDOG") == "1") DEBUG else RELEASE
    }
}

/**
 * A low-priority daemon thread asks the UI thread (through [post]) to answer every interval and keeps how
 * late the answers were, since launch and over the last 10 minutes. It runs in every build; the numbers are
 * shown on the Health page, and a ping more than a second late is logged to stderr.
 */
class UiWatchdog(
    private val post: (() -> Unit) -> Unit,
    private val config: WatchdogConfig = WatchdogConfig.current(),
    private val clockMs: () -> Double = { System.nanoTime() / 1e6 },
    private val log: (String) -> Unit = { System.err.println("[watchdog] $it") },
) {
    private val stats = LatenessStats(config.thresholdMs)
    @Volatile private var running = false

    val isRunning: Boolean get() = running
    val report: String get() = synchronized(stats) { stats.summary }
    fun recent(): LatenessWindow = synchronized(stats) { stats.window(clockMs()) }

    fun start() {
        if (running) return
        running = true
        Thread({
            while (running) {
                val sent = clockMs()
                post {
                    val ms = clockMs() - sent
                    synchronized(stats) { stats.record(ms, clockMs()) }
                    if (ms > config.logAboveMs) log("ui thread late by ${ms.toInt()} ms")
                }
                Thread.sleep(config.intervalMs)
            }
        }, "mason-watchdog").apply { isDaemon = true; priority = Thread.MIN_PRIORITY; start() }
    }

    fun stop() { running = false }
}

/** Where the Health page reads the watchdog's numbers; the app points it at its watchdog. */
object UiLateness {
    @Volatile var recent: () -> LatenessWindow = { LatenessWindow() }
}
