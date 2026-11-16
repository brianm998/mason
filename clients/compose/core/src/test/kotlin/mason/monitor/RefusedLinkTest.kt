// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.atomic.AtomicInteger
import kotlin.test.*

/** A remote link: the host chosen for an attempt reaches the factory, and a 401 stops the connection. */
class RefusedLinkTest {
    private class Sock(val info: DaemonInfo, val events: SocketEvents, override val refusal: Int?) : MonitorSocket {
        override fun send(text: String) = true
        override fun close() { events.onClosed() }
    }

    private fun waitFor(what: String, f: () -> Boolean) {
        val end = System.currentTimeMillis() + 5000
        while (!f()) {
            if (System.currentTimeMillis() > end) fail("timed out waiting for $what")
            Thread.sleep(10)
        }
    }

    private val timing = ConnectionTiming(pingEverySeconds = 0.05, stallAfterSeconds = 15.0, watchEverySeconds = 0.02, pauseScale = 0.01)

    @Test fun theChosenHostReachesTheFactoryAndAFailedAttemptIsReported() {
        val opened = CopyOnWriteArrayList<Sock>()
        val ended = CopyOnWriteArrayList<Boolean>()
        // A factory written as an object overrides the host-aware open.
        val factory = object : SocketFactory {
            override fun open(port: Int, events: SocketEvents): MonitorSocket = error("unused")
            override fun open(info: DaemonInfo, events: SocketEvents): MonitorSocket = Sock(info, events, null).also { opened += it }
        }
        val c2 = MonitorConnection(MonitorModel(MemoryPreferences()), { DaemonInfo(7000, host = "m.local") }, timing, factory, onAttemptEnd = { ended += it })
        c2.start()
        waitFor("a socket") { opened.isNotEmpty() }
        assertEquals("m.local", opened[0].info.host)
        assertEquals(7000, opened[0].info.port)
        opened[0].close()
        waitFor("the attempt reported") { ended.isNotEmpty() }
        assertEquals(false, ended[0], "no welcome: the next attempt may try another address")
        c2.stop()
    }

    @Test fun a401StopsTheConnectionAndTellsTheCaller() {
        val opened = AtomicInteger()
        val refused = CopyOnWriteArrayList<Int>()
        val factory = object : SocketFactory {
            override fun open(port: Int, events: SocketEvents): MonitorSocket = error("unused")
            override fun open(info: DaemonInfo, events: SocketEvents): MonitorSocket {
                opened.incrementAndGet()
                return Sock(info, events, 401).also { s -> Thread { events.onClosed() }.start() }
            }
        }
        val c = MonitorConnection(MonitorModel(MemoryPreferences()), { DaemonInfo(7000, host = "m.local") }, timing, factory, onRefused = { refused += it })
        c.start()
        waitFor("the refusal") { refused.isNotEmpty() }
        assertEquals(listOf(401), refused.toList())
        // The loop breaks straight after the callback, so no second attempt can have been opened.
        assertEquals(1, opened.get(), "no retry after a revoked token")
    }

    @Test fun anotherHostIsAReplacement() {
        assertTrue(DaemonInfo(1, host = "b").replaces(DaemonInfo(1, host = "a")))
        assertFalse(DaemonInfo(1, host = "a").replaces(DaemonInfo(1, host = "a")))
        assertFalse(DaemonInfo(1).replaces(DaemonInfo(1, host = "a")))
    }
}
