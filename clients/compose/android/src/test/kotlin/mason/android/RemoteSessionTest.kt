// SPDX-License-Identifier: GPL-3.0-or-later
package mason.android

import java.util.concurrent.CopyOnWriteArrayList
import mason.monitor.*
import kotlin.test.*

class RemoteSessionTest {
    private val fp = "ab".repeat(32)
    private val timing = ConnectionTiming(pingEverySeconds = 0.05, stallAfterSeconds = 15.0, watchEverySeconds = 0.02, pauseScale = 0.01)

    private class Sock(val info: DaemonInfo, val events: SocketEvents, override val refusal: Int?) : MonitorSocket {
        val sent = CopyOnWriteArrayList<String>()
        override fun send(text: String): Boolean { sent += text; return true }
        override fun close() { events.onClosed() }
    }

    private fun waitFor(what: String, f: () -> Boolean) {
        val end = System.currentTimeMillis() + 5000
        while (!f()) {
            if (System.currentTimeMillis() > end) fail("timed out waiting for $what")
            Thread.sleep(10)
        }
    }

    private fun rig(refusal: Int?, sockets: CopyOnWriteArrayList<Sock>): Triple<RemoteSession, MemoryPairedStore, MemoryTokenStore> {
        val store = MemoryPairedStore(PairedRecord("m.local", 7443, fp, "owner", listOf("m.ts.net")))
        val tokens = MemoryTokenStore().also { it.set(fp, "tok") }
        val s = RemoteSession(MonitorModel(MemoryPreferences()), store, tokens, null, socketFor = {
            object : SocketFactory {
                override fun open(port: Int, events: SocketEvents): MonitorSocket = error("unused")
                override fun open(info: DaemonInfo, events: SocketEvents): MonitorSocket =
                    Sock(info, events, refusal).also { sockets += it; if (refusal != null) Thread { events.onClosed() }.start() }
            }
        }, timing = timing)
        return Triple(s, store, tokens)
    }

    @Test fun a401ClearsThePairingAndTheToken() {
        val sockets = CopyOnWriteArrayList<Sock>()
        val (s, store, tokens) = rig(401, sockets)
        assertEquals(RemoteState.Paired, s.state)
        s.activate()
        waitFor("unpaired") { s.state == RemoteState.Unpaired }
        assertNull(store.load())
        assertNull(tokens.token(fp))
        assertEquals(1, sockets.size, "no further attempts with a revoked token")
    }

    @Test fun theBackgroundClosesTheSocketAndTheReturnResumesFromTheLastSequence() {
        val sockets = CopyOnWriteArrayList<Sock>()
        val model = MonitorModel(MemoryPreferences())
        val store = MemoryPairedStore(PairedRecord("m.local", 7443, fp, "owner"))
        val tokens = MemoryTokenStore().also { it.set(fp, "tok") }
        val s = RemoteSession(model, store, tokens, null, socketFor = {
            object : SocketFactory {
                override fun open(port: Int, events: SocketEvents): MonitorSocket = error("unused")
                override fun open(info: DaemonInfo, events: SocketEvents): MonitorSocket = Sock(info, events, null).also { sockets += it }
            }
        }, timing = timing)
        s.activate()
        waitFor("a socket") { sockets.size == 1 }
        assertEquals("m.local", sockets[0].info.host)
        model.lastSeq = 41
        sockets[0].events.onOpen()
        s.deactivate()
        waitFor("closed") { sockets[0].sent.isNotEmpty() }
        s.activate()
        waitFor("a second socket") { sockets.size == 2 }
        sockets[1].events.onOpen()
        assertTrue(Regex("[lL]ast_?[sS]eq\"?:\"?41\\b").containsMatchIn(sockets[1].sent[0]), sockets[1].sent[0])
        s.deactivate()
    }

    @Test fun forgetDeletesTheTokenAndShowsPairing() {
        val sockets = CopyOnWriteArrayList<Sock>()
        val (s, store, tokens) = rig(null, sockets)
        s.unpair()
        assertEquals(RemoteState.Unpaired, s.state)
        assertNull(store.load())
        assertNull(tokens.token(fp))
    }

    @Test fun aMissingTokenMeansPairingAgain() {
        val sockets = CopyOnWriteArrayList<Sock>()
        val (s, store, tokens) = rig(null, sockets)
        tokens.set(fp, null)
        s.activate()
        assertEquals(RemoteState.Unpaired, s.state)
        assertNull(store.load())
        assertTrue(sockets.isEmpty())
    }
}
