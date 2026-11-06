// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

object Format {
    /** Time in state: "42 s", "9 min", "3h10". Always shown, never hidden. */
    fun duration(ms: Long): String {
        val s = ms / 1000
        return when {
            s < 60 -> "$s s"
            s < 3600 -> "${s / 60} min"
            else -> {
                val m = (s % 3600) / 60
                if (m == 0L) "${s / 3600}h" else "${s / 3600}h${m.toString().padStart(2, '0')}"
            }
        }
    }
}
