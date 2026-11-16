// SPDX-License-Identifier: GPL-3.0-or-later
package mason.android

import mason.monitor.PairedDaemon
import mason.monitor.PhoneNav
import mason.monitor.PhoneScreen
import mason.monitor.Page
import mason.monitor.PhoneLayout
import mason.monitor.PhoneBlock
import mason.monitor.ServiceRecord
import kotlin.test.*

class RemoteDiscoveryTest {
    private val fp = "ab".repeat(32)
    private val paired = PairedDaemon("m.local", 7443, fp, "t", "owner", listOf("m.ts.net", "100.1.1.1"))
    private fun found(fingerprint: String, host: String, port: Int) = ResolvedService(ServiceRecord("MASON on m", 1, fingerprint, "1"), host, port)
    private fun source(vararg s: ResolvedService) = object : ServiceSource { override fun services() = s.toList() }

    @Test fun nsdFirstThenTheNameThenTheExtraHosts() {
        val d = RemoteDiscovery(paired, source(found(fp, "192.168.1.5", 9000)))
        assertEquals(
            listOf(Endpoint("192.168.1.5", 9000, true), Endpoint("m.local", 7443, false), Endpoint("m.ts.net", 7443, false), Endpoint("100.1.1.1", 7443, false)),
            d.candidates(),
        )
    }

    @Test fun aRecordWithAnotherFingerprintIsIgnored() {
        val d = RemoteDiscovery(paired, source(found("cd".repeat(32), "10.0.0.9", 1)))
        assertEquals(listOf("m.local", "m.ts.net", "100.1.1.1"), d.candidates().map { it.host })
    }

    @Test fun withoutNsdTheOrderIsTheNameThenTheExtraHosts() {
        assertEquals(listOf("m.local", "m.ts.net", "100.1.1.1"), RemoteDiscovery(paired, null).candidates().map { it.host })
    }

    @Test fun anAttemptThatFailsMovesToTheNextAddressAndAWelcomedOneKeepsItsOwn() {
        val d = RemoteDiscovery(paired, null)
        val picked = mutableListOf<String>()
        d.onChoose { e, _ -> picked += e.host }
        assertEquals("m.local", d.current()?.host)
        assertEquals("m.local", d.current()?.host, "the same all through one attempt")
        d.attemptEnded(welcomed = false)
        assertEquals("m.ts.net", d.current()?.host)
        d.attemptEnded(welcomed = false)
        assertEquals("100.1.1.1", d.current()?.host)
        d.attemptEnded(welcomed = false)
        assertEquals("m.local", d.current()?.host, "wraps round")
        d.attemptEnded(welcomed = true)
        assertEquals("m.local", d.current()?.host, "a welcomed attempt starts the next from the first")
        assertEquals(listOf("m.local", "m.ts.net", "100.1.1.1", "m.local", "m.local"), picked)
    }

    @Test fun nsdThatAppearsLaterIsUsedOnTheNextAttempt() {
        var list = emptyList<ResolvedService>()
        val d = RemoteDiscovery(paired, object : ServiceSource { override fun services() = list })
        assertEquals("m.local", d.current()?.host)
        list = listOf(found(fp, "192.168.1.5", 9000))
        d.attemptEnded(welcomed = true)
        val c = d.current()!!
        assertEquals("192.168.1.5", c.host)
        assertEquals(9000, c.port)
    }

    @Test fun ipv6LiteralsAreBracketed() {
        assertEquals("[fe80::1]", urlHost("fe80::1"))
        assertEquals("[fe80::1]", urlHost("[fe80::1]"))
        assertEquals("m.local", urlHost("m.local"))
    }

    // ---- the phone's navigation (PhoneNav): the sidebar first, a row pushes, Back pops ----

    @Test fun theSidebarIsFirstAChoicePushesAndBackPops() {
        var n = PhoneNav()
        assertEquals(PhoneScreen.Sidebar, n.screen)
        n = n.select(Page.Landing).select(Page.Task(7))
        assertEquals(PhoneScreen.Detail(Page.Task(7)), n.screen)
        assertEquals(PhoneScreen.Detail(Page.Landing), n.back().screen)
        assertEquals(PhoneScreen.Sidebar, n.back().back().screen)
        assertEquals(PhoneScreen.Sidebar, n.back().back().back().screen, "Back on the sidebar stays on it")
        assertEquals(n, n.select(Page.Task(7)), "choosing the page already shown pushes nothing")
    }

    @Test fun theSidebarBlocksAreInTheDesignedOrderAndTheDifferencesAreNamed() {
        assertEquals(listOf(PhoneBlock.Tiles, PhoneBlock.Home, PhoneBlock.Sections, PhoneBlock.Landing, PhoneBlock.Usage), PhoneLayout.sidebarOrder)
        assertEquals(listOf("graph editing", "log deep-dives", "agent sessions"), PhoneLayout.phoneHasNo)
    }

    @Test fun pairedStoreKeepsTheExtraHostsAndRefusesDamage() {
        val kv = MemoryKeyValue()
        val store = KeyValuePairedStore(kv)
        assertNull(store.load())
        val r = PairedRecord("m.local", 7443, fp, "owner", listOf("m.ts.net"))
        store.save(r)
        assertEquals(r, store.load())
        kv.put(KeyValuePairedStore.KEY, "m.local\nnope\n$fp\nowner")
        assertNull(store.load())
        kv.put(KeyValuePairedStore.KEY, "m.local\n7443\nshort\nowner")
        assertNull(store.load())
        store.save(r)
        store.clear()
        assertNull(store.load())
    }
}
