// SPDX-License-Identifier: GPL-3.0-or-later
package mason.android

import android.content.Context
import android.content.Intent
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.core.content.ContextCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import java.util.concurrent.Executors
import mason.android.ui.PhoneApp
import mason.monitor.MonitorModel
import mason.monitor.Page

/**
 * The one activity. Everything lives for the process: the model, the paired store and the link. The
 * link is up while the app is in the foreground and closed in the background; coming back resumes
 * from the last sequence number.
 */
class MainActivity : ComponentActivity() {
    private lateinit var nsd: NsdSource
    private lateinit var session: RemoteSession
    private lateinit var pairing: PairingFlow
    private val model = MonitorModel()
    private val io = Executors.newSingleThreadExecutor()

    /** A deep link (`mason://task/12`) waiting for the monitor to show. */
    private var pendingPage by androidx.compose.runtime.mutableStateOf<Page?>(null)

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val prefs = getSharedPreferences("mason", Context.MODE_PRIVATE)
        val kv = object : KeyValue {
            override fun get(key: String) = prefs.getString(key, null)
            override fun put(key: String, value: String?) { prefs.edit().apply { if (value == null) remove(key) else putString(key, value) }.apply() }
        }
        val store = KeyValuePairedStore(kv)
        val tokens = KeystoreTokenStore(kv, AndroidKeystore.box())
        nsd = NsdSource(this)
        val main = ContextCompat.getMainExecutor(this)
        session = RemoteSession(model, store, tokens, nsd, ui = main)
        pairing = PairingFlow(
            store, tokens, HttpPairClient(), Build.MODEL ?: "Android phone",
            background = { io.execute(it) }, ui = { main.execute(it) }, onPaired = { session.pairingChanged() },
        )
        handle(intent)
        lifecycle.addObserver(LifecycleEventObserver { _, e ->
            when (e) {
                Lifecycle.Event.ON_START -> { nsd.start(); session.activate() }
                Lifecycle.Event.ON_STOP -> { session.deactivate(); nsd.stop() }
                else -> {}
            }
        })
        setContent {
            PhoneApp(
                model = model, session = session, pairing = pairing, nearby = nsd,
                pendingPage = pendingPage, onPageShown = { pendingPage = null },
            )
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        handle(intent)
    }

    /** `mason://pair?...` fills the pairing screen; any other `mason://` address opens that page. */
    private fun handle(i: Intent?) {
        val uri = i?.data?.takeIf { it.scheme == "mason" } ?: return
        if (uri.host == "pair") {
            pairing.open(uri.toString())
        } else {
            Page.from(uri.toString())?.let { pendingPage = it }
        }
    }
}
