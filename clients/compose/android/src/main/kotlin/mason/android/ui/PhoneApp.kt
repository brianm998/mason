// SPDX-License-Identifier: GPL-3.0-or-later
package mason.android.ui

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import mason.android.PairingFlow
import mason.android.RemoteSession
import mason.android.RemoteState
import mason.android.ServiceSource
import mason.monitor.MonitorModel
import mason.monitor.Page
import mason.monitor.PhoneNav
import mason.monitor.PhoneScreen

/** The app: the pairing screen until a daemon is paired, then the phone monitor. */
@Composable
fun PhoneApp(
    model: MonitorModel, session: RemoteSession, pairing: PairingFlow, nearby: ServiceSource,
    pendingPage: Page?, onPageShown: () -> Unit,
) {
    MaterialTheme(colorScheme = if (androidx.compose.foundation.isSystemInDarkTheme()) darkColorScheme() else lightColorScheme()) {
        Surface(Modifier.fillMaxSize()) {
            when (session.state) {
                RemoteState.Unpaired -> PairingScreen(pairing, nearby)
                RemoteState.Paired -> PhoneMonitor(model, session, pendingPage, onPageShown)
            }
        }
    }
}

/** The sidebar is the first screen; choosing a row pushes its page; Back pops. */
@Composable
fun PhoneMonitor(model: MonitorModel, session: RemoteSession, pendingPage: Page?, onPageShown: () -> Unit) {
    var nav by remember { mutableStateOf(PhoneNav()) }
    // The connection subscribes to the views of `model.page`, so the model follows the screen.
    fun show(n: PhoneNav) {
        nav = n
        model.go(n.stack.lastOrNull() ?: Page.Home)
    }
    LaunchedEffect(pendingPage) {
        pendingPage?.let { show(PhoneNav().select(it)); onPageShown() }
    }
    BackHandler(enabled = nav.stack.isNotEmpty()) { show(nav.back()) }
    Column(Modifier.fillMaxSize().systemBarsPadding()) {
        TopBar(model, nav, onBack = { show(nav.back()) }, onSettings = { show(nav.select(Page.Settings)) })
        Banners(model)
        Box(Modifier.weight(1f)) {
            when (val screen = nav.screen) {
                PhoneScreen.Sidebar -> SidebarScreen(model) { show(nav.select(it)) }
                is PhoneScreen.Detail -> DetailScreen(model, screen.page, onOpen = { show(nav.select(it)) }) {
                    PairedSettings(session.paired?.let(::PairedRecordView), session.reachedHost) { session.unpair() }
                }
            }
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun TopBar(model: MonitorModel, nav: PhoneNav, onBack: () -> Unit, onSettings: () -> Unit) {
    TopAppBar(
        title = { Text(if (nav.stack.isEmpty()) "MASON" else pageTitle(nav.stack.last())) },
        navigationIcon = { if (nav.stack.isNotEmpty()) TextButton(onClick = onBack) { Text("‹ Back") } },
        actions = {
            if (model.attention > 0) Text("${model.attention} need you", color = MaterialTheme.colorScheme.error)
            TextButton(onClick = { model.toggleMute() }) { Text(if (model.muted) "Unmute" else "Mute") }
            if (nav.stack.isEmpty()) TextButton(onClick = onSettings) { Text("Daemon") }
        },
    )
}

@Composable
private fun Banners(model: MonitorModel) {
    val texts = buildList {
        if (!model.connected) add(if (model.reconnecting) "Connection lost. Reconnecting…" else "Connecting…")
        else {
            model.header?.banners?.filter { it.level != "info" }?.forEach { add(it.text) }
        }
    }
    for (t in texts) {
        Surface(color = MaterialTheme.colorScheme.secondaryContainer, modifier = Modifier.fillMaxWidth()) {
            Text(t, Modifier.padding(horizontal = 16.dp, vertical = 8.dp), style = MaterialTheme.typography.bodyMedium)
        }
    }
}

fun pageTitle(p: Page): String = when (p) {
    Page.Home -> "Home"
    is Page.Task -> "Task #${p.id}"
    Page.Landing -> "Landing"
    Page.Usage -> "Usage"
    Page.Settings -> "Paired daemon"
    Page.Pipeline -> "Pipeline"
    Page.Health -> "Health"
    is Page.Section -> mason.monitor.SectionPages.title(p.name)
}

/** What the settings page shows about the pairing. */
class PairedRecordView(val host: String, val role: String, val fingerprint: String) {
    constructor(d: mason.monitor.PairedDaemon) : this(d.host, d.role, d.fingerprint)
}
