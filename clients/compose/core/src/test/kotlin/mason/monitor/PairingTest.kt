// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import kotlin.test.*

class PairingTest {
    private val fp = "ab".repeat(32)

    @Test fun pairUrlParses() {
        assertEquals(PairUrl("studio.local", 7424, fp, "12345678"),
            PairUrl.parse("mason://pair?host=studio.local&port=7424&fp=$fp&code=12345678"))
    }

    @Test fun badPairUrlsAreRefused() {
        listOf(
            "https://pair?host=h&port=1&fp=$fp&code=12345678",
            "mason://home",
            "mason://pair?host=h&port=1&fp=short&code=12345678",
            "mason://pair?host=h&port=1&fp=$fp&code=1234",
            "mason://pair?host=h&port=0&fp=$fp&code=12345678",
            "mason://pair?port=1&fp=$fp&code=12345678",
            "mason://pair?host=h&port=99999&fp=$fp&code=12345678",
        ).forEach { assertNull(PairUrl.parse(it), it) }
    }

    @Test fun thePinMatchesOnlyTheSameFingerprint() {
        assertTrue(Pin.matches(fp, fp.uppercase()))
        assertFalse(Pin.matches(fp, "cd".repeat(32)))
        assertFalse(Pin.matches(fp, ""))
    }

    @Test fun fingerprintIsSha256OfTheDer() {
        assertEquals("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad", Pin.fingerprint("abc".toByteArray()))
    }

    @Test fun aChangedCertificateIsRejected() {
        val tm = PinningTrustManager("00".repeat(32))
        assertFailsWith<java.security.cert.CertificateException> { tm.checkServerTrusted(emptyArray(), "RSA") }
    }

    @Test fun tokensAreKeyedByFingerprint() {
        val s = MemoryTokenStore()
        s.set(fp, "t1")
        assertEquals("t1", s.token(fp.uppercase()))
        assertNull(s.token("cd".repeat(32)))
        s.set(fp, null)
        assertNull(s.token(fp))
        assertEquals("token:ab", TokenKey.account("AB"))
    }

    @Test fun serviceRecords() {
        assertEquals(fp, ServiceRecord.parse("MASON on studio", mapOf("fp" to fp, "proto" to "1", "version" to "0"))?.fingerprint)
        assertNull(ServiceRecord.parse("x", mapOf("proto" to "1")))
        assertNull(ServiceRecord.parse("x", mapOf("fp" to fp)))
    }

    @Test fun phoneLayoutOrder() {
        assertEquals(PhoneScreen.Sidebar, PhoneLayout.firstScreen)
        val o = PhoneLayout.sidebarOrder
        assertEquals(PhoneBlock.Tiles, o.first())
        assertEquals(o.indexOf(PhoneBlock.Home) - 1, o.indexOf(PhoneBlock.Tiles), "tiles sit directly above Home")
        assertTrue(o.indexOf(PhoneBlock.Home) < o.indexOf(PhoneBlock.Sections))
    }

    @Test fun selectingPushesAndBackPops() {
        var n = PhoneNav()
        assertEquals(PhoneScreen.Sidebar, n.screen)
        n = n.select(Page.Task(4))
        assertEquals(PhoneScreen.Detail(Page.Task(4)), n.screen)
        n = n.select(Page.Task(4)).back()
        assertEquals(PhoneScreen.Sidebar, n.screen)
        assertEquals(PhoneScreen.Sidebar, n.back().screen)
    }
}
