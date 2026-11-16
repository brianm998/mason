// SPDX-License-Identifier: GPL-3.0-or-later
package mason.android

import mason.monitor.PairUrl
import mason.monitor.PairedDaemon
import mason.monitor.ServiceRecord
import mason.monitor.MemoryTokenStore
import kotlin.test.*

class PairingScreenTest {
    private val fp = "ab".repeat(32)
    private val link = "mason://pair?host=mac.local&port=7443&fp=$fp&code=12345678&alt=m.ts.net"
    private val svc = ResolvedService(ServiceRecord("MASON on mac", 1, fp, "1"), "192.168.1.5", 7443)

    @Test fun aScanFillsTheAddressAndTheCode() {
        val s = PairingScreenState().scanned(link)
        assertEquals("mac.local", s.address?.host)
        assertEquals(listOf("m.ts.net"), s.address?.alternates)
        assertEquals("12345678", s.code)
        assertTrue(s.canSubmit)
        assertNull(s.error)
    }

    @Test fun somethingThatIsNotAPairingAddressChangesNothingButTheError() {
        val before = PairingScreenState().scanned(link)
        val after = before.scanned("https://example.com")
        assertEquals("That is not a MASON pairing code.", after.error)
        assertEquals(before.address, after.address)
    }

    @Test fun choosingANearbyDaemonNeedsTheTypedCode() {
        var s = PairingScreenState().choose(svc)
        assertEquals(PairingScreenState.Path.Nearby, s.path)
        assertFalse(s.canSubmit)
        s = s.withCode("12ab 34-56789")
        assertEquals("12345678", s.code, "digits only, at most 8")
        val r = s.request!!
        assertEquals(PairUrl("192.168.1.5", 7443, fp, "12345678"), r)
    }

    @Test fun aScannedLinkKeepsItsExtraHostsAndAnEditedCode() {
        val s = PairingScreenState().scanned(link).withCode("87654321")
        assertEquals(listOf("m.ts.net"), s.request?.alternates)
        assertEquals("87654321", s.request?.code)
    }

    @Test fun beginNeedsATargetAndACodeAndRefusesTwice() {
        val (s0, none) = PairingScreenState().begin()
        assertNull(none)
        assertEquals("Choose a daemon or scan its QR code first.", s0.error)
        val (s1, none1) = PairingScreenState().choose(svc).withCode("123").begin()
        assertNull(none1)
        assertEquals("The code is 8 digits.", s1.error)
        val ready = PairingScreenState().scanned(link)
        val (busy, req) = ready.begin()
        assertNotNull(req)
        assertTrue(busy.busy)
        assertNull(busy.begin().second, "a second tap while busy does nothing")
    }

    @Test fun wordsForEachRefusalKeepTheDaemonsOwnMessage() {
        assertTrue(PairingMessage.text(400, "bad code").contains("wrong (bad code)"))
        assertTrue(PairingMessage.text(410, "").contains("expired"))
        assertTrue(PairingMessage.text(429, "").contains("Too many"))
        assertTrue(PairingMessage.text(403, "").contains("LAN access"))
        assertTrue(PairingMessage.text(500, "x").contains("(500)"))
        assertTrue(PairingMessage.text(javax.net.ssl.SSLHandshakeException("pin")).contains("certificate did not match"))
    }

    @Test fun theNearbyWarningSaysScanningIsStronger() {
        assertTrue(PairingScreenState.NEARBY_WARNING.contains("Scanning the QR code is the stronger check"))
        assertEquals("abababab", PairingScreenState.shortFingerprint(fp))
    }

    private fun flow(client: PairClient, store: PairedStore = MemoryPairedStore(), tokens: MemoryTokenStore = MemoryTokenStore(), paired: () -> Unit = {}) =
        PairingFlow(store, tokens, client, "Pixel", background = { it() }, ui = { it() }, onPaired = paired)

    @Test fun aSuccessfulPairingStoresTheTokenAndTheRecordThenResetsTheScreen() {
        val store = MemoryPairedStore()
        val tokens = MemoryTokenStore()
        var told = 0
        val f = flow({ u, name ->
            assertEquals("Pixel", name)
            PairedDaemon(u.host, u.port, u.fingerprint, "tok", "owner", u.alternates)
        }, store, tokens) { told++ }
        f.open(link)
        f.submit()
        assertEquals("tok", tokens.token(fp))
        assertEquals(PairedRecord("mac.local", 7443, fp, "owner", listOf("m.ts.net")), store.load())
        assertEquals(PairingScreenState(), f.state)
        assertEquals(1, told)
    }

    @Test fun aRefusalShowsTheMessageAndStoresNothing() {
        val store = MemoryPairedStore()
        val tokens = MemoryTokenStore()
        val f = flow({ _, _ -> throw PairingRefused(410, "expired") }, store, tokens)
        f.open(link)
        f.submit()
        assertNull(store.load())
        assertNull(tokens.token(fp))
        assertFalse(f.state.busy)
        assertTrue(f.state.error!!.contains("expired"))
    }

    @Test fun theDaemonsAnswerIsReadStrictly() {
        val u = PairUrl.parse(link)!!
        val d = parsePairAnswer(200, """{"token":"t","role":"viewer"}""", u)
        assertEquals("viewer", d.role)
        assertEquals(listOf("m.ts.net"), d.alternates)
        assertFailsWith<PairingBadAnswer> { parsePairAnswer(200, """{"role":"viewer"}""", u) }
        assertFailsWith<PairingBadAnswer> { parsePairAnswer(200, "nope", u) }
        val e = assertFailsWith<PairingRefused> { parsePairAnswer(400, """{"error":"wrong code"}""", u) }
        assertEquals(400, e.status)
        assertEquals("wrong code", e.daemon)
    }
}
