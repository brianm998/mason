// SPDX-License-Identifier: GPL-3.0-or-later
package mason.android

import android.content.Context
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import mason.monitor.ServiceRecord

/**
 * Looks for `_mason._tcp` daemons with NSD. The record's TXT carries proto, fp and version;
 * a record without a valid fingerprint is ignored. The manifest needs INTERNET, ACCESS_WIFI_STATE
 * and CHANGE_WIFI_MULTICAST_STATE (and NEARBY_WIFI_DEVICES on Android 13+).
 */
class Nsd(context: Context, private val onFound: (ServiceRecord, host: String, port: Int) -> Unit) {
    private val nsd = context.getSystemService(Context.NSD_SERVICE) as NsdManager

    private val listener = object : NsdManager.DiscoveryListener {
        override fun onServiceFound(info: NsdServiceInfo) {
            @Suppress("DEPRECATION")
            nsd.resolveService(info, object : NsdManager.ResolveListener {
                override fun onResolveFailed(i: NsdServiceInfo, code: Int) {}
                override fun onServiceResolved(i: NsdServiceInfo) {
                    val txt = i.attributes.mapValues { String(it.value ?: ByteArray(0)) }
                    val record = ServiceRecord.parse(i.serviceName, txt) ?: return
                    onFound(record, i.host.hostAddress ?: return, i.port)
                }
            })
        }
        override fun onServiceLost(info: NsdServiceInfo) {}
        override fun onDiscoveryStarted(type: String) {}
        override fun onDiscoveryStopped(type: String) {}
        override fun onStartDiscoveryFailed(type: String, code: Int) {}
        override fun onStopDiscoveryFailed(type: String, code: Int) {}
    }

    fun start() = nsd.discoverServices("${ServiceRecord.TYPE}.", NsdManager.PROTOCOL_DNS_SD, listener)
    fun stop() = nsd.stopServiceDiscovery(listener)
}
