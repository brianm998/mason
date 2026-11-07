// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import mason.v1.*
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import java.util.concurrent.CountDownLatch
import kotlin.concurrent.thread
import kotlin.math.min

/**
 * The monitor's one WebSocket to `masond`. It says hello with the last sequence
 * number it saw, so a reconnect costs only what was missed, and it reconnects by
 * itself with a growing pause. The protocol is the JSON encoding.
 */
class MonitorConnection(private val model: MonitorModel, private val port: Int) {
    private val http = OkHttpClient()
    @Volatile private var socket: WebSocket? = null
    @Volatile private var running = false
    private var nextId = 1

    init {
        model.sender = ::send
        model.subscribe = ::sendSubscription
    }

    fun start() {
        if (running) return
        running = true
        thread(isDaemon = true, name = "mason-connection") {
            var pause = 250L
            while (running) {
                val closed = CountDownLatch(1)
                val listener = object : WebSocketListener() {
                    override fun onOpen(webSocket: WebSocket, response: Response) {
                        write(ClientMessage(hello = Hello(
                            protocol = 1,
                            last_seq = model.lastSeq,
                            client = "mason-monitor-compose",
                            client_version = model.ownVersion,
                            actor = model.identity,
                        )))
                    }

                    override fun onMessage(webSocket: WebSocket, text: String) {
                        val msg = Codec.decode(text) ?: return
                        if (msg.welcome != null) {
                            pause = 250L
                            model.apply(msg)
                            sendSubscription()
                        } else {
                            model.apply(msg)
                        }
                    }

                    override fun onClosed(webSocket: WebSocket, code: Int, reason: String) {
                        model.disconnected(); closed.countDown()
                    }

                    override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
                        model.disconnected(); closed.countDown()
                    }
                }
                socket = http.newWebSocket(Request.Builder().url("ws://127.0.0.1:$port/ws").build(), listener)
                closed.await()
                if (!running) break
                Thread.sleep(pause)
                pause = min(pause * 2, 5000L)
            }
        }
    }

    fun stop() {
        running = false
        socket?.close(1000, null)
        http.dispatcher.executorService.shutdown()
    }

    /** Which views to keep sending, given the page being shown. */
    fun sendSubscription() {
        val views = mutableListOf(ViewKind.HEADER, ViewKind.SIDEBAR, ViewKind.HOME, ViewKind.USAGE)
        var task = 0L
        when (val p = model.page) {
            is Page.Task -> { views += ViewKind.TASK_DETAIL; task = p.id }
            Page.Landing -> views += ViewKind.LANDING
            Page.Pipeline -> views += ViewKind.EDITOR
            Page.Health -> model.raw("GET", "/resources", id = "health")
            else -> {}
        }
        write(ClientMessage(subscribe = Subscribe(views = views, project = model.project, task = task, editor_step = model.editorStep)))
    }

    private fun send(c: Command) {
        write(ClientMessage(command = if (c.id.isEmpty()) c.copy(id = "m${nextId++}") else c))
    }

    private fun write(m: ClientMessage) {
        socket?.send(Codec.encode(m))
    }
}
