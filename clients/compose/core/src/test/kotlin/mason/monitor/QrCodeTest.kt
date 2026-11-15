// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertTrue

class QrCodeTest {
    @Test fun capacitiesMatchTheStandard() {
        assertEquals(19, QrCode.dataCodewords(1, QrCode.Ecc.L))
        assertEquals(16, QrCode.dataCodewords(1, QrCode.Ecc.M))
        assertEquals(108, QrCode.dataCodewords(5, QrCode.Ecc.L))
        assertEquals(86, QrCode.dataCodewords(5, QrCode.Ecc.M))
        assertEquals(274, QrCode.dataCodewords(10, QrCode.Ecc.L))
        assertEquals(216, QrCode.dataCodewords(10, QrCode.Ecc.M))
        assertEquals(2956, QrCode.dataCodewords(40, QrCode.Ecc.L))
        assertEquals(2334, QrCode.dataCodewords(40, QrCode.Ecc.M))
    }

    @Test fun sizeFollowsTheTextAndFinderPatternsAreInPlace() {
        val small = QrCode.encode("hi")
        assertEquals(1, small.version)
        assertEquals(21, small.size)
        val url = "mason://pair?host=mac.local&port=7443&fp=" + "ab".repeat(32) + "&code=12345678&alt=x.ts.net"
        val q = QrCode.encode(url)
        assertEquals(q.version * 4 + 17, q.size)
        assertTrue(q.version in 6..12)
        for ((ox, oy) in listOf(0 to 0, q.size - 7 to 0, 0 to q.size - 7)) {
            for (i in 0..6) {
                assertTrue(q.modules[oy][ox + i] && q.modules[oy + 6][ox + i] && q.modules[oy + i][ox] && q.modules[oy + i][ox + 6])
            }
            assertTrue(q.modules[oy + 3][ox + 3] && !q.modules[oy + 1][ox + 1])
        }
        assertTrue(q.modules[q.size - 8][8])
    }

    @Test fun tooLongTextIsRefused() {
        assertFailsWith<IllegalArgumentException> { QrCode.encode("x".repeat(3000)) }
    }
}
