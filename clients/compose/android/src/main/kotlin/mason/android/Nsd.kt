// SPDX-License-Identifier: GPL-3.0-or-later
package mason.android

import android.content.Context
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import java.net.Inet4Address
import java.util.ArrayDeque
import mason.monitor.ServiceRecord

/**
 * Looks for `_mason._tcp` daemons with NSD. The record's TXT carries proto, fp and version; a
 * record without a valid fingerprint is ignored. What it has found is [services]. NSD resolves one
 * service at a time, so found services queue.
 */
class NsdSource(context: Context) : ServiceSource {
    private val nsd = context.getSystemService(Context.NSD_SERVICE) as NsdManager
    private val found = linkedMapOf<String, ResolvedService>()
    private val queue = ArrayDeque<NsdServiceInfo>()
    private var resolving = false
    private var running = false

    @Synchronized override fun services(): List<ResolvedService> = found.values.toList()

    private val listener = object : NsdManager.DiscoveryListener {
        override fun onServiceFound(info: NsdServiceInfo) {
            synchronized(this@NsdSource) { queue.add(info) }
            resolveNext()
        }
        override fun onServiceLost(info: NsdServiceInfo) {
            synchronized(this@NsdSource) { found.remove(info.serviceName) }
        }
        override fun onDiscoveryStarted(type: String) {}
        override fun onDiscoveryStopped(type: String) {}
        override fun onStartDiscoveryFailed(type: String, code: Int) { synchronized(this@NsdSource) { running = false } }
        override fun onStopDiscoveryFailed(type: String, code: Int) {}
    }

    private fun resolveNext() {
        val next = synchronized(this) {
            if (resolving) return
            queue.poll()?.also { resolving = true } ?: return
        }
        @Suppress("DEPRECATION")
        nsd.resolveService(next, object : NsdManager.ResolveListener {
            override fun onResolveFailed(i: NsdServiceInfo, code: Int) { done() }
            override fun onServiceResolved(i: NsdServiceInfo) {
                val txt = i.attributes.mapValues { String(it.value ?: ByteArray(0)) }
                val record = ServiceRecord.parse(i.serviceName, txt)
                // A name that works on a LAN: prefer IPv4 (a link-local IPv6 address needs a scope).
                @Suppress("DEPRECATION")
                val host = i.host?.let { h -> if (h is Inet4Address) h.hostAddress else h.hostAddress?.substringBefore('%') }
                if (record != null && host != null) {
                    synchronized(this@NsdSource) { found[i.serviceName] = ResolvedService(record, host, i.port) }
                }
                done()
            }
            private fun done() { synchronized(this@NsdSource) { resolving = false }; resolveNext() }
        })
    }

    fun start() {
        synchronized(this) { if (running) return; running = true }
        nsd.discoverServices("${ServiceRecord.TYPE}.", NsdManager.PROTOCOL_DNS_SD, listener)
    }

    fun stop() {
        synchronized(this) { if (!running) return; running = false; queue.clear() }
        runCatching { nsd.stopServiceDiscovery(listener) }
    }
}
