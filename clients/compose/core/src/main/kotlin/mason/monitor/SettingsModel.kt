// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

/** The ids of the commands whose answer the Settings page reports. */
internal val SETTINGS_RESULTS = setOf("settings-save", "notify-save", "notify-test", "schedule-run", "remote-save", "review-save", "review-run")

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

/**
 * `GET /remote`: where runs go (#831). The global mode, and for each project whether it has
 * `[remote_runs]`, where it runs, its effective mode and why ([Project.source]). Every project can
 * be set: one with no `[remote_runs]` and a mode of its own sends its runs to nodes only (#1150).
 */
data class RemoteSettings(
    val mode: String = "local",
    val problem: String? = null,
    val changedBy: String? = null,
    val changedAt: String? = null,
    val projects: List<Project> = emptyList(),
) {
    /** [source] is `project`, `global` or `no [remote_runs]`. */
    /** [nodesOnly]: no `[remote_runs]`, but a mode of its own (`[placement]`). */
    data class Project(
        val project: String, val remote: Boolean, val runsOn: String, val mode: String, val source: String,
        val nodesOnly: Boolean = false,
    ) {
        val sourceText: String
            get() = if (nodesOnly) "$mode (nodes only: no GitHub setup)" else when (source) {
                "project" -> "set for this project"
                "global" -> "follows the global mode"
                else -> "always runs here: the project file has no [remote_runs]; a mode chosen here sends its runs to nodes"
            }
    }

    fun project(name: String): Project? = projects.firstOrNull { it.project == name }

    val changedText: String?
        get() = changedBy?.let { by -> changedAt?.let { "Changed by $by at $it" } ?: "Changed by $by" }

    companion object {
        val MODES = listOf("local", "remote", "favor-local", "favor-remote")
        const val EXPLANATION = "local: this machine. remote: GitHub. favor-local / favor-remote: prefer one, spill to the other."

        @Suppress("UNCHECKED_CAST")
        fun parse(json: String): RemoteSettings {
            val v = Codec.json(json) ?: return RemoteSettings()
            fun text(k: String): String? = when (val x = v[k]) {
                is String -> x.ifEmpty { null }
                is Number -> x.toLong().toString()
                else -> null
            }
            return RemoteSettings(
                mode = v["mode"] as? String ?: "local",
                problem = text("problem"),
                changedBy = text("changed_by"),
                changedAt = text("changed_at"),
                projects = (v["projects"] as? List<Map<String, Any?>> ?: emptyList()).map {
                    Project(
                        it["project"] as? String ?: "?", it["remote"] as? Boolean ?: false,
                        it["runs_on"] as? String ?: "", it["mode"] as? String ?: "local", it["source"] as? String ?: "",
                        it["nodes_only"] as? Boolean ?: false,
                    )
                },
            )
        }
    }
}

/** `GET /review`: the nightly pipeline review's settings, its last and next run. */
data class ReviewSettings(
    val enabled: Boolean = true,
    /** Local time of day, "HH:MM". */
    val at: String = "00:00",
    val everyDays: Int = 1,
    val model: String = "",
    val effort: String = "",
    val maxProposals: Int = 10,
    /** MASON's own project, where the review is filed; empty when none is registered. */
    val project: String = "",
    val problem: String? = null,
    val runningTask: Long? = null,
    val nextRun: String? = null,
    val lastTask: Long? = null,
    val lastAt: String? = null,
    val lastSummary: String = "",
    /** The proposals the last completed review filed. */
    val filed: List<Long> = emptyList(),
    val lastStatus: String = "",
) {
    val everyText: String get() = if (everyDays == 1) "Every day" else "Every $everyDays days"

    /** The task numbers of the last run's proposals, as text a task link is made from. */
    val filedText: String
        get() = if (filed.isEmpty()) "No proposals filed" else "Proposals filed: " + filed.joinToString(", ") { "#$it" }

    val nextText: String
        get() = when {
            runningTask != null -> "Running now: #$runningTask"
            project.isEmpty() -> "No project of MASON's own is registered, so no review runs"
            !enabled -> "Next run: none (off)"
            else -> "Next run: ${nextRun ?: "not known"}"
        }

    /** [at] moved by whole hours and minutes, round the clock. */
    fun atMoved(hours: Int = 0, minutes: Int = 0): String {
        val now = minutes(at) ?: 0
        return timeText(Math.floorMod(now + hours * 60 + minutes, 24 * 60))
    }

    companion object {
        /** The frequencies offered by name, in days. */
        val PRESETS = listOf("Daily" to 1, "Weekly" to 7)
        const val EXPLANATION = "An agent asks where the pipeline was stuck, slow, over-tested, untested, crashing or needing you, and files what it finds as proposals for you to approve."

        /** "HH:MM" as minutes after midnight; null when it is not a time. */
        fun minutes(at: String): Int? {
            val p = at.split(":")
            if (p.size != 2) return null
            val h = p[0].toIntOrNull() ?: return null
            val m = p[1].toIntOrNull() ?: return null
            return if (h in 0..23 && m in 0..59) h * 60 + m else null
        }

        fun timeText(minutes: Int): String = "${(minutes / 60).toString().padStart(2, '0')}:${(minutes % 60).toString().padStart(2, '0')}"

        @Suppress("UNCHECKED_CAST")
        fun parse(json: String): ReviewSettings {
            val v = Codec.json(json) ?: return ReviewSettings()
            val s = v["settings"] as? Map<String, Any?> ?: emptyMap()
            val l = v["last"] as? Map<String, Any?>
            fun long(x: Any?): Long? = (x as? Number)?.toLong()
            return ReviewSettings(
                enabled = s["enabled"] as? Boolean ?: true,
                at = s["at"] as? String ?: "00:00",
                everyDays = (s["every_days"] as? Number)?.toInt() ?: 1,
                model = s["model"] as? String ?: "",
                effort = s["effort"] as? String ?: "",
                maxProposals = (s["max_proposals"] as? Number)?.toInt() ?: 10,
                project = v["project"] as? String ?: "",
                problem = (v["problem"] as? String)?.ifEmpty { null },
                runningTask = long(v["running_task"]),
                nextRun = v["next_run_text"] as? String,
                lastTask = long(l?.get("task")),
                lastAt = l?.get("completed_text") as? String,
                lastSummary = l?.get("summary") as? String ?: "",
                filed = (l?.get("filed") as? List<Any?> ?: emptyList()).mapNotNull { long(it) },
                lastStatus = v["last_status"] as? String ?: "",
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

    /** `POST /remote`: the global mode, or one project's when [project] is given. */
    fun remoteMode(mode: String, project: String? = null) =
        Codec.body(if (project == null) mapOf("mode" to mode) else mapOf("mode" to mode, "project" to project))

    /** `POST /review`: one setting of the pipeline review. */
    fun reviewEnabled(on: Boolean) = Codec.body(mapOf("enabled" to on))
    fun reviewAt(at: String) = Codec.body(mapOf("at" to at))
    fun reviewEvery(days: Int) = Codec.body(mapOf("every_days" to days))

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
        "schedule-run", "review-run" -> "Started."
        else -> "Saved. It applies now, with no restart."
    }
}
