// SPDX-License-Identifier: GPL-3.0-or-later
package mason.android

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import java.util.concurrent.Executor
import mason.monitor.ConnectionTiming
import mason.monitor.MonitorConnection
import mason.monitor.MonitorModel
import mason.monitor.PairedDaemon
import mason.monitor.SocketFactory
import mason.monitor.TokenStore

enum class RemoteState { Unpaired, Paired }

/**
 * The monitor's link to the one paired daemon, driven by the app's lifecycle: [activate] when the
 * app comes to the foreground, [deactivate] when it goes to the background (the socket is closed,
 * and the next [activate] says hello with the last sequence number the model saw). A 401 on the
 * upgrade means the token was revoked: the pairing is forgotten and the pairing screen shows.
 */
class RemoteSession(
    private val model: MonitorModel,
    private val store: PairedStore,
    private val tokens: TokenStore,
    private val source: ServiceSource?,
    private val socketFor: (PairedDaemon) -> SocketFactory = ::PinnedSocketFactory,
    private val timing: ConnectionTiming = ConnectionTiming(),
    private val ui: Executor = Executor { it.run() },
) {
    var state by mutableStateOf(if (store.load() == null) RemoteState.Unpaired else RemoteState.Paired)
        private set
    /** The address the link last chose, for the settings page. */
    var reachedHost by mutableStateOf<String?>(null)
        private set

    private var connection: MonitorConnection? = null
    private var discovery: RemoteDiscovery? = null

    /** The paired daemon with its token; null when unpaired or the token is gone. */
    val paired: PairedDaemon?
        get() {
            val r = store.load() ?: return null
            val t = tokens.token(r.fingerprint) ?: return null
            return r.withToken(t)
        }

    /** The app is in the foreground: connect, resuming from the last sequence number. */
    fun activate() {
        if (connection != null) return
        val d = paired ?: run { forget(); return }
        state = RemoteState.Paired
        val disc = RemoteDiscovery(d, source)
        disc.onChoose { e, _ -> ui.execute { reachedHost = e.host } }
        discovery = disc
        val c = MonitorConnection(
            model, { disc.current() }, timing, socketFor(d), ui = ui,
            onAttemptEnd = { disc.attemptEnded(it) },
            onRefused = { ui.execute { unpair() } },
        )
        connection = c
        c.start()
    }

    /** The app went to the background: close the socket. */
    fun deactivate() = dropConnection()

    /** Pairing wrote a new daemon: connect to it instead. */
    fun pairingChanged() {
        dropConnection()
        model.lastSeq = 0
        activate()
    }

    /** Forget: delete the token and the pairing, and show the pairing screen. */
    fun unpair() {
        store.load()?.let { tokens.set(it.fingerprint, null) }
        forget()
    }

    private fun forget() {
        dropConnection()
        store.clear()
        state = RemoteState.Unpaired
    }

    private fun dropConnection() {
        connection?.stop()
        connection = null
        discovery = null
        reachedHost = null
    }
}
