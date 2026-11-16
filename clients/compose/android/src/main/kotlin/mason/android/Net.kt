// SPDX-License-Identifier: GPL-3.0-or-later
package mason.android

import java.security.SecureRandom
import java.util.concurrent.TimeUnit
import javax.net.ssl.SSLContext
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import org.json.JSONObject
import mason.monitor.DaemonInfo
import mason.monitor.MonitorSocket
import mason.monitor.PairUrl
import mason.monitor.PairedDaemon
import mason.monitor.PinningTrustManager
import mason.monitor.SocketEvents
import mason.monitor.SocketFactory

/** An HTTP client that trusts exactly one certificate: the one whose SHA-256 is [fingerprint]. The name is not checked; the pin is. */
fun pinnedClient(fingerprint: String): OkHttpClient {
    val trust = PinningTrustManager(fingerprint)
    val ctx = SSLContext.getInstance("TLS").apply { init(null, arrayOf(trust), SecureRandom()) }
    return OkHttpClient.Builder()
        .sslSocketFactory(ctx.socketFactory, trust)
        .hostnameVerifier { _, _ -> true }
        .connectTimeout(10, TimeUnit.SECONDS)
        .readTimeout(0, TimeUnit.MILLISECONDS)  // a WebSocket is silent between messages; the connection watches for stalls itself
        .build()
}

/** What the daemon said to `POST /pair`, as a paired daemon (without the token being stored). */
fun parsePairAnswer(status: Int, body: String, url: PairUrl): PairedDaemon {
    val obj = runCatching { JSONObject(body) }.getOrNull()
    if (status != 200) throw PairingRefused(status, obj?.optString("error", "refused") ?: "refused")
    val token = obj?.optString("token", "")?.takeIf { it.isNotEmpty() }
    val role = obj?.optString("role", "")?.takeIf { it.isNotEmpty() }
    if (token == null || role == null) throw PairingBadAnswer()
    return PairedDaemon(url.host, url.port, url.fingerprint, token, role, url.alternates)
}

/** Pairs over TLS pinned to the fingerprint in the address. */
class HttpPairClient : PairClient {
    override fun pair(url: PairUrl, deviceName: String): PairedDaemon {
        val client = pinnedClient(url.fingerprint)
        val body = JSONObject().put("code", url.code).put("name", deviceName).toString()
            .toRequestBody("application/json".toMediaType())
        val req = Request.Builder().url("https://${urlHost(url.host)}:${url.port}/pair").post(body).build()
        try {
            client.newCall(req).execute().use { r -> return parsePairAnswer(r.code, r.body?.string().orEmpty(), url) }
        } finally {
            client.dispatcher.executorService.shutdown()
            client.connectionPool.evictAll()
        }
    }
}

/** The monitor's socket to a paired daemon: `wss`, pinned, the token on the upgrade. */
class PinnedSocketFactory(private val daemon: PairedDaemon) : SocketFactory {
    override fun open(port: Int, events: SocketEvents) = open(DaemonInfo(port, host = daemon.host), events)

    override fun open(info: DaemonInfo, events: SocketEvents): MonitorSocket {
        val client = pinnedClient(daemon.fingerprint)
        val host = urlHost(info.host ?: daemon.host)
        val req = Request.Builder()
            .url("wss://$host:${info.port}/ws")
            .header("Authorization", "Bearer ${daemon.token}")
            .build()
        val refused = java.util.concurrent.atomic.AtomicReference<Int?>(null)
        val ws = client.newWebSocket(req, object : WebSocketListener() {
            override fun onOpen(webSocket: WebSocket, response: Response) = events.onOpen()
            override fun onMessage(webSocket: WebSocket, text: String) = events.onMessage(text)
            override fun onClosed(webSocket: WebSocket, code: Int, reason: String) = events.onClosed()
            override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
                refused.set(response?.code)  // read by the connection after onClosed
                events.onClosed()
            }
        })
        return object : MonitorSocket {
            override fun send(text: String) = ws.send(text)
            override fun close() {
                ws.cancel()
                client.dispatcher.executorService.shutdown()
            }
            override val refusal: Int? get() = refused.get()
        }
    }
}
