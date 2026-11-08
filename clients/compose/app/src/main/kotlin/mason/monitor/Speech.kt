// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import java.util.concurrent.Executors

/**
 * Plays a named system sound, then speaks with an OS voice. Each call finishes when the sound or
 * the sentence has ended (the helper process exits), which is what keeps announcements from
 * overlapping. macOS uses `afplay` and `say`; Linux `espeak-ng` or `espeak` or `spd-say`; Windows
 * PowerShell's speech synthesizer. Where none is present it falls back to the AWT beep and says
 * nothing.
 */
class SystemSink : SpeechSink {
    private val os = System.getProperty("os.name").lowercase()
    private val mac = os.contains("mac")
    private val windows = os.contains("win")
    private val pool = Executors.newSingleThreadExecutor { r -> Thread(r, "speech").apply { isDaemon = true } }
    @Volatile private var current: Process? = null

    private fun run(cmd: List<String>, then: () -> Unit) {
        pool.execute {
            try {
                val p = ProcessBuilder(cmd).redirectErrorStream(true).redirectOutput(ProcessBuilder.Redirect.DISCARD).start()
                current = p
                p.waitFor()
            } catch (_: Exception) {
                // not installed: say nothing
            } finally {
                current = null
                then()
            }
        }
    }

    private fun which(vararg names: String): String? = names.firstOrNull { n ->
        System.getenv("PATH").orEmpty().split(java.io.File.pathSeparator).any { java.io.File(it, n).canExecute() }
    }

    override fun play(sound: String, then: () -> Unit) {
        val file = "/System/Library/Sounds/$sound.aiff"
        if (mac && java.io.File(file).exists()) run(listOf("afplay", file), then)
        else { java.awt.Toolkit.getDefaultToolkit().beep(); then() }
    }

    override fun speak(sentence: String, voice: String, then: () -> Unit) {
        val cmd: List<String>? = when {
            mac -> listOf("say") + (if (voice.isNotEmpty()) listOf("-v", voice) else emptyList()) + listOf("--", sentence)
            windows -> listOf(
                "powershell", "-NoProfile", "-Command",
                "Add-Type -AssemblyName System.Speech; \$s = New-Object System.Speech.Synthesis.SpeechSynthesizer; " +
                    (if (voice.isNotEmpty()) "\$s.SelectVoice(\$args[1]); " else "") + "\$s.Speak(\$args[0])",
                sentence, voice,
            )
            else -> which("espeak-ng", "espeak")?.let { listOf(it) + (if (voice.isNotEmpty()) listOf("-v", voice) else emptyList()) + listOf("--", sentence) }
                ?: which("spd-say")?.let { listOf(it, "--wait", sentence) }
        }
        if (cmd == null) then() else run(cmd, then)
    }

    override fun stop() { current?.destroy() }

    override fun voices(): List<String> = try {
        val lines = when {
            mac -> exec(listOf("say", "-v", "?")).lines().map { it.trim().split(Regex("\\s{2,}")).first() }
            windows -> exec(listOf("powershell", "-NoProfile", "-Command",
                "Add-Type -AssemblyName System.Speech; (New-Object System.Speech.Synthesis.SpeechSynthesizer).GetInstalledVoices() | % { \$_.VoiceInfo.Name }")).lines()
            else -> which("espeak-ng", "espeak")?.let { exec(listOf(it, "--voices")).lines().drop(1).mapNotNull { l -> l.trim().split(Regex("\\s+")).getOrNull(3) } } ?: emptyList()
        }
        lines.map { it.trim() }.filter { it.isNotEmpty() }.distinct().sorted()
    } catch (_: Exception) { emptyList() }

    private fun exec(cmd: List<String>): String {
        val p = ProcessBuilder(cmd).redirectErrorStream(true).start()
        val out = p.inputStream.bufferedReader().readText()
        p.waitFor()
        return out
    }
}
