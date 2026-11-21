// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor.ui

import kotlin.test.Test
import kotlin.test.assertEquals

class CountTextTest {
    @Test
    fun smallCountsArePlainNumbers() {
        assertEquals("0", countText(0))
        assertEquals("999", countText(999))
    }

    @Test
    fun thousandsUseK() {
        assertEquals("1.0k", countText(1000))
        assertEquals("12.3k", countText(12_300))
        assertEquals("1000.0k".length, countText(999_999).length)
    }

    @Test
    fun millionsUseM() {
        assertEquals("1.0M", countText(1_000_000))
        assertEquals("2.5M", countText(2_500_000))
    }
}
