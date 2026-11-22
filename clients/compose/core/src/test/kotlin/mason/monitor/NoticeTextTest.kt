// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import kotlin.test.*

/** #951: a notification is one short line; the rest is detail, and the tasks it names are actions. */
class NoticeTextTest {
    @Test
    fun aShortTextHasNoDetailAndALongOneKeepsItAll() {
        val short = NoticeText.oneLine("Paused for p")
        assertEquals("Paused for p", short.text)
        assertFalse(short.hasDetails)
        val long = "Landing is frozen for p:\nonly the fix task lands " + "until green ".repeat(12)
        val one = NoticeText.oneLine(long)
        assertFalse(one.text.contains('\n'))
        assertTrue(one.text.length <= NoticeText.LIMIT)
        assertEquals(long.trim(), one.detail)
        assertTrue(one.hasDetails)
    }

    @Test
    fun aProducersDetailIsKept() {
        val one = NoticeText.oneLine("p: 2 red CI jobs being fixed", "p: 2 red CI jobs being fixed: #7, #9")
        assertEquals("p: 2 red CI jobs being fixed", one.text)
        assertEquals(listOf(7L, 9L), NoticeText.taskNumbers(one.text, one.detail))
    }
}
