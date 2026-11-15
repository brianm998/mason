// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertTrue

class DevicesTest {
    @Test fun lanStatusParses() {
        val on = LanStatus.parse("""{"enabled":true,"port":7443,"hosts":["a.local","b.ts.net"]}""")
        assertTrue(on.enabled)
        assertEquals(listOf("a.local", "b.ts.net"), on.hosts)
        assertFalse(LanStatus.parse("""{"enabled":false}""").enabled)
        assertFalse(LanStatus.parse("garbage").enabled)
    }

    @Test fun liveDevicesSortBySeenAndRevokedAreApart() {
        val l = DeviceList.parse(
            """{"devices":[
             {"id":1,"name":"old","role":"owner","created_at":1,"last_seen":100,"revoked_at":null},
             {"id":2,"name":"new","role":"viewer","created_at":2,"last_seen":900,"revoked_at":null},
             {"id":3,"name":"never","role":"viewer","created_at":3,"last_seen":null,"revoked_at":null},
             {"id":4,"name":"gone1","role":"owner","created_at":4,"last_seen":950,"revoked_at":1000},
             {"id":5,"name":"gone2","role":"owner","created_at":5,"last_seen":null,"revoked_at":2000}]}""",
        )
        assertEquals(listOf(2L, 1L, 3L), l.active.map { it.id })
        assertEquals(listOf(5L, 4L), l.revoked.map { it.id })
        assertTrue(l.revoked.all { it.isRevoked })
        assertEquals(DeviceList(), DeviceList.parse("x"))
    }

    @Test fun lastSeenText() {
        assertEquals("never", DeviceList.lastSeenText(null, 5))
        assertEquals("just now", DeviceList.lastSeenText(1_000, 30_000))
        assertEquals("5 min ago", DeviceList.lastSeenText(0, 300_000))
        assertEquals("3 h ago", DeviceList.lastSeenText(0, 3 * 3_600_000L))
        assertEquals("2 d ago", DeviceList.lastSeenText(0, 2 * 86_400_000L))
    }

    @Test fun countdownRunsOnAnInjectedClock() {
        val json = """{"code":"12345678","role":"viewer","expires_in_s":300,"hosts":["h.local"],"url":"mason://pair?x=1"}"""
        val o = PairOffer.parse(json, 10_000)!!
        assertEquals("viewer", o.role)
        assertEquals(PairOffer.State.Valid(300), o.state(10_000))
        assertEquals(PairOffer.State.Valid(300), o.state(10_001))
        assertEquals(PairOffer.State.Valid(1), o.state(10_000 + 299_001))
        assertEquals(PairOffer.State.Expired, o.state(10_000 + 300_000))
        assertEquals(PairOffer.State.Valid(310), o.state(0))
        assertEquals("4:59", PairOffer.clock(299))
        assertNull(PairOffer.parse("""{"code":"12","url":"u"}""", 0))
    }
}
