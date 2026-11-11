// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor.ui

import mason.MascotManifest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNotNull
import kotlin.test.assertNull

class MascotTest {
    @Test
    fun everyIdInTheManifestLoads() {
        for (id in MascotManifest.stateIds) for (size in listOf(20, 24)) for (scale in 1..3) {
            assertNotNull(Mascot.icon(id, size, scale), "icon $id $size@$scale")
        }
        for (id in MascotManifest.bannerIds) for (scale in 1..3) {
            assertNotNull(Mascot.bannerIcon(id, scale), "banner $id@$scale")
        }
        for (id in MascotManifest.illustrationIds) {
            assertNotNull(Mascot.illustration(id), "illustration $id")
        }
    }

    @Test
    fun everyListedFileIsOnTheClasspath() {
        for (f in MascotManifest.files) {
            assertNotNull(Mascot::class.java.classLoader.getResource(f), f)
        }
        assertEquals(110, MascotManifest.files.size)
    }

    @Test
    fun anUnknownIdGivesNothing() {
        assertNull(Mascot.icon("nonsense", 20))
        assertNull(Mascot.icon("", 20))
        assertNull(Mascot.icon("queued", 17))
        assertNull(Mascot.icon("queued", 20, 4))
        assertNull(Mascot.icon("../icons/queued-20@1x", 20))
        assertNull(Mascot.bannerIcon("nonsense"))
        assertNull(Mascot.bannerIcon("queued"))
        assertNull(Mascot.illustration("nonsense"))
        assertNull(Mascot.illustration("../brand/app_icon"))
    }

    @Test
    fun everyBannerKindMapsToAnImageOrDeliberatelyNone() {
        for ((kind, id) in BannerIcons.daemonKinds) {
            assertEquals(id, BannerIcons.forDaemonKind(kind), kind)
            if (id != null) assertNotNull(Mascot.bannerIcon(id), "$kind -> $id")
        }
        for ((_, id) in BannerIcons.clientBanners) assertNotNull(Mascot.bannerIcon(id), id)
        assertEquals("landing_frozen", BannerIcons.forDaemonKind("landing_frozen"))
        assertEquals("provider", BannerIcons.forDaemonKind("provider_budget"))
    }

    @Test
    fun anUnknownBannerKindHasNoIcon() {
        assertNull(BannerIcons.forDaemonKind("nonsense"))
        assertNull(BannerIcons.forDaemonKind(""))
    }
}
