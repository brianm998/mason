// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

/**
 * What clicking Open session does, apart from the window: the daemon's answer about the task's
 * session, the question when the step is running, the `mason task open` line, and the notice when
 * no terminal could be started. The actual terminal spawn is the app's (`MonitorModel.launcher`).
 */
object OpenSession {
    sealed interface Decision {
        /** The step is running: ask before a second session collides with it. */
        data object Confirm : Decision
        data class Launch(val anyway: Boolean) : Decision
        /** The daemon said no; show its words. */
        data class Failed(val why: String) : Decision
    }

    const val CONFIRM_QUESTION = "The step is running; a second session would collide with it. Open anyway?"

    /** Where the installer puts the CLI. Never found through PATH, which a new shell may lack. */
    fun defaultCli(home: String = System.getProperty("user.home") ?: ""): String = "$home/.config/mason/bin/mason"

    fun decide(ok: Boolean, json: String, error: String): Decision {
        if (!ok) return Decision.Failed(error.ifEmpty { "The daemon could not open that task's session." })
        val v = Codec.json(json) ?: return Decision.Failed("The daemon's answer about the session could not be read.")
        return if (v["idle"] == true) Decision.Launch(anyway = false) else Decision.Confirm
    }

    fun argv(cli: String, task: Long, anyway: Boolean): List<String> =
        listOf(cli, "task", "open", task.toString()) + if (anyway) listOf("--anyway") else emptyList()

    fun commandLine(cli: String, task: Long, anyway: Boolean): String =
        argv(cli, task, anyway).joinToString(" ") { quote(it) }

    internal fun quote(s: String): String =
        if (s.isNotEmpty() && s.all { it.isLetterOrDigit() || it in "/._-~=" }) s else "'" + s.replace("'", "'\\''") + "'"

    fun copiedNotice(line: String) = "Copied `$line`; paste it in a terminal."
}
