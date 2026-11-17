// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import mason.v1.*
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executor
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import kotlin.test.*

class UiThreadTest {
    private val cli = "/h/mason"
    private val pool = Executors.newSingleThreadExecutor { Thread(it, "bg-test").also { t -> t.isDaemon = true } }
    private val uiQueue = ArrayDeque<Runnable>()
    private val uiExec = Executor { synchronized(uiQueue) { uiQueue.addLast(it) } }
    private fun runUi() { while (true) { val r = synchronized(uiQueue) { uiQueue.removeFirstOrNull() } ?: break; r.run() } }
    private val copied = mutableListOf<String>()

    private fun model(l: (List<String>) -> Boolean) = MonitorModel(MemoryPreferences()).also {
        it.cliPath = cli; it.launcher = l; it.background = pool; it.ui = uiExec
        it.copier = { t -> copied += t }
    }

    private fun idle() = ServerMessage(result = CommandResult(id = "session:42", ok = true, json = """{"idle":true}"""))

    @Test fun launchReturnsBeforeTheLauncherFinishesAndRunsOffThread() {
        val release = CountDownLatch(1)
        val started = CountDownLatch(1)
        var launchThread: Thread? = null
        val m = model { launchThread = Thread.currentThread(); started.countDown(); release.await(); true }
        m.apply(idle())  // would hang here if the launcher ran inline
        assertTrue(started.await(10, TimeUnit.SECONDS))
        assertNotSame(Thread.currentThread(), launchThread)
        release.countDown()
        pool.submit {}.get()
        runUi()
        assertNull(m.sessionNotice)
        assertTrue(copied.isEmpty())
    }

    @Test fun aFailingLauncherCopiesAndNotesOnTheUiExecutor() {
        val m = model { false }
        m.apply(idle())
        pool.submit {}.get()
        assertNull(m.sessionNotice)  // not yet: waiting for the UI executor
        runUi()
        assertEquals(listOf("$cli task open 42"), copied)
        assertEquals("Copied `$cli task open 42`; paste it in a terminal.", m.sessionNotice)
    }

    @Test fun aThrowingLauncherCountsAsFailed() {
        val m = model { error("boom") }
        m.apply(idle())
        pool.submit {}.get()
        runUi()
        assertEquals(1, copied.size)
    }

    private class CountingSink : SpeechSink {
        var calls = 0
        var thread: Thread? = null
        val release = CountDownLatch(1)
        override fun play(sound: String, volume: Double, then: () -> Unit) {}
        override fun speak(sentence: String, voice: String, volume: Double, then: () -> Unit) {}
        override fun stop() {}
        override fun voices(): List<String> { calls++; thread = Thread.currentThread(); release.await(); return listOf("Daniel") }
    }

    @Test fun voicesLoadOnceOffThreadAndNeverBlockTheCaller() {
        val sink = CountingSink()
        val a = Announcer(sink, { false }, null)
        a.loadVoices(pool)
        a.loadVoices(pool)  // a second composition
        assertNull(a.voices)
        sink.release.countDown()
        pool.submit {}.get()
        assertEquals(listOf("Daniel"), a.voices)
        assertEquals(1, sink.calls)
        assertNotSame(Thread.currentThread(), sink.thread)
    }

    private class CountingPrefs : Preferences {
        val reads = mutableMapOf<String, Int>()
        private val inner = MemoryPreferences()
        override fun bool(key: String): Boolean? { reads.merge(key, 1, Int::plus); return inner.bool(key) }
        override fun setBool(key: String, value: Boolean) = inner.setBool(key, value)
        override fun double(key: String) = inner.double(key)
        override fun setDouble(key: String, value: Double) = inner.setDouble(key, value)
        override fun string(key: String) = inner.string(key)
        override fun setString(key: String, value: String) = inner.setString(key, value)
    }

    @Test fun isExpandedReadsEachKeyOnce() {
        val prefs = CountingPrefs()
        val m = MonitorModel(prefs)
        val sec = SidebarSection(state = "running", collapsed = true)
        repeat(50) { assertFalse(m.isExpanded(sec)) }
        assertEquals(1, prefs.reads.values.sum())
        m.setExpanded("running", true)
        assertTrue(m.isExpanded(sec))
        assertEquals(1, prefs.reads.values.sum())
    }

    @Test fun knownIdsAreRebuiltOnlyWhenTheViewsChange() {
        val m = MonitorModel(MemoryPreferences())
        m.sidebar = SidebarView(sections = listOf(SidebarSection(state = "s", rows = listOf(TaskRow(id = 7)))))
        repeat(20) { m.linkify("see #7 and #8") }
        assertEquals(1, m.knownRebuilds)
        assertTrue(m.isKnownTask(7))
        assertFalse(m.isKnownTask(8))
        m.sidebar = SidebarView(sections = listOf(SidebarSection(state = "s", rows = listOf(TaskRow(id = 8)))))
        assertTrue(m.isKnownTask(8))
        assertEquals(2, m.knownRebuilds)
    }
}
