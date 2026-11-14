// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import kotlin.test.*

// The same cases as the Swift core's CompileHolderFormatTests: the outputs must match.
class CompileHolderFormatTest {
    @Test fun shortTitleUnchanged() = assertEquals("Fix the build", Format.shortTitle("Fix the build"))

    @Test fun longTitleCutAtWordWithEllipsis() {
        val t = "Known red ledger record a test failure that was already red on develop"
        assertEquals("Known red ledger record a test failure...", Format.shortTitle(t))
    }

    @Test fun longWordIsCutAtLimit() =
        assertEquals("x".repeat(40) + "...", Format.shortTitle("x".repeat(60)))

    @Test fun holderText() {
        assertEquals("(mason) - Fix -", Format.compileHolder("mason", "Fix"))
        assertEquals("(mason)", Format.compileHolder("mason", ""))
        assertEquals("", Format.compileHolder("", ""))
    }
}
