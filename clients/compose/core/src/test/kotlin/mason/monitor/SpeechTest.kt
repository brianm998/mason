// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import mason.v1.*
import kotlin.test.*

/** Records what it is asked to do and finishes only when the test says so. */
class FakeSink : SpeechSink {
    val log = mutableListOf<String>()
    private val pending = ArrayDeque<() -> Unit>()
    var stopped = 0
    override fun play(sound: String, then: () -> Unit) { log += "sound:$sound"; pending.addLast(then) }
    override fun speak(sentence: String, voice: String, then: () -> Unit) { log += "say:$sentence|$voice"; pending.addLast(then) }
    override fun stop() { stopped++; pending.clear() }
    override fun voices() = listOf("Samantha", "Daniel")
    /** The current sound or sentence ends. */
    fun finish() = pending.removeFirst()()
}

class SpeechTest {
    private fun ann(id: String, kind: String, s: String) = Announcement(id, kind, s)

    private fun announcer(sink: FakeSink, muted: () -> Boolean = { false }, prefs: Preferences? = MemoryPreferences()) =
        Announcer(sink, muted, prefs)

    @Test fun twoAnnouncementsQueueAndNeverOverlap() {
        val sink = FakeSink()
        val a = announcer(sink)
        a.enqueue(ann("1", "needs_you", "Web API 204 needs a decision on token format."))
        a.enqueue(ann("2", "failed", "Task 7 failed."))
        assertEquals(listOf("sound:Glass"), sink.log)
        assertEquals(1, a.queue.size)
        sink.finish()
        assertEquals(listOf("sound:Glass", "say:Web API 204 needs a decision on token format.|"), sink.log)
        sink.finish()
        assertEquals("sound:Basso", sink.log.last())
        sink.finish()
        assertEquals("say:Task 7 failed.|", sink.log.last())
        sink.finish()
        assertNull(a.speaking)
        assertTrue(a.queue.isEmpty())
    }

    @Test fun decisionAndBrokeSoundDifferent() {
        val s = SpeechSettings()
        assertNotEquals(s.sound(ann("a", "needs_you", "x")), s.sound(ann("b", "failed", "x")))
    }

    @Test fun theSameAnnouncementIsVoicedOnce() {
        val sink = FakeSink()
        val a = announcer(sink)
        a.enqueue(ann("1", "failed", "Task 7 failed."))
        a.enqueue(ann("1", "failed", "Task 7 failed."))
        assertTrue(a.queue.isEmpty())
        sink.finish(); sink.finish()
        assertEquals(2, sink.log.size)
    }

    @Test fun mutingSilencesSoundAndSpeech() {
        val sink = FakeSink()
        var muted = false
        val a = announcer(sink, { muted })
        a.enqueue(ann("1", "needs_you", "One."))
        a.enqueue(ann("2", "needs_you", "Two."))
        muted = true
        a.silence()
        assertEquals(1, sink.stopped)
        assertTrue(a.queue.isEmpty())
        a.enqueue(ann("3", "failed", "Three."))
        assertTrue(a.queue.isEmpty())
        assertEquals(listOf("sound:Glass"), sink.log)
    }

    @Test fun mutingLeavesTheVisualAlerts() {
        val m = MonitorModel()
        m.header = HeaderView(status = "2 need you", attention = 2)
        m.connected = true
        m.toggleMute()
        assertTrue(m.muted)
        assertEquals(2, m.header?.attention)
        // The lost connection is still raised while muted.
        m.disconnected(1_000)
        m.pulse(1_000 + 16_000)
        assertTrue(m.announcer.daemonDown)
    }

    @Test fun unlistedKindsStayVisualOnly() {
        val sink = FakeSink()
        val a = announcer(sink)
        a.update(a.settings.copy(kinds = a.settings.kinds - "landed"))
        a.enqueue(ann("1", "landed", "Task 7 landed."))
        assertTrue(sink.log.isEmpty())
    }

    @Test fun theDefaultsSpeakDisk() {
        val sink = FakeSink()
        val a = announcer(sink)
        a.enqueue(ann("1", "disk", "Disk nearly full."))
        assertEquals(listOf("sound:Basso"), sink.log.take(1))
    }

    @Test fun aSavedListGainsTheNewKindsOnceAndALaterUntickStays() {
        val prefs = MemoryPreferences()
        prefs.setString("speech.kinds", "failed,landed,needs_you")
        val sink = FakeSink()
        val a = Announcer(sink, { false }, prefs)
        assertTrue("disk" in a.settings.kinds)
        assertTrue("provider_budget" in a.settings.kinds)
        assertTrue("credentials" !in a.settings.kinds)
        a.update(a.settings.copy(kinds = a.settings.kinds - "disk"))
        val again = Announcer(sink, { false }, prefs)
        assertTrue("disk" !in again.settings.kinds)
        assertTrue("tracker" in again.settings.kinds)
    }

    @Test fun voiceAndSoundsAreConfigurableAndRemembered() {
        val prefs = MemoryPreferences()
        val sink = FakeSink()
        val a = Announcer(sink, { false }, prefs)
        assertEquals(listOf("Samantha", "Daniel"), a.availableVoices())
        a.update(a.settings.copy(voice = "Daniel", sounds = a.settings.sounds + ("decision" to "Ping")))
        a.enqueue(ann("1", "needs_you", "Hello."))
        sink.finish()
        assertEquals(listOf("sound:Ping", "say:Hello.|Daniel"), sink.log)
        val again = Announcer(sink, { false }, prefs)
        assertEquals("Daniel", again.settings.voice)
        assertEquals("Ping", again.settings.sounds["decision"])
    }

    @Test fun daemonDownIsRaisedAfterFifteenSecondsOnceAndRearms() {
        val sink = FakeSink()
        val a = announcer(sink)
        a.connectionLost(0)
        a.check(14_000)
        assertFalse(a.daemonDown)
        a.check(15_000)
        assertTrue(a.daemonDown)
        assertEquals(listOf("sound:Basso"), sink.log)
        sink.finish()
        assertEquals("say:$DAEMON_DOWN_SENTENCE|", sink.log.last())
        sink.finish()
        a.check(60_000)
        assertEquals(2, sink.log.size)
        a.connectionRestored()
        assertFalse(a.daemonDown)
        a.connectionLost(100_000)
        a.check(120_000)
        assertTrue(a.daemonDown)
        assertEquals(3, sink.log.size)
    }

    @Test fun theNoChannelLineAppearsOnlyWhenTheDaemonSaysNoChannelWorks() {
        val a = announcer(FakeSink())
        assertNull(a.noChannelLine)
        a.handleClaim("""{"speaker":false,"reaches_without_monitor":true,"announcements":[]}""")
        assertNull(a.noChannelLine)
        a.handleClaim("""{"speaker":false,"reaches_without_monitor":false,"announcements":[]}""")
        assertNotNull(a.noChannelLine)
        a.handleClaim("""{"speaker":false,"announcements":[]}""")
        assertNull(a.noChannelLine)
    }

    @Test fun onlyTheSpeakerQueuesWhatTheDaemonSends() {
        val sink = FakeSink()
        val a = announcer(sink)
        a.handleClaim("""{"speaker":false,"announcements":[{"key":"x","kind":"failed","sentence":"No."}]}""")
        assertFalse(a.isSpeaker)
        assertTrue(sink.log.isEmpty())
        a.handleClaim(
            """{"speaker":true,"announcements":[
              {"key":"a","kind":"needs_you","sentence":"First.","sound":true,"speech":true},
              {"key":"b","kind":"failed","sentence":"Second.","sound":true,"speech":true}]}""",
        )
        assertTrue(a.isSpeaker)
        assertEquals(1, a.queue.size)
        sink.finish(); sink.finish()
        assertEquals("sound:Basso", sink.log.last())
    }

    @Test fun theClaimCarriesTheMuteState() {
        val a = announcer(FakeSink(), { true })
        assertTrue(a.claimBody("c1").contains("\"muted\":true"))
        assertTrue(a.claimBody("c1").contains("\"client\":\"c1\""))
    }

    @Test fun theOldChimeStopsOnceTheDaemonSpeaksForItself() {
        val m = MonitorModel()
        var chimes = 0
        m.onAttention = { chimes++ }
        fun sidebar(vararg ids: Long) = SidebarView(sections = listOf(SidebarSection(state = "needs_attention", rows = ids.map { TaskRow(id = it) })))
        m.noteAttention(sidebar())
        m.noteAttention(sidebar(1))
        assertEquals(1, chimes)
        m.announcer.handleClaim("""{"speaker":false,"announcements":[]}""")
        m.noteAttention(sidebar(1, 2))
        assertEquals(1, chimes)
    }
}
