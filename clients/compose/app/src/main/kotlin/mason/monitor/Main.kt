// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.ui.input.key.*
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Window
import androidx.compose.ui.window.application
import androidx.compose.ui.window.rememberWindowState
import mason.monitor.ui.RootView

/** The monitor window. Run with `./gradlew :app:monitor` (optionally `--args="--data-dir D"`). */
fun main(args: Array<String>) = application {
    val model = remember { MonitorModel(SystemPreferences()) }
    val state = rememberWindowState(width = 1360.dp, height = 860.dp)
    LaunchedEffect(Unit) {
        // The daemon may not be up yet: look for it until it is.
        while (true) {
            val port = Discovery.port(args.toList())
            if (port != null) {
                MonitorConnection(model, port).start()
                break
            }
            kotlinx.coroutines.delay(1000)
        }
    }
    Window(
        onCloseRequest = ::exitApplication,
        title = "MASON",
        state = state,
        onPreviewKeyEvent = { e -> shortcut(model, e) },
    ) {
        RootView(model)
    }
}

/**
 * The same shortcuts as the SwiftUI client: back and forward (⌘[ ⌘], Ctrl on other
 * systems), text scale (+ − 0) and home (shift+h).
 */
internal fun shortcut(model: MonitorModel, e: KeyEvent): Boolean {
    if (e.type != KeyEventType.KeyDown) return false
    val mod = if (System.getProperty("os.name").lowercase().contains("mac")) e.isMetaPressed else e.isCtrlPressed
    if (!mod) return false
    return when (e.key) {
        Key.LeftBracket -> { model.goBack(); true }
        Key.RightBracket -> { model.goForward(); true }
        Key.Equals, Key.Plus, Key.NumPadAdd -> { model.zoomIn(); true }
        Key.Minus, Key.NumPadSubtract -> { model.zoomOut(); true }
        Key.Zero, Key.NumPad0 -> { model.zoomReset(); true }
        Key.H -> if (e.isShiftPressed) { model.goHome(); true } else false
        else -> false
    }
}
