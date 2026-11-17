// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor.ui

import org.junit.platform.launcher.LauncherSession
import org.junit.platform.launcher.LauncherSessionListener

/** Loads Skiko's native library once, before any test decodes an image through Skia. */
class SkiaSession : LauncherSessionListener {
    override fun launcherSessionOpened(session: LauncherSession) {
        org.jetbrains.skia.impl.Library.staticLoad()
    }
}
