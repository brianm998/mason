// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import org.json.JSONObject

/*
 * Sound and speech (Part 19), without any sound hardware: a queue of announcements behind a
 * SpeechSink. The app's sink plays real sounds and speaks with the OS; tests use a fake.
 */

/** One thing to say: a short sound, then one sentence. */
data class Announcement(
    val id: String,
    /** The condition (needs_you, failed, landed, daemon_down, ...). */
    val kind: String,
    val sentence: String,
    val sound: Boolean = true,
    val speech: Boolean = true,
) {
    /** Which family of sound goes with this: "wants a decision" and "broke" differ. */
    val family: String
        get() = when (kind) {
            "needs_you" -> "decision"
            "failed", "landing", "daemon_down", "credentials", "provider_outage", "environment", "runtime", "definition" -> "broke"
            else -> "info"
        }
}

/**
 * What actually makes noise. Each call finishes by calling `then` exactly once, when the sound or
 * the sentence has ended; the queue never starts the next thing before that.
 */
interface SpeechSink {
    fun play(sound: String, then: () -> Unit)
    fun speak(sentence: String, voice: String, then: () -> Unit)
    /** Cut off whatever is playing (the user muted). */
    fun stop()
    /** The voices the OS offers, by name. */
    fun voices(): List<String>
}

/** Makes no noise and completes at once (the default until the app supplies a sink). */
class SilentSink : SpeechSink {
    override fun play(sound: String, then: () -> Unit) = then()
    override fun speak(sentence: String, voice: String, then: () -> Unit) = then()
    override fun stop() {}
    override fun voices(): List<String> = emptyList()
}

/** Which announcements are voiced, which sound each family uses, and which voice speaks. */
data class SpeechSettings(
    /** Conditions that make noise. Everything else stays visual. */
    val kinds: Set<String> = DEFAULT_KINDS,
    /** Sound name per family (decision, broke, info). */
    val sounds: Map<String, String> = mapOf("decision" to "Glass", "broke" to "Basso", "info" to "Tink"),
    /** An OS voice name; empty for the system default. */
    val voice: String = "",
) {
    fun sound(a: Announcement): String = sounds[a.family] ?: "Tink"

    companion object {
        val DEFAULT_KINDS = setOf("needs_you", "failed", "landing", "daemon_down", "credentials", "provider_outage", "landed")
    }
}

/** How long the connection must be down before the monitor raises "daemon down" itself. */
const val DAEMON_DOWN_AFTER_MS = 15_000L
const val DAEMON_DOWN_SENTENCE = "MASON has lost its daemon; check that masond is running."

class Announcer(
    /** What makes the noise. The app replaces the silent default with a real one. */
    var sink: SpeechSink = SilentSink(),
    /** Mute is the user's; the announcer only obeys it. It silences sound and speech and nothing else. */
    var isMuted: () -> Boolean = { false },
    private val prefs: Preferences? = null,
) {
    var settings by mutableStateOf(prefs?.let(::load) ?: SpeechSettings())
        private set
    private val queueItems = ArrayDeque<Announcement>()
    val queue: List<Announcement> @Synchronized get() = queueItems.toList()
    @get:Synchronized var speaking: Announcement? = null
        private set
    /** Set once the daemon has answered a claim: from then on its announcements are the sound,
     *  and the monitor's own chime for a newly waiting task would only double it. */
    var heardFromDaemon = false
        private set
    /** This monitor is the one that speaks right now. */
    var isSpeaker by mutableStateOf(false)
        private set
    /** The local alert: the connection has been lost for DAEMON_DOWN_AFTER_MS. Shown as a banner
     *  whether or not anything is muted. */
    var daemonDown by mutableStateOf(false)
        private set
    private var lostSince: Long? = null
    private val seen = mutableSetOf<String>()

    fun update(s: SpeechSettings) {
        settings = s
        prefs?.let { save(it, s) }
    }

    // ---- the queue ----

    /** Add to the end. Muted, or not a kind that speaks, it makes no noise (the visual alert does
     *  not come through here). The same id is voiced once. */
    @Synchronized
    fun enqueue(a: Announcement) {
        if (!seen.add(a.id)) return
        if (isMuted() || a.kind !in settings.kinds) return
        queueItems.addLast(a)
        pump()
    }

    private fun pump() {
        if (speaking != null || queueItems.isEmpty()) return
        val a = queueItems.removeFirst()
        speaking = a
        val speakIt = {
            if (!a.speech || a.sentence.isEmpty()) finish(a)
            else sink.speak(a.sentence, settings.voice) { finish(a) }
        }
        if (a.sound) sink.play(settings.sound(a)) { speakIt() } else speakIt()
    }

    @Synchronized
    private fun finish(a: Announcement) {
        if (speaking != a) return
        speaking = null
        pump()
    }

    /** The user muted: stop what is playing and forget what was waiting. */
    @Synchronized
    fun silence() {
        queueItems.clear()
        if (speaking != null) sink.stop()
        speaking = null
    }

    // ---- the daemon's announcements ----

    fun claimBody(client: String): String =
        JSONObject().put("client", client).put("muted", isMuted()).toString()

    /** The daemon's answer to a claim: whether this monitor speaks, and what to say, in order. */
    @Synchronized
    fun handleClaim(json: String) {
        val v = try { JSONObject(json) } catch (_: Exception) { return }
        heardFromDaemon = true
        isSpeaker = v.optBoolean("speaker", false)
        if (!isSpeaker) return
        val list = v.optJSONArray("announcements") ?: return
        for (i in 0 until list.length()) {
            val x = list.getJSONObject(i)
            enqueue(
                Announcement(
                    id = x.optString("key", x.opt("id")?.toString() ?: ""),
                    kind = x.optString("kind", "info"),
                    sentence = x.optString("sentence", ""),
                    sound = x.optBoolean("sound", true),
                    speech = x.optBoolean("speech", true),
                ),
            )
        }
    }

    // ---- the lost connection ----

    /** The connection went down (call on each failed attempt; only the first sets the clock). */
    @Synchronized
    fun connectionLost(nowMs: Long) { if (lostSince == null) lostSince = nowMs }

    /** The connection is back: the alert clears, and a later loss is a new one. */
    @Synchronized
    fun connectionRestored() {
        lostSince = null
        daemonDown = false
        seen.removeAll { it.startsWith("daemon_down:") }
    }

    /** Look at the clock: after DAEMON_DOWN_AFTER_MS of failed reconnects, raise "daemon down" once. */
    @Synchronized
    fun check(nowMs: Long) {
        val t = lostSince ?: return
        if (daemonDown || nowMs - t < DAEMON_DOWN_AFTER_MS) return
        daemonDown = true
        enqueue(Announcement("daemon_down:$t", "daemon_down", DAEMON_DOWN_SENTENCE))
    }

    // ---- settings ----

    fun availableVoices(): List<String> = sink.voices()

    private fun load(p: Preferences): SpeechSettings {
        val d = SpeechSettings()
        val kinds = p.string("speech.kinds")?.split(",")?.filter { it.isNotEmpty() }?.toSet() ?: d.kinds
        val sounds = d.sounds.mapValues { (f, v) -> p.string("speech.sound.$f") ?: v }
        return SpeechSettings(kinds, sounds, p.string("speech.voice") ?: "")
    }

    private fun save(p: Preferences, s: SpeechSettings) {
        p.setString("speech.kinds", s.kinds.sorted().joinToString(","))
        p.setString("speech.voice", s.voice)
        s.sounds.forEach { (f, n) -> p.setString("speech.sound.$f", n) }
    }
}
