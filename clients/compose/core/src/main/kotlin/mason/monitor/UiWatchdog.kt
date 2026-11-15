// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

/** How late the UI thread was, from the watchdog's pings. Pure, so it can be tested. */
class LatenessStats(val thresholdMs: Double) {
    var samples = 0; private set
    var late = 0; private set
    var worstMs = 0.0; private set
    var totalLateMs = 0.0; private set

    /** Record one ping that came back [ms] late; true when over the threshold. */
    fun record(ms: Double): Boolean {
        samples += 1
        worstMs = maxOf(worstMs, ms)
        if (ms <= thresholdMs) return false
        late += 1
        totalLateMs += ms
        return true
    }

    val summary: String
        get() = "ui thread: $samples pings, $late late (> ${thresholdMs.toInt()} ms), worst ${worstMs.toInt()} ms, ${totalLateMs.toInt()} ms late in all"
}

/**
 * A debug aid: a daemon thread asks the UI thread (through [post]) to answer every [intervalMs] and logs
 * when the answer is more than [thresholdMs] late. Runs only when `MASON_WATCHDOG=1` is set.
 */
class UiWatchdog(
    private val post: (() -> Unit) -> Unit,
    private val intervalMs: Long = 20,
    thresholdMs: Double = 50.0,
    private val log: (String) -> Unit = { System.err.println("[watchdog] $it") },
) {
    private val stats = LatenessStats(thresholdMs)
    @Volatile private var running = false

    val report: String get() = synchronized(stats) { stats.summary }

    fun start() {
        if (running || System.getenv("MASON_WATCHDOG") != "1") return
        running = true
        Thread({
            while (running) {
                val sent = System.nanoTime()
                post {
                    val ms = (System.nanoTime() - sent) / 1e6
                    val over = synchronized(stats) { stats.record(ms) }
                    if (over) log("ui thread late by ${ms.toInt()} ms")
                }
                Thread.sleep(intervalMs)
            }
        }, "mason-watchdog").apply { isDaemon = true; start() }
    }

    fun stop() { running = false }
}
