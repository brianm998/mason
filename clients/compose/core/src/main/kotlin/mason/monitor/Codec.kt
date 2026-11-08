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

/**
 * The Health page's introspection half (Part 14): what the detectors and the analyst found, what
 * came of the fixes, the recurring jobs, the triage review queue and the digest.
 */
data class IntrospectionSnapshot(
    val findings: List<Finding> = emptyList(),
    val jobs: List<Job> = emptyList(),
    val triageOn: Boolean = false,
    val triageWaiting: Int = 0,
    /** Lines of the digest, each a figure computed from recorded data. */
    val digest: List<String> = emptyList(),
) {
    data class Finding(val id: Int, val title: String, val metric: String, val task: Int, val verdict: String, val text: String)
    data class Job(val name: String, val kind: String, val runs: Int, val status: String)

    companion object {
        @Suppress("UNCHECKED_CAST")
        fun parse(json: String): IntrospectionSnapshot {
            val v = Codec.json(json) ?: return IntrospectionSnapshot()
            fun list(m: Map<String, Any?>, k: String) = (m[k] as? List<Map<String, Any?>>) ?: emptyList()
            fun int(x: Any?) = (x as? Number)?.toInt() ?: 0
            val t = v["triage"] as? Map<String, Any?> ?: emptyMap()
            val g = v["digest"] as? Map<String, Any?> ?: emptyMap()
            val ex = g["exploring"] as? Map<String, Any?> ?: emptyMap()
            val lines = mutableListOf("Landed: ${int(g["landed"])}")
            (g["median_claim_to_land_ms"] as? Number)?.let { lines += "Median time from claim to land: ${it.toLong() / 1000} s" }
            (g["interventions_per_landed"] as? Number)?.let { lines += "Interventions per landed task: ${"%.2f".format(it.toDouble())}" }
            lines += "Tokens per landed task: not recorded"
            lines += "Exploring skipped for ${int(ex["skipped"])} task(s); ${int(ex["came_back"])} came back"
            (ex["by_route"] as? Map<String, Map<String, Any?>>)?.toSortedMap()?.forEach { (route, r) ->
                lines += "  $route: ${int(r["tasks"])} task(s), ${int(r["came_back"])} came back"
            }
            list(g, "forced_landings").forEach { lines += "Forced landing, task #${int(it["task"])}: ${it["reason"] ?: ""}" }
            return IntrospectionSnapshot(
                findings = list(v, "findings").map {
                    Finding(int(it["id"]), it["title"] as? String ?: "?", it["metric"] as? String ?: "?", int(it["task"]), it["verdict"] as? String ?: "open", it["text"] as? String ?: "")
                },
                jobs = list(v, "schedules").map {
                    Job(it["name"] as? String ?: "?", it["kind"] as? String ?: "?", int(it["runs"]), it["last_status"] as? String ?: "")
                },
                triageOn = t["enabled"] as? Boolean ?: false,
                triageWaiting = (t["queue"] as? List<*>)?.size ?: 0,
                digest = lines,
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
