// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import kotlin.test.*

// The same cases as the Swift core's CommandDialogTests.
class CommandDialogTest {
    @Test fun noCommandMeansNoDialog() = assertNull(Format.commandDialog("#88", ""))

    @Test fun dialogIsTitledWithTheDescriptionAndHoldsTheCommand() {
        val d = Format.commandDialog("python3 - <<'EOF' …", "python3 - <<'EOF'\nprint(1)\nEOF")
        assertEquals("python3 - <<'EOF' …", d?.title)
        assertEquals("python3 - <<'EOF'\nprint(1)\nEOF", d?.command)
    }

    @Test fun openKeepsTheCommandAndCloseClears() {
        val m = MonitorModel()
        assertNull(m.commandDialog)
        m.openCommand("", "")
        assertNull(m.commandDialog)
        m.openCommand("make", "make -j8\nmake test")
        assertEquals("make -j8\nmake test", m.commandDialog?.command)
        assertEquals("make", m.commandDialog?.title)
        m.closeCommand()
        assertNull(m.commandDialog)
    }
}
