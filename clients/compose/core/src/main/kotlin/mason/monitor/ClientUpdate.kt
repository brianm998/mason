// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import java.io.File

/**
 * How this monitor gets a newer version of itself (Part 15). On desktop the button
 * builds from a source checkout (development) and relaunches; where a signed build
 * is published it would fetch that instead (not built yet); otherwise there is a
 * page to go to. This only decides; the app does it.
 */
sealed class UpdatePlan {
    /** Build the monitor from this checkout, launch it, close this one. */
    data class BuildFromSource(val dir: String) : UpdatePlan()

    /** Open this page (the install path). */
    data class OpenPage(val url: String) : UpdatePlan()
    data class Unavailable(val why: String) : UpdatePlan()
}

object ClientUpdate {
    fun plan(env: Map<String, String>, exists: (String) -> Boolean = { File(it).exists() }): UpdatePlan {
        val dir = env["MASON_SOURCE_DIR"]
        if (!dir.isNullOrEmpty()) {
            return if (exists("$dir/clients/compose/settings.gradle.kts")) UpdatePlan.BuildFromSource(dir)
            else UpdatePlan.Unavailable("MASON_SOURCE_DIR is $dir, but there is no clients/compose in it")
        }
        val url = env["MASON_INSTALL_URL"]
        if (url != null && url.startsWith("https://")) return UpdatePlan.OpenPage(url)
        return UpdatePlan.Unavailable(
            "This monitor does not know where to get a newer one. Set MASON_SOURCE_DIR to a checkout of the source, or MASON_INSTALL_URL to the install page.",
        )
    }
}
