// SPDX-License-Identifier: GPL-3.0-or-later
package mason.android

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import mason.monitor.PairUrl
import mason.monitor.PairedDaemon
import mason.monitor.Pin
import mason.monitor.ServiceRecord
import mason.monitor.TokenStore

/** A `_mason._tcp` record that has been resolved to an address. */
data class ResolvedService(val record: ServiceRecord, val host: String, val port: Int)

/** Words for a pairing that did not work. The daemon's own message is kept; the status only picks the lead-in. */
object PairingMessage {
    fun text(status: Int, daemon: String): String {
        val said = if (daemon.isEmpty()) "" else " ($daemon)"
        return when (status) {
            400 -> "That code is wrong$said. Check the 8 digits on the Mac and try again."
            410 -> "That code has expired$said. Make a new one with `mason pair` and try again."
            429 -> "Too many wrong codes$said. Pairing is locked for a while; make a new code later."
            401, 403 -> "The daemon refused this device$said. LAN access may be off."
            else -> "The daemon refused the pairing ($status)$said."
        }
    }

    fun text(e: Throwable): String = when (e) {
        is PairingRefused -> text(e.status, e.daemon)
        is PairingBadAnswer -> "The daemon's answer could not be read."
        is javax.net.ssl.SSLException, is java.security.cert.CertificateException ->
            "This is not the daemon the code or fingerprint named: its certificate did not match."
        else -> "Could not reach the daemon: ${e.message ?: e.javaClass.simpleName}"
    }
}

/**
 * The pairing screen's state: which path, what was scanned or chosen, the typed code, the last error.
 * No I/O; [PairingFlow] runs the exchange. The same rules as `PairingScreenState` on the iPhone.
 */
data class PairingScreenState(
    val path: Path = Path.Scan,
    /** A daemon picked from the Nearby list (its fingerprint came from unauthenticated mDNS). */
    val chosen: ResolvedService? = null,
    /** A full pairing address: scanned, or opened from a `mason://pair` link. */
    val address: PairUrl? = null,
    val code: String = "",
    val error: String? = null,
    val busy: Boolean = false,
) {
    enum class Path { Scan, Nearby }

    /** A scan or link result. An error (and nothing else changed) when the text is not a pairing address. */
    fun scanned(text: String): PairingScreenState {
        val u = PairUrl.parse(text) ?: return copy(error = "That is not a MASON pairing code.")
        return copy(address = u, chosen = null, code = u.code, error = null, path = Path.Scan)
    }

    fun choose(s: ResolvedService) = copy(chosen = s, address = null, code = "", error = null, path = Path.Nearby)

    fun withPath(p: Path) = copy(path = p)

    /** Digits only, at most 8. */
    fun withCode(typed: String) = copy(code = typed.filter { it in '0'..'9' }.take(8))

    val codeValid get() = Pin.isCode(code)

    /** What to pair with, when there is a target and a valid code. */
    val request: PairUrl?
        get() {
            if (!codeValid) return null
            address?.let { return it.copy(code = code) }
            chosen?.let { return PairUrl(it.host, it.port, it.record.fingerprint, code) }
            return null
        }

    val canSubmit get() = request != null && !busy

    /** Start a pairing: the busy state and what to send, or the same state with an error when it cannot start. */
    fun begin(): Pair<PairingScreenState, PairUrl?> {
        if (busy) return this to null
        val r = request ?: return copy(
            error = if (address == null && chosen == null) "Choose a daemon or scan its QR code first." else "The code is 8 digits.",
        ) to null
        return copy(busy = true, error = null) to r
    }

    fun failed(e: Throwable) = copy(busy = false, error = PairingMessage.text(e))

    companion object {
        /** The caution under the Nearby list. */
        const val NEARBY_WARNING =
            "These names come from the network and are not verified. Scanning the QR code is the stronger check. " +
                "Compare the fingerprint start with `mason lan status` before you pair."

        /** The first 8 characters of a fingerprint, to compare with `mason lan status`. */
        fun shortFingerprint(fp: String) = fp.take(8)
    }
}

/** Runs the pairing exchange for the screen, saves the daemon and its token, and tells the session. */
class PairingFlow(
    private val store: PairedStore,
    private val tokens: TokenStore,
    private val client: PairClient,
    private val deviceName: String,
    /** Runs the blocking exchange off the UI thread. */
    private val background: (() -> Unit) -> Unit,
    /** Runs a result on the UI thread. */
    private val ui: (() -> Unit) -> Unit,
    private val onPaired: () -> Unit,
) {
    var state by mutableStateOf(PairingScreenState())

    fun update(f: (PairingScreenState) -> PairingScreenState) { state = f(state) }

    /** A `mason://pair` link opened the app. */
    fun open(url: String) = update { it.scanned(url) }

    fun submit() {
        val (next, req) = state.begin()
        state = next
        if (req == null) return
        background {
            val result = runCatching { client.pair(req, deviceName) }
            ui {
                result.onSuccess { d ->
                    tokens.set(d.fingerprint, d.token)
                    store.save(PairedRecord(d))
                    state = PairingScreenState()
                    onPaired()
                }.onFailure { state = state.failed(it) }
            }
        }
    }
}

/** What the exchange can say back. */
class PairingRefused(val status: Int, val daemon: String) : Exception("pairing refused ($status)")
class PairingBadAnswer : Exception("unreadable pairing answer")

/** Exchanges a code for a token. Blocking. */
fun interface PairClient {
    fun pair(url: PairUrl, deviceName: String): PairedDaemon
}
