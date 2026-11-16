// SPDX-License-Identifier: GPL-3.0-or-later
package mason.android

import mason.monitor.DaemonInfo
import mason.monitor.PairedDaemon
import mason.monitor.Pin
import mason.monitor.Reach
import mason.monitor.ServiceRecord

/** What the network browse has found so far. The real one is [NsdSource]; tests script one. */
interface ServiceSource {
    fun services(): List<ResolvedService>
}

/** One address to try. */
data class Endpoint(val host: String, val port: Int, val viaNsd: Boolean)

/**
 * Chooses the endpoint for each connection attempt. It looks at what NSD has found for the paired
 * fingerprint (a record with any other fingerprint is ignored), puts that first, then walks
 * [Reach.order] (the pairing's host and the extra hosts). An attempt that never got a welcome moves
 * to the next endpoint; one that did keeps its endpoint. While an attempt is running the answer does
 * not change, so the connection's watch does not read a re-resolve as a restart.
 */
class RemoteDiscovery(private val paired: PairedDaemon, private val source: ServiceSource?) {
    private var cursor = 0
    private var chosen: Endpoint? = null
    private var onChoose: ((Endpoint, ServiceRecord?) -> Unit)? = null

    @Synchronized fun onChoose(f: (Endpoint, ServiceRecord?) -> Unit) { onChoose = f }

    private fun match(): ResolvedService? =
        source?.services()?.firstOrNull { Pin.matches(it.record.fingerprint, paired.fingerprint) }

    /** Every endpoint, in the order to try them. */
    fun candidates(): List<Endpoint> {
        val found = match()
        return Reach.order(paired, found?.host).map { h ->
            if (found != null && found.host == h) Endpoint(h, found.port, true) else Endpoint(h, paired.port, false)
        }
    }

    /** The endpoint for the attempt in hand; the same until [attemptEnded]. */
    fun current(): DaemonInfo? {
        synchronized(this) { chosen?.let { return DaemonInfo(it.port, host = it.host) } }
        val list = candidates()
        if (list.isEmpty()) return null
        val (e, f) = synchronized(this) {
            val e = list[cursor % list.size]
            chosen = e
            e to onChoose
        }
        f?.invoke(e, match()?.record)
        return DaemonInfo(e.port, host = e.host)
    }

    /** The attempt is over. After one that was never welcomed, the next tries the next endpoint. */
    @Synchronized fun attemptEnded(welcomed: Boolean) {
        if (welcomed) cursor = 0 else cursor++
        chosen = null
    }
}

/** `[::1]` for an IPv6 literal, the host as it is otherwise. */
fun urlHost(host: String) = if (':' in host && !host.startsWith("[")) "[$host]" else host
