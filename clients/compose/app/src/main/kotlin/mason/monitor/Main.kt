// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.ui.input.key.*
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.MenuBar
import androidx.compose.ui.window.Window
import androidx.compose.ui.window.application
import androidx.compose.ui.window.rememberWindowState
import androidx.compose.ui.graphics.painter.BitmapPainter
import androidx.compose.ui.graphics.toAwtImage
import mason.monitor.ui.Mascot
import mason.monitor.ui.RootView

/** The monitor window. Run with `./gradlew :app:monitor` (optionally `--args="--data-dir D"`). */
fun main(args: Array<String>) = application {
    val model = remember {
        MonitorModel(SystemPreferences()).also {
            // The alert sound the model asks for when something new needs the owner.
            it.onAttention = { java.awt.Toolkit.getDefaultToolkit().beep() }
            // Sounds and sentences come from the daemon's announcements, spoken by this sink when
            // this monitor is the one that speaks. The beep stays for a daemon that sends none.
            it.announcer.sink = SystemSink()
            it.copier = { text -> mason.monitor.ui.copyToClipboard(text) }
            it.launcher = { argv -> openTerminal(argv) }
            val pool = java.util.concurrent.Executors.newCachedThreadPool { r -> Thread(r, "monitor-bg").also { t -> t.isDaemon = true } }
            it.background = pool
            it.ui = java.util.concurrent.Executor { r -> javax.swing.SwingUtilities.invokeLater(r) }
        }
    }
    remember {
        UiWatchdog({ job -> javax.swing.SwingUtilities.invokeLater(Runnable { job() }) })
            .also { it.start(); UiLateness.recent = it::recent; UiLateness.recentBackground = it::recentBackground }
        0
    }
    LaunchedEffect(Unit) {
        while (true) {
            kotlinx.coroutines.delay(1000)
            model.pulse()
        }
    }
    remember { setDockIcon(); 0 }
    val state = rememberWindowState(width = 1360.dp, height = 860.dp)
    remember { model.onInstall = { installMonitor(model, it) }; 0 }
    remember {
        model.onOpenUrl = { u ->
            runCatching { java.awt.Desktop.getDesktop().browse(java.net.URI(u)) }
        }
        0
    }
    remember {
        // Progress bursts are applied once a frame (about 30 ms), on the UI thread.
        model.scheduleProgressFlush = { flush ->
            javax.swing.Timer(33) { flush() }.apply { isRepeats = false; start() }
        }
        0
    }
    LaunchedEffect(Unit) {
        // The daemon may not be up yet: look for it until it is.
        while (true) {
            // daemon.json is read on an IO thread, not the UI's.
            val found = kotlinx.coroutines.withContext(kotlinx.coroutines.Dispatchers.IO) { Discovery.info(args.toList()) }
            if (found != null) {
                // A daemon found through its own daemon.json is on this machine, so a folder picker can name its paths.
                if (found.pid != null) model.chooseDirectory = { chooseLocalDirectory() }
                MonitorConnection(model, { Discovery.info(args.toList()) }, ui = java.util.concurrent.Executor { javax.swing.SwingUtilities.invokeLater(it) }).start()
                break
            }
            kotlinx.coroutines.delay(1000)
        }
    }
    Window(
        onCloseRequest = ::exitApplication,
        title = "MASON",
        icon = remember { Mascot.appIcon()?.let { BitmapPainter(it) } },
        state = state,
        onPreviewKeyEvent = { e -> shortcut(model, e) },
    ) {
        remember {
            UiLateness.appState = {
                when {
                    !window.isShowing || state.isMinimized -> mason.monitor.AppVisibility.HIDDEN
                    window.isFocused -> mason.monitor.AppVisibility.FRONTMOST
                    else -> mason.monitor.AppVisibility.VISIBLE
                }
            }
            0
        }
        MenuBar {
            Menu("File") {
                val mac = System.getProperty("os.name").lowercase().contains("mac")
                Item(
                    "Add Project…",
                    shortcut = KeyShortcut(Key.N, meta = mac, ctrl = !mac, shift = true),
                    enabled = model.canAct,
                    onClick = { model.openAddProject() },
                )
            }
        }
        RootView(model)
    }
}

/** A directory picker (JFileChooser); null when cancelled. Runs on the UI thread, from a click. */
internal fun chooseLocalDirectory(): String? {
    val c = javax.swing.JFileChooser().apply {
        fileSelectionMode = javax.swing.JFileChooser.DIRECTORIES_ONLY
        dialogTitle = "Choose a git repository"
    }
    return if (c.showOpenDialog(null) == javax.swing.JFileChooser.APPROVE_OPTION) c.selectedFile.absolutePath else null
}

/** Put the MASON icon in the macOS Dock and app switcher; a no-op where the platform has none. */
internal fun setDockIcon() {
    runCatching {
        val icon = Mascot.dockIcon() ?: return
        if (!java.awt.Taskbar.isTaskbarSupported()) return
        val bar = java.awt.Taskbar.getTaskbar()
        if (bar.isSupported(java.awt.Taskbar.Feature.ICON_IMAGE)) bar.iconImage = icon.toAwtImage()
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

/** The daemon has built a new monitor at [path]: start it and close this one. */
fun installMonitor(model: MonitorModel, path: String) {
    model.background.execute {
        val started = runCatching {
            ProcessBuilder(path).redirectErrorStream(true).redirectOutput(ProcessBuilder.Redirect.DISCARD).start()
        }.isSuccess
        model.ui.execute {
            if (started) kotlin.system.exitProcess(0) else model.monitorUpdateFailed("The new monitor would not start; this one keeps running.")
        }
    }
}

/**
 * A terminal running `argv`, from a temporary script: macOS `open -a Terminal`, else `$TERMINAL`
 * or `x-terminal-emulator`. False when none could be started; the caller copies the command then.
 */
internal fun openTerminal(argv: List<String>): Boolean = runCatching {
    val line = argv.joinToString(" ") { OpenSession.quote(it) }
    val mac = System.getProperty("os.name").orEmpty().startsWith("Mac")
    val script = java.io.File.createTempFile("mason-open-", if (mac) ".command" else ".sh")
    script.writeText("#!/bin/bash\nrm -f \"$0\"\n$line\n")
    script.setExecutable(true)
    val cmd = if (mac) {
        listOf("open", "-a", "Terminal", script.path)
    } else {
        val t = System.getenv("TERMINAL")?.takeIf { it.isNotBlank() } ?: "x-terminal-emulator"
        listOf(t, "-e", script.path)
    }
    val p = ProcessBuilder(cmd).redirectErrorStream(true).start()
    // `open` returns at once; a terminal emulator stays up, so only a quick failure counts.
    if (p.waitFor(2, java.util.concurrent.TimeUnit.SECONDS)) p.exitValue() == 0 else true
}.getOrDefault(false)
