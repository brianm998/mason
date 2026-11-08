// SPDX-License-Identifier: GPL-3.0-or-later

package mason.sample

import org.junit.Assert.assertEquals
import org.junit.Test

class SampleTest {
    @Test
    fun greetsAStrangerWhenThereIsNoName() {
        assertEquals("Hello, stranger", greeting(""))
    }

    @Test
    fun greetsByName() {
        assertEquals("Hello, Ada", greeting("Ada"))
    }
}
