// SPDX-License-Identifier: GPL-3.0-or-later
package mason.android

import mason.monitor.PairedDaemon
import mason.monitor.Pin

/** A paired daemon as kept on the phone. The token is not here: it is sealed in the token store, by fingerprint. */
data class PairedRecord(
    val host: String, val port: Int, val fingerprint: String, val role: String,
    val alternates: List<String> = emptyList(),
) {
    constructor(d: PairedDaemon) : this(d.host, d.port, d.fingerprint, d.role, d.alternates)

    fun withToken(token: String) = PairedDaemon(host, port, fingerprint, token, role, alternates)
}

/** Where the one paired daemon is remembered. Pairing writes it; a revoked token clears it. */
interface PairedStore {
    fun load(): PairedRecord?
    fun save(r: PairedRecord)
    fun clear()
}

class MemoryPairedStore(var record: PairedRecord? = null) : PairedStore {
    override fun load() = record
    override fun save(r: PairedRecord) { record = r }
    override fun clear() { record = null }
}

/** One value in [kv]: host, port, fingerprint, role, then each extra host, a line each. One daemon at a time. */
class KeyValuePairedStore(private val kv: KeyValue) : PairedStore {
    override fun load(): PairedRecord? {
        val lines = kv.get(KEY)?.split("\n") ?: return null
        if (lines.size < 4) return null
        val port = lines[1].toIntOrNull()?.takeIf { it in 1..65535 } ?: return null
        val fp = lines[2].lowercase().takeIf(Pin::isFingerprint) ?: return null
        if (lines[0].isEmpty()) return null
        return PairedRecord(lines[0], port, fp, lines[3], lines.drop(4).filter { it.isNotEmpty() })
    }

    override fun save(r: PairedRecord) {
        kv.put(KEY, (listOf(r.host, r.port.toString(), r.fingerprint, r.role) + r.alternates).joinToString("\n"))
    }

    override fun clear() = kv.put(KEY, null)

    companion object { const val KEY = "mason.paired-daemon" }
}
