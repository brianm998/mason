// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import mason.v1.*
import kotlin.test.*

class OpenSessionTest {
    private val cli = "/h/.config/mason/bin/mason"
    private val launched = mutableListOf<List<String>>()
    private val copied = mutableListOf<String>()
    private var launchWorks = true

    private fun model() = MonitorModel(MemoryPreferences()).also {
        it.cliPath = cli
        it.launcher = { argv -> launched += argv; launchWorks }
        it.copier = { text -> copied += text }
    }

    private fun result(ok: Boolean = true, json: String = "", error: String = "") =
        ServerMessage(result = CommandResult(id = "session:42", ok = ok, json = json, error = error))

    @Test fun idleLaunchesWithTheFullPathAndNoQuestion() {
        val m = model()
        m.apply(result(json = """{"idle":true,"worktree":"/w"}"""))
        assertEquals(listOf(listOf(cli, "task", "open", "42")), launched)
        assertNull(m.sessionPrompt)
    }

    @Test fun runningAsksAndYesAddsAnyway() {
        val m = model()
        m.apply(result(json = """{"idle":false}"""))
        assertEquals(42L, m.sessionPrompt)
        assertTrue(launched.isEmpty())
        m.answerSessionPrompt(true)
        assertEquals(listOf(listOf(cli, "task", "open", "42", "--anyway")), launched)
        assertNull(m.sessionPrompt)
    }

    @Test fun runningAndNoDoesNothing() {
        val m = model()
        m.apply(result(json = """{"idle":false}"""))
        m.answerSessionPrompt(false)
        assertTrue(launched.isEmpty())
        assertTrue(copied.isEmpty())
        assertNull(m.sessionPrompt)
    }

    @Test fun aNotFoundShowsTheDaemonsWords() {
        val m = model()
        m.apply(result(ok = false, error = "no such task"))
        assertEquals("no such task", m.sessionNotice)
        assertTrue(launched.isEmpty())
    }

    @Test fun aFailedLauncherCopiesAndSaysSo() {
        val m = model()
        launchWorks = false
        m.apply(result(json = """{"idle":true}"""))
        assertEquals(listOf("$cli task open 42"), copied)
        assertEquals("Copied `$cli task open 42`; paste it in a terminal.", m.sessionNotice)
    }

    @Test fun pathsWithSpacesAreQuoted() {
        assertEquals("'/a b/mason' task open 7 --anyway", OpenSession.commandLine("/a b/mason", 7, true))
    }
}
