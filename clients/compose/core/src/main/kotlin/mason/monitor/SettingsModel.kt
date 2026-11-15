// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

/** The ids of the commands whose answer the Settings page reports. */
internal val SETTINGS_RESULTS = setOf("settings-save", "notify-save", "notify-test", "schedule-run")

/**
 * The Settings page's data (s430): a project's proposal policy, schedules and tracker, as the
 * daemon reports them. It never holds a secret: the daemon sends only references.
 */
data class SettingsSnapshot(
    val project: String = "",
    /** The four disposition keys, by key. */
    val dispositions: Map<String, String> = emptyMap(),
    val introspection: Map<String, String> = emptyMap(),
    val perTask: Int = 0,
    val perDay: Int = 0,
    val schedules: List<Schedule> = emptyList(),
    val tracker: Tracker? = null,
) {
    data class Schedule(val name: String, val kind: String, val every: String, val title: String)

    /** [token] is `env:NAME`, `file:/path`, or "set in file, not shown"; empty when there is none. */
    data class Tracker(val intervalS: Int, val label: String, val importAs: String, val requireLabel: Boolean, val token: String)

    companion object {
        val DISPOSITION_KEYS = listOf("agent_normal", "agent_blocks", "outside_normal", "outside_blocks")
        val INTROSPECTION_KEYS = listOf("low_small", "low_large", "high_small", "high_large")
        val DISPOSITIONS = listOf("proposed", "queued")

        @Suppress("UNCHECKED_CAST")
        fun parse(json: String): SettingsSnapshot {
            val v = Codec.json(json) ?: return SettingsSnapshot()
            val p = v["proposals"] as? Map<String, Any?> ?: emptyMap()
            val i = p["introspection"] as? Map<String, Any?> ?: emptyMap()
            val t = v["tracker"] as? Map<String, Any?>
            return SettingsSnapshot(
                project = v["project"] as? String ?: "",
                dispositions = DISPOSITION_KEYS.associateWith { p[it] as? String ?: "proposed" },
                introspection = INTROSPECTION_KEYS.associateWith { i[it] as? String ?: "proposed" },
                perTask = (p["per_task"] as? Number)?.toInt() ?: 0,
                perDay = (p["per_day"] as? Number)?.toInt() ?: 0,
                schedules = (v["schedules"] as? List<Map<String, Any?>> ?: emptyList()).map {
                    Schedule(it["name"] as? String ?: "?", it["kind"] as? String ?: "task", it["every"] as? String ?: "", it["title"] as? String ?: "")
                },
                tracker = t?.let {
                    Tracker(
                        (it["interval_s"] as? Number)?.toInt() ?: 60,
                        it["label"] as? String ?: "",
                        it["import_as"] as? String ?: "proposed",
                        it["require_label"] as? Boolean ?: false,
                        it["token"] as? String ?: "",
                    )
                },
            )
        }
    }
}

/** `GET /notify/settings`: where each alert goes. */
data class NotifySettings(
    val routes: List<Route> = emptyList(),
    val channels: List<String> = emptyList(),
    val quiet: Map<String, String> = emptyMap(),
    val reminderAfterS: Int = 0,
    val reminderRepeatS: Int = 0,
    val configured: Map<String, Boolean> = emptyMap(),
    val error: String? = null,
) {
    data class Rule(val channel: String, val afterS: Int)
    data class Route(val condition: String, val rules: List<Rule>)

    /** Does the condition send to the channel? */
    fun sends(condition: String, channel: String): Boolean =
        routes.firstOrNull { it.condition == condition }?.rules?.any { it.channel == channel } ?: false

    companion object {
        @Suppress("UNCHECKED_CAST")
        fun parse(json: String): NotifySettings {
            val v = Codec.json(json) ?: return NotifySettings()
            return NotifySettings(
                routes = (v["routing"] as? List<Map<String, Any?>> ?: emptyList()).map { r ->
                    Route(
                        r["condition"] as? String ?: "?",
                        (r["rules"] as? List<Map<String, Any?>> ?: emptyList()).map {
                            Rule(it["channel"] as? String ?: "?", (it["after_s"] as? Number)?.toInt() ?: 0)
                        },
                    )
                },
                channels = (v["channels"] as? List<Any?> ?: emptyList()).filterIsInstance<String>(),
                quiet = (v["quiet"] as? Map<String, Any?> ?: emptyMap()).mapNotNull { (k, x) -> (x as? String)?.let { k to it } }.toMap(),
                reminderAfterS = (v["reminder_after_s"] as? Number)?.toInt() ?: 0,
                reminderRepeatS = (v["reminder_repeat_s"] as? Number)?.toInt() ?: 0,
                configured = (v["configured"] as? Map<String, Any?> ?: emptyMap()).mapNotNull { (k, x) -> (x as? Boolean)?.let { k to it } }.toMap(),
                error = v["error"] as? String,
            )
        }
    }
}

/** The request bodies the page sends. Each is the smallest change: one key. */
object SettingsPayload {
    fun path(project: String) = "/projects/$project/settings"

    fun disposition(key: String, value: String) = Codec.body(mapOf("proposals" to mapOf(key to value)))
    fun introspection(key: String, value: String) = Codec.body(mapOf("proposals" to mapOf("introspection" to mapOf(key to value))))
    fun cap(key: String, value: Int) = Codec.body(mapOf("proposals" to mapOf(key to value)))
    fun schedule(name: String, every: String) = Codec.body(mapOf("schedules" to mapOf(name to mapOf("every" to every))))
    fun tracker(key: String, value: Any) = Codec.body(mapOf("tracker" to mapOf(key to value)))

    /** Turn a channel on for a condition (`delay` like "0" or "5m"), or off (null). */
    fun route(condition: String, channel: String, delay: String?) =
        Codec.body(mapOf("routing" to mapOf(condition to mapOf(channel to delay))))

    fun quiet(channel: String, window: String?) = Codec.body(mapOf("quiet" to mapOf(channel to window)))
    fun test(channel: String) = Codec.body(mapOf("channel" to channel))
    fun runSchedule(project: String, name: String) = Codec.body(mapOf("name" to "$project/$name"))

    fun done(id: String) = when (id) {
        "notify-test" -> "Test notification sent."
        "schedule-run" -> "Started."
        else -> "Saved. It applies now, with no restart."
    }
}
