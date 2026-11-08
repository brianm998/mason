// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import java.io.File

/** Finding the daemon on this machine: loopback only, by the port in `daemon.json`. */
object Discovery {
    fun defaultDataDir(): File {
        val home = System.getProperty("user.home")
        val os = System.getProperty("os.name").lowercase()
        return when {
            "mac" in os -> File(home, "Library/Application Support/mason")
            "win" in os -> File(System.getenv("APPDATA") ?: home, "mason")
            else -> File(home, ".local/share/mason")
        }
    }

    /** `--port N`, else `--data-dir D` (or `MASON_DATA_DIR`), else the default. */
    fun port(args: List<String>, env: Map<String, String> = System.getenv()): Int? = info(args, env)?.port

    /** The same, with the pid and start time from `daemon.json`, read afresh each call. */
    fun info(args: List<String>, env: Map<String, String> = System.getenv()): DaemonInfo? {
        fun opt(name: String) = args.indexOf(name).takeIf { it >= 0 }?.let { args.getOrNull(it + 1) }
        opt("--port")?.toIntOrNull()?.let { return DaemonInfo(it) }
        val dir = opt("--data-dir")?.let(::File) ?: env["MASON_DATA_DIR"]?.let(::File) ?: defaultDataDir()
        return info(File(dir, "daemon.json"))
    }

    fun info(file: File): DaemonInfo? {
        val text = runCatching { file.readText() }.getOrNull() ?: return null
        fun num(key: String) = Regex("\"$key\"\\s*:\\s*(\\d+)").find(text)?.groupValues?.get(1)?.toLongOrNull()
        val port = num("port")?.toInt() ?: return null
        return DaemonInfo(port, num("pid"), num("started_ms"))
    }
}
