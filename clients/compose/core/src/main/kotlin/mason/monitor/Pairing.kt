// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import java.net.URI
import java.net.URLDecoder
import java.security.MessageDigest
import java.security.cert.CertificateException
import java.security.cert.X509Certificate
import javax.net.ssl.X509TrustManager

/** What `mason pair` shows as a QR code: `mason://pair?host=H&port=P&fp=SHA256HEX&code=12345678`. */
data class PairUrl(val host: String, val port: Int, val fingerprint: String, val code: String) {
    companion object {
        /** null for anything that is not a well-formed pairing address. */
        fun parse(text: String): PairUrl? {
            val uri = runCatching { URI(text.trim()) }.getOrNull() ?: return null
            if (uri.scheme != "mason" || uri.host != "pair") return null
            val q = (uri.rawQuery ?: return null).split("&").mapNotNull {
                val i = it.indexOf('=')
                if (i < 0) null else it.substring(0, i) to URLDecoder.decode(it.substring(i + 1), "UTF-8")
            }.toMap()
            val host = q["host"]?.takeIf { it.isNotEmpty() } ?: return null
            val port = q["port"]?.toIntOrNull()?.takeIf { it in 1..65535 } ?: return null
            val fp = q["fp"]?.lowercase()?.takeIf(Pin::isFingerprint) ?: return null
            val code = q["code"]?.takeIf(Pin::isCode) ?: return null
            return PairUrl(host, port, fp, code)
        }
    }
}

object Pin {
    fun isFingerprint(s: String) = s.length == 64 && s.all { it in '0'..'9' || it in 'a'..'f' }
    fun isCode(s: String) = s.length == 8 && s.all { it in '0'..'9' }

    fun fingerprint(der: ByteArray): String =
        MessageDigest.getInstance("SHA-256").digest(der).joinToString("") { "%02x".format(it) }

    /** Constant-time comparison of the fingerprint shown with the one pinned. */
    fun matches(shown: String, pinned: String): Boolean =
        MessageDigest.isEqual(shown.lowercase().toByteArray(), pinned.lowercase().toByteArray())
}

/** Accepts a server only when its certificate's SHA-256 is the pinned one. */
class PinningTrustManager(private val pinned: String) : X509TrustManager {
    override fun checkClientTrusted(chain: Array<X509Certificate>, authType: String) =
        throw CertificateException("no client certificates")

    override fun checkServerTrusted(chain: Array<X509Certificate>, authType: String) {
        val leaf = chain.firstOrNull() ?: throw CertificateException("no certificate")
        if (!Pin.matches(Pin.fingerprint(leaf.encoded), pinned)) {
            throw CertificateException("the certificate is not the one this device paired with")
        }
    }

    override fun getAcceptedIssuers(): Array<X509Certificate> = emptyArray()
}

/** A paired daemon: where it is, the certificate to expect, and our token. */
data class PairedDaemon(val host: String, val port: Int, val fingerprint: String, val token: String, val role: String)

/** Where tokens live (the Keystore-backed store on Android). Keyed by fingerprint: another certificate means pairing again. */
interface TokenStore {
    fun token(fingerprint: String): String?
    fun set(fingerprint: String, token: String?)
}

object TokenKey {
    fun account(fingerprint: String) = "token:" + fingerprint.lowercase()
}

class MemoryTokenStore : TokenStore {
    private val items = mutableMapOf<String, String>()
    override fun token(fingerprint: String) = items[TokenKey.account(fingerprint)]
    override fun set(fingerprint: String, token: String?) {
        if (token == null) items.remove(TokenKey.account(fingerprint)) else items[TokenKey.account(fingerprint)] = token
    }
}

/** A `_mason._tcp` record's TXT: proto, fp, version; the instance is "MASON on <host>". */
data class ServiceRecord(val instance: String, val proto: Int, val fingerprint: String, val version: String) {
    companion object {
        const val TYPE = "_mason._tcp"
        fun parse(instance: String, txt: Map<String, String>): ServiceRecord? {
            val fp = txt["fp"]?.lowercase()?.takeIf(Pin::isFingerprint) ?: return null
            val proto = txt["proto"]?.toIntOrNull() ?: return null
            return ServiceRecord(instance, proto, fp, txt["version"] ?: "")
        }
    }
}
