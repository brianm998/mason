// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener

/**
 * What `daemon.json` says about the running daemon. `pid` and `startedMs` are null
 * when the port was given by hand (`--port`), so there is nothing to watch.
 */
data class DaemonInfo(val port: Int, val pid: Long? = null, val startedMs: Long? = null) {
    /** A different daemon (or a restarted one) than [old]. */
    fun replaces(old: DaemonInfo): Boolean =
        port != old.port ||
            (pid != null && old.pid != null && pid != old.pid) ||
            (startedMs != null && old.startedMs != null && startedMs != old.startedMs)
}

/** The pause before each retry: 1, 2, 4, 8 seconds, then every 10. A welcome starts it over. */
class ReconnectPolicy(private val scale: Double = 1.0) {
    private var failures = 0

    fun nextPauseSeconds(): Double {
        val base = if (failures < SCHEDULE.size) SCHEDULE[failures] else STEADY
        failures++
        return base * scale
    }

    fun reset() { failures = 0 }

    companion object {
        val SCHEDULE = listOf(1.0, 2.0, 4.0, 8.0)
        const val STEADY = 10.0
    }
}

/** How often the connection looks while it is up. */
data class ConnectionTiming(
    /** Ask the daemon "are you there" this often. */
    val pingEverySeconds: Double = 5.0,
    /** No message and no Pong for this long is a stall: reconnect. */
    val stallAfterSeconds: Double = 15.0,
    /** Re-read `daemon.json` this often while connected. */
    val watchEverySeconds: Double = 2.0,
    /** Multiplies every pause between retries; tests shrink it. */
    val pauseScale: Double = 1.0,
)

/** What a socket tells the connection. */
interface SocketEvents {
    fun onOpen()
    fun onMessage(text: String)
    fun onClosed()
}

/** One WebSocket, as the connection needs it. The real one wraps OkHttp; tests script a fake. */
interface MonitorSocket {
    fun send(text: String): Boolean
    fun close()
}

fun interface SocketFactory {
    fun open(port: Int, events: SocketEvents): MonitorSocket
}

class OkHttpSocketFactory : SocketFactory {
    private val http = OkHttpClient()

    override fun open(port: Int, events: SocketEvents): MonitorSocket {
        val ws = http.newWebSocket(
            Request.Builder().url("ws://127.0.0.1:$port/ws").build(),
            object : WebSocketListener() {
                override fun onOpen(webSocket: WebSocket, response: Response) = events.onOpen()
                override fun onMessage(webSocket: WebSocket, text: String) = events.onMessage(text)
                override fun onClosed(webSocket: WebSocket, code: Int, reason: String) = events.onClosed()
                override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) = events.onClosed()
            },
        )
        return object : MonitorSocket {
            override fun send(text: String) = ws.send(text)
            override fun close() { ws.cancel() }
        }
    }
}
