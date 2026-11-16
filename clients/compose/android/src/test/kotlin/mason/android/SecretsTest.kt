// SPDX-License-Identifier: GPL-3.0-or-later
package mason.android

import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import kotlin.test.*

class SecretsTest {
    private fun key(): SecretKey = KeyGenerator.getInstance("AES").apply { init(256) }.generateKey()
    private val fp = "ab".repeat(32)

    @Test fun aTokenIsSealedAtRestAndReadBack() {
        val kv = MemoryKeyValue()
        val store = KeystoreTokenStore(kv, AesGcmBox { key })
        store.set(fp, "secret-token")
        assertEquals("secret-token", store.token(fp))
        assertTrue(kv.items.values.none { "secret-token" in it }, "the token is not stored in the clear: ${kv.items}")
    }

    private val key = key()

    @Test fun tokensAreKeyedByFingerprintAndForgetRemovesOne() {
        val store = KeystoreTokenStore(MemoryKeyValue(), AesGcmBox { key })
        val other = "cd".repeat(32)
        store.set(fp, "one")
        store.set(other, "two")
        assertEquals("one", store.token(fp.uppercase()), "the fingerprint's case does not matter")
        assertEquals("two", store.token(other))
        store.set(fp, null)
        assertNull(store.token(fp))
        assertEquals("two", store.token(other))
    }

    @Test fun anotherCertificateHasNoToken() {
        val store = KeystoreTokenStore(MemoryKeyValue(), AesGcmBox { key })
        store.set(fp, "one")
        assertNull(store.token("cd".repeat(32)), "a daemon that changed its certificate must be paired again")
    }

    @Test fun aGoneKeyReadsAsNoTokenNotAnError() {
        val kv = MemoryKeyValue()
        KeystoreTokenStore(kv, AesGcmBox { key }).set(fp, "one")
        assertNull(KeystoreTokenStore(kv, AesGcmBox { key() }).token(fp))
    }

    @Test fun damagedStorageReadsAsNoToken() {
        val kv = MemoryKeyValue()
        val store = KeystoreTokenStore(kv, AesGcmBox { key })
        store.set(fp, "one")
        val account = mason.monitor.TokenKey.account(fp)
        val raw = java.util.Base64.getDecoder().decode(kv.items.getValue(account))
        raw[raw.size - 1] = (raw[raw.size - 1].toInt() xor 1).toByte()
        kv.items[account] = java.util.Base64.getEncoder().encodeToString(raw)
        assertNull(store.token(fp))
        kv.items[account] = "not base64 !!"
        assertNull(store.token(fp))
        kv.items[account] = ""
        assertNull(store.token(fp))
    }

    @Test fun eachSealingUsesAFreshIv() {
        val box = AesGcmBox { key }
        assertNotEquals(box.seal("x".toByteArray()).toList(), box.seal("x".toByteArray()).toList())
    }
}
