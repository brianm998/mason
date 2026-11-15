// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor.ui

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

/** s329: the task detail's branch and worktree copy buttons are labelled and copy the exact value. */
class CopyButtonTest {
    @Test
    fun bothButtonsAreLabelledAndCarryTheValues() {
        val t = CopyTarget.detail("mason/mason/t1", "/w/mason/1")
        assertEquals(listOf("Copy branch name", "Copy worktree"), t.map { it.label })
        val copied = mutableListOf<String>()
        val stub: (String) -> Unit = { copied += it }
        t.forEach { stub(it.value) }
        assertEquals(listOf("mason/mason/t1", "/w/mason/1"), copied)
    }

    @Test
    fun emptyValuesHideTheirButton() {
        assertEquals(listOf("Copy worktree"), CopyTarget.detail("", "/w").map { it.label })
        assertEquals(listOf("Copy branch name"), CopyTarget.detail("b", "").map { it.label })
        assertTrue(CopyTarget.detail("", "").isEmpty())
    }

    @Test
    fun copiedReplacesTheLabel() {
        val t = CopyTarget("Copy worktree", "/w")
        assertEquals("Copy worktree", t.shown(false))
        assertEquals("Copied", t.shown(true))
    }
}
