// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

/** Whether the owner could see the app when a ping was answered. */
enum class AppVisibility {
    /** The app's window has focus. */
    FRONTMOST,
    /** On screen but without focus: drawing is still seen. */
    VISIBLE,
    /** Hidden or minimised: lateness here is not slow drawing. */
    HIDDEN;

    val isBackground: Boolean get() = this == HIDDEN
}

/** The numbers for one window of the watchdog's pings. */
data class LatenessWindow(val pings: Int = 0, val late: Int = 0, val worstMs: Double = 0.0, val totalLateMs: Double = 0.0) {
    /** The Health page's line; the Swift monitor prints the same text. */
    fun line(windowMinutes: Int?): String {
        val span = if (windowMinutes != null) "in the last $windowMinutes minutes" else "since launch"
        return "UI thread: worst ${worstMs.toInt()} ms late $span, $late late ping${if (late == 1) "" else "s"}"
    }

    /** The smaller line for stalls taken in the background; the Swift monitor prints the same text. */
    fun backgroundLine(windowMinutes: Int?): String {
        val span = if (windowMinutes != null) "in the last $windowMinutes minutes" else "since launch"
        return "In the background: worst ${worstMs.toInt()} ms late $span, $late late ping${if (late == 1) "" else "s"}"
    }
}

/**
 * How late the UI thread was, from the watchdog's pings. Pure, so it can be tested: the clock is passed in
 * and the rolling window drops pings older than [windowMs].
 */
class LatenessStats(
    val thresholdMs: Double,
    val windowMs: Double = WINDOW_MS,
) {
    companion object { const val WINDOW_MS = 10 * 60_000.0 }
    var samples = 0; private set
    var late = 0; private set
    var worstMs = 0.0; private set
    var totalLateMs = 0.0; private set
    private var bgSamples = 0
    private var bgLate = 0
    private var bgWorstMs = 0.0
    private var bgTotalLateMs = 0.0
    private class Sample(val at: Double, val ms: Double, val background: Boolean)
    private val recent = ArrayDeque<Sample>()

    /**
     * Record one ping that came back [ms] late at clock time [atMs]; true when over the threshold. Samples
     * taken while the app was hidden count in the background numbers, not the visible ones.
     */
    fun record(ms: Double, atMs: Double = 0.0, app: AppVisibility = AppVisibility.FRONTMOST): Boolean {
        val bg = app.isBackground
        recent.addLast(Sample(atMs, ms, bg))
        while (recent.isNotEmpty() && atMs - recent.first().at > windowMs) recent.removeFirst()
        if (bg) {
            bgSamples += 1
            bgWorstMs = maxOf(bgWorstMs, ms)
            if (ms <= thresholdMs) return false
            bgLate += 1
            bgTotalLateMs += ms
            return true
        }
        samples += 1
        worstMs = maxOf(worstMs, ms)
        if (ms <= thresholdMs) return false
        late += 1
        totalLateMs += ms
        return true
    }

    val sinceLaunch: LatenessWindow get() = LatenessWindow(samples, late, worstMs, totalLateMs)

    val backgroundSinceLaunch: LatenessWindow get() = LatenessWindow(bgSamples, bgLate, bgWorstMs, bgTotalLateMs)

    /** The visible pings in the last [windowMs] before [now]. */
    fun window(now: Double): LatenessWindow = window(now, background = false)

    /** The background pings in the last [windowMs] before [now]. */
    fun backgroundWindow(now: Double): LatenessWindow = window(now, background = true)

    private fun window(now: Double, background: Boolean): LatenessWindow {
        var w = LatenessWindow()
        for (r in recent) {
            val at = r.at; val ms = r.ms
            if (now - at > windowMs || r.background != background) continue
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
 * late the answers were, since launch and over the last 10 minutes. Only one ping is in flight at a time,
 * so one blocked UI thread is one sample, not one per tick. It runs in every build; the numbers are shown
 * on the Health page, and a stall of more than a second is logged to stderr. [appState] is read on the UI
 * thread when a ping is answered.
 */
class UiWatchdog(
    private val post: (() -> Unit) -> Unit,
    private val config: WatchdogConfig = WatchdogConfig.current(),
    private val clockMs: () -> Double = { System.nanoTime() / 1e6 },
    private val log: (String) -> Unit = { System.err.println("[watchdog] $it") },
    private val appState: () -> AppVisibility = { UiLateness.appState() },
) {
    private val stats = LatenessStats(config.thresholdMs)
    @Volatile private var running = false
    private val inFlight = java.util.concurrent.atomic.AtomicBoolean(false)

    val isRunning: Boolean get() = running
    val report: String get() = synchronized(stats) { stats.summary }
    fun recent(): LatenessWindow = synchronized(stats) { stats.window(clockMs()) }
    fun recentBackground(): LatenessWindow = synchronized(stats) { stats.backgroundWindow(clockMs()) }

    /** One tick: send a ping unless the last one has not come back yet. */
    fun tick() {
        if (!inFlight.compareAndSet(false, true)) return
        val sent = clockMs()
        post {
            val ms = clockMs() - sent
            val app = appState()
            inFlight.set(false)
            synchronized(stats) { stats.record(ms, clockMs(), app) }
            if (ms > config.logAboveMs) {
                log("ui thread blocked for ${ms.toInt()} ms" + if (app.isBackground) " (app in background)" else "")
            }
        }
    }

    fun start() {
        if (running) return
        running = true
        Thread({
            while (running) {
                tick()
                Thread.sleep(config.intervalMs)
            }
        }, "mason-watchdog").apply { isDaemon = true; priority = Thread.MIN_PRIORITY; start() }
    }

    fun stop() { running = false }
}

/** Where the Health page reads the watchdog's numbers; the app points it at its watchdog. */
object UiLateness {
    @Volatile var recent: () -> LatenessWindow = { LatenessWindow() }
    @Volatile var recentBackground: () -> LatenessWindow = { LatenessWindow() }
    /** The app points this at its window; read on the UI thread. */
    @Volatile var appState: () -> AppVisibility = { AppVisibility.FRONTMOST }
}
