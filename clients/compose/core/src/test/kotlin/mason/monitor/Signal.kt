// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import java.util.concurrent.locks.ReentrantLock
import kotlin.concurrent.withLock

/**
 * What a test waits on instead of sleeping: the fake sockets and the discover callback call
 * [signal] each time they are used, and [waitFor] re-checks its condition on every signal.
 * There is no deadline: how long a tick takes does not matter, and a condition that never
 * comes true is a hang that the CI job's own limit reports.
 */
class Signal {
    private val lock = ReentrantLock()
    private val changed = lock.newCondition()
    private var count = 0L

    fun signal() = lock.withLock { count++; changed.signalAll() }

    fun waitFor(f: () -> Boolean) = lock.withLock {
        while (!f()) {
            val seen = count
            while (count == seen && !f()) changed.await()
        }
    }
}

/** The one signal the Compose tests share; each test re-checks its own condition on every wake. */
val testSignal = Signal()
