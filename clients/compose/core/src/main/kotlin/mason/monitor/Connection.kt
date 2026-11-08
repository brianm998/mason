// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import mason.v1.*
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import kotlin.concurrent.thread

/**
 * The monitor's one WebSocket to `masond`. It says hello with the last sequence
 * number it saw, so a reconnect costs only what was missed. It reconnects by
 * itself: 1, 2, 4, 8 seconds, then every 10, re-reading `daemon.json` each time,
 * so a restarted daemon is found on its new port. A changed pid or start time,
 * or 15 seconds of silence, also reconnects. The protocol is the JSON encoding.
 */
class MonitorConnection(
    private val model: MonitorModel,
    private val discover: () -> DaemonInfo?,
    private val timing: ConnectionTiming = ConnectionTiming(),
    private val factory: SocketFactory = OkHttpSocketFactory(),
) {
    /** A fixed port, for callers that were given one. */
    constructor(model: MonitorModel, port: Int) : this(model, { DaemonInfo(port) })

    @Volatile private var socket: MonitorSocket? = null
    @Volatile private var running = false
    private var nextId = 1
    private var pings = 0L

    init {
        model.sender = ::send
        model.subscribe = ::sendSubscription
    }

    fun start() {
        if (running) return
        running = true
        thread(isDaemon = true, name = "mason-connection") { loop() }
    }

    fun stop() {
        running = false
        socket?.close()
    }

    private fun loop() {
        val policy = ReconnectPolicy(timing.pauseScale)
        while (running) {
            val info = discover()
            if (info == null) {
                model.disconnected()
            } else {
                if (session(info)) policy.reset()
                model.disconnected()
            }
            if (!running) break
            Thread.sleep((policy.nextPauseSeconds() * 1000).toLong().coerceAtLeast(1))
        }
    }

    /** One connection, from open to the first failure. True if the daemon welcomed us. */
    private fun session(info: DaemonInfo): Boolean {
        val closed = CountDownLatch(1)
        val heard = java.util.concurrent.atomic.AtomicLong(System.nanoTime())
        val welcomed = java.util.concurrent.atomic.AtomicBoolean(false)
        val events = object : SocketEvents {
            override fun onOpen() {
                write(ClientMessage(hello = Hello(
                    protocol = 1,
                    last_seq = model.lastSeq,
                    client = "mason-monitor-compose",
                    client_version = model.ownVersion,
                    actor = model.identity,
                )))
            }

            override fun onMessage(text: String) {
                heard.set(System.nanoTime())
                val msg = Codec.decode(text) ?: return
                if (msg.welcome != null) {
                    welcomed.set(true)
                    model.apply(msg)
                    sendSubscription()
                } else {
                    model.apply(msg)
                }
            }

            override fun onClosed() { closed.countDown() }
        }
        val s = factory.open(info.port, events)
        socket = s
        val tickMs = (minOf(timing.watchEverySeconds, timing.pingEverySeconds) * 1000).toLong().coerceAtLeast(1)
        var lastPing = System.nanoTime()
        while (running && !closed.await(tickMs, TimeUnit.MILLISECONDS)) {
            val now = discover()
            if (now != null && now.replaces(info)) break  // restarted: start over from the last seq
            val quiet = (System.nanoTime() - heard.get()) / 1e9
            if (quiet > timing.stallAfterSeconds) break  // a stall: show reconnecting
            if ((System.nanoTime() - lastPing) / 1e9 >= timing.pingEverySeconds) {
                lastPing = System.nanoTime()
                write(ClientMessage(ping = Ping(nonce = ++pings)))
            }
        }
        s.close()
        return welcomed.get()
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
