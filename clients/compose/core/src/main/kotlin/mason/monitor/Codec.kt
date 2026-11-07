// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import com.squareup.moshi.Moshi
import com.squareup.moshi.Types
import com.squareup.wire.WireJsonAdapterFactory
import mason.v1.ClientMessage
import mason.v1.ServerMessage

/** The protocol's JSON encoding (the proto3 mapping), through Wire's generated adapters. */
object Codec {
    private val moshi: Moshi = Moshi.Builder().add(WireJsonAdapterFactory()).build()
    private val server = moshi.adapter(ServerMessage::class.java)
    private val client = moshi.adapter(ClientMessage::class.java)
    private val map = moshi.adapter<Map<String, Any?>>(
        Types.newParameterizedType(Map::class.java, String::class.java, Any::class.java)
    )

    fun decode(text: String): ServerMessage? = runCatching { server.fromJson(text) }.getOrNull()
    fun encode(m: ClientMessage): String = client.toJson(m)
    fun json(text: String): Map<String, Any?>? = runCatching { map.fromJson(text) }.getOrNull()
}

/** The Health page: resources and who holds them, from the daemon's own report. */
data class HealthSnapshot(
    val resources: List<Resource> = emptyList(),
    val waiting: List<String> = emptyList(),
    val holders: List<String> = emptyList(),
    /** Overall CPU use now, if the daemon could read it. */
    val cpuBusy: Int? = null,
    /** A compile starts only while CPU use is below this percent. */
    val compileCpuMax: Int = 50,
) {
    /** The CPU line of the Health page. */
    val cpuLine: String
        get() = cpuBusy?.let { "CPU $it% busy; a compile starts below $compileCpuMax%." }
            ?: "CPU not readable; a compile starts below $compileCpuMax%."

    data class Resource(val name: String, val kind: String, val used: Int, val limit: Int)

    companion object {
        @Suppress("UNCHECKED_CAST")
        fun parse(json: String): HealthSnapshot {
            val v = Codec.json(json) ?: return HealthSnapshot()
            fun list(k: String) = (v[k] as? List<Map<String, Any?>>) ?: emptyList()
            return HealthSnapshot(
                resources = list("resources").map {
                    Resource(
                        it["name"] as? String ?: "?",
                        it["kind"] as? String ?: "?",
                        (it["used"] as? Number)?.toInt() ?: 0,
                        (it["limit"] as? Number)?.toInt() ?: 0,
                    )
                },
                waiting = list("waiting").map { "${it["label"] ?: "?"}: ${it["explain"] ?: ""}" },
                holders = list("holders").map { it["label"] as? String ?: "?" },
                cpuBusy = ((v["cpu"] as? Map<String, Any?>)?.get("busy_pct") as? Number)?.toInt(),
                compileCpuMax = ((v["cpu"] as? Map<String, Any?>)?.get("compile_max_pct") as? Number)?.toInt() ?: 50,
            )
        }
    }
}
