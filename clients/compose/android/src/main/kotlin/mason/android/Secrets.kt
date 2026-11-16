// SPDX-License-Identifier: GPL-3.0-or-later
package mason.android

import java.security.KeyStore
import java.util.Base64
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec
import mason.monitor.TokenKey
import mason.monitor.TokenStore

/** Seals bytes with a key the app cannot read out. The real one is the Android Keystore; tests use a plain AES key. */
interface SecretBox {
    fun seal(plain: ByteArray): ByteArray
    /** null when the bytes were not sealed by this key (changed, or the key is gone). */
    fun open(sealed: ByteArray): ByteArray?
}

/** AES-GCM under [key]. The IV is made by the cipher (the Keystore insists on that) and kept in front of the text. */
class AesGcmBox(private val key: () -> SecretKey) : SecretBox {
    override fun seal(plain: ByteArray): ByteArray {
        val c = Cipher.getInstance(TRANSFORM)
        c.init(Cipher.ENCRYPT_MODE, key())
        return c.iv + c.doFinal(plain)
    }

    override fun open(sealed: ByteArray): ByteArray? {
        if (sealed.size <= IV_BYTES) return null
        return runCatching {
            val c = Cipher.getInstance(TRANSFORM)
            c.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(TAG_BITS, sealed, 0, IV_BYTES))
            c.doFinal(sealed, IV_BYTES, sealed.size - IV_BYTES)
        }.getOrNull()
    }

    private companion object {
        const val TRANSFORM = "AES/GCM/NoPadding"
        const val IV_BYTES = 12
        const val TAG_BITS = 128
    }
}

/** The one key, made on first use inside the Android Keystore (hardware-backed where the phone has it). */
object AndroidKeystore {
    private const val PROVIDER = "AndroidKeyStore"
    const val ALIAS = "mason.token.key"

    fun box(alias: String = ALIAS): SecretBox = AesGcmBox { key(alias) }

    private fun key(alias: String): SecretKey {
        val ks = KeyStore.getInstance(PROVIDER).apply { load(null) }
        (ks.getKey(alias, null) as? SecretKey)?.let { return it }
        val spec = android.security.keystore.KeyGenParameterSpec.Builder(
            alias,
            android.security.keystore.KeyProperties.PURPOSE_ENCRYPT or android.security.keystore.KeyProperties.PURPOSE_DECRYPT,
        )
            .setBlockModes(android.security.keystore.KeyProperties.BLOCK_MODE_GCM)
            .setEncryptionPaddings(android.security.keystore.KeyProperties.ENCRYPTION_PADDING_NONE)
            .setKeySize(256)
            .build()
        return KeyGenerator.getInstance(android.security.keystore.KeyProperties.KEY_ALGORITHM_AES, PROVIDER)
            .apply { init(spec) }.generateKey()
    }
}

/** Small string settings. The real one is `SharedPreferences`; tests use a map. */
interface KeyValue {
    fun get(key: String): String?
    fun put(key: String, value: String?)
}

class MemoryKeyValue : KeyValue {
    val items = mutableMapOf<String, String>()
    override fun get(key: String) = items[key]
    override fun put(key: String, value: String?) { if (value == null) items.remove(key) else items[key] = value }
}

/**
 * Tokens, sealed with [box] and kept in [kv] under the fingerprint they were issued for. A token whose
 * sealing key has gone (the Keystore was cleared, the app restored on another phone) reads as no token,
 * so the phone asks to be paired again rather than sending garbage.
 */
class KeystoreTokenStore(private val kv: KeyValue, private val box: SecretBox) : TokenStore {
    override fun token(fingerprint: String): String? {
        val stored = kv.get(TokenKey.account(fingerprint)) ?: return null
        val sealed = runCatching { Base64.getDecoder().decode(stored) }.getOrNull() ?: return null
        return box.open(sealed)?.toString(Charsets.UTF_8)
    }

    override fun set(fingerprint: String, token: String?) {
        val account = TokenKey.account(fingerprint)
        if (token == null) kv.put(account, null)
        else kv.put(account, Base64.getEncoder().encodeToString(box.seal(token.toByteArray(Charsets.UTF_8))))
    }
}
