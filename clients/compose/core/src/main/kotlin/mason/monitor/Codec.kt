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
) {
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
            )
        }
    }
}

/** The daemon's answer to a publish preview: the redacted text, and what it would touch. */
data class PublishPreview(
    val title: String = "",
    val body: String = "",
    val head: String = "",
    val base: String = "",
    val files: List<String> = emptyList(),
    val redactions: Int = 0,
    val blocked: String? = null,
    val pr: Int? = null,
) {
    companion object {
        fun parse(json: String): PublishPreview {
            val v = Codec.json(json) ?: return PublishPreview()
            return PublishPreview(
                title = v["title"] as? String ?: "",
                body = v["body"] as? String ?: "",
                head = v["head"] as? String ?: "",
                base = v["base"] as? String ?: "",
                files = (v["files"] as? List<*>)?.mapNotNull { it as? String } ?: emptyList(),
                redactions = (v["redactions"] as? Number)?.toInt() ?: 0,
                blocked = v["blocked"] as? String,
                pr = (v["pr"] as? Number)?.toInt(),
            )
        }
    }
}
