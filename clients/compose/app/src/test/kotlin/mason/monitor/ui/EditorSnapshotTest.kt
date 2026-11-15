// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor.ui

import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.test.ExperimentalTestApi
import androidx.compose.ui.test.assertIsEnabled
import androidx.compose.ui.test.hasAnyAncestor
import androidx.compose.ui.test.hasTestTag
import androidx.compose.ui.test.hasSetTextAction
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.isPopup
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performTextInput
import androidx.compose.ui.test.runComposeUiTest
import mason.monitor.EditorJson
import mason.monitor.MonitorModel
import mason.v1.Command
import mason.v1.EditorEdge
import mason.v1.EditorLane
import mason.v1.EditorNode
import mason.v1.EditorView
import mason.v1.Inspector
import kotlin.test.Test
import kotlin.test.assertEquals

/**
 * s299: the pipeline editor driven by its controls (add a step from the palette, connect it, edit
 * a form field, undo), with the operations the controls send checked against a small stand-in for
 * the daemon's draft and the drawn tree checked against the model. Compose's offscreen test rule:
 * nothing appears on the screen.
 */
@OptIn(ExperimentalTestApi::class)
class EditorSnapshotTest {
    private data class Draft(
        val steps: List<Pair<String, String>> = listOf("develop" to "developing"),
        val edges: List<Triple<String, String, String>> = emptyList(),
        val fields: Map<String, String> = emptyMap(),
    )

    /** The daemon's draft as far as these operations go; every command is answered with a new view. */
    private class FakeDaemon(val model: MonitorModel) {
        var draft = Draft()
        val history = mutableListOf<Draft>()
        val ops = mutableListOf<Map<String, Any?>>()

        init {
            model.project = "web"
            model.sender = { c: Command -> receive(c) }
            publish()
        }

        fun receive(c: Command) {
            val raw = c.raw ?: return
            if (raw.path.endsWith("/editor/undo")) {
                history.removeLastOrNull()?.let { draft = it }
            } else if (raw.path.endsWith("/editor/op")) {
                val o = EditorJson.parse(raw.json)
                ops += o.keys().asSequence().associateWith { k -> if (o.isNull(k)) null else o.get(k) }
                history += draft
                when (o.getString("op")) {
                    "insert_step" -> draft = draft.copy(steps = draft.steps + (o.getString("name") to o.getString("category")))
                    "set_edge" -> {
                        val kept = draft.edges.filterNot { it.first == o.getString("from") && it.second == o.getString("edge") }
                        draft = draft.copy(edges = if (o.isNull("to")) kept else kept + Triple(o.getString("from"), o.getString("edge"), o.getString("to")))
                    }
                    "set_field" -> draft = draft.copy(fields = draft.fields + (o.getString("key") to o.getString("value")))
                }
            }
            publish()
        }

        fun publish() {
            model.editor = EditorView(
                project = "web",
                lanes = listOf(EditorLane("developing", "Developing"), EditorLane("testing", "Testing")),
                nodes = draft.steps.mapIndexed { i, (n, cat) ->
                    EditorNode(name = n, kind = if (i == 0) "agent" else "test", category = cat, is_start = i == 0, x = i * 190L, y = if (cat == "testing") 140L else 0L)
                },
                edges = draft.edges.map { EditorEdge(from = it.first, kind = it.second, to = it.third) },
                step_names = draft.steps.map { it.first },
                selected = "develop",
                dirty = history.isNotEmpty(),
                can_undo = history.isNotEmpty(),
                palette = listOf("""{"type":"test","categories":["testing"],"available":true,"about":"run a suite","fields":{"suite":"unit"}}"""),
                inspector = Inspector(
                    step = "develop", kind = "agent",
                    schema = """{"properties":{"model":{"type":"string","title":"Model"}},"x-order":["model"]}""",
                    values = draft.fields["model"]?.let { """{"model":"$it"}""" } ?: "{}",
                ),
            )
        }
    }

    @Test fun addConnectEditAndUndoThroughTheControls() = runComposeUiTest {
        val model = MonitorModel()
        val daemon = FakeDaemon(model)
        setContent { CompositionLocalProvider(LocalTheme provides Theme(dark = false, scale = 1.0)) { EditorPage(model) } }

        // Add a step from the palette.
        onNodeWithTag("palette:test").performClick()
        waitForIdle()
        assertEquals("insert_step", daemon.ops.last()["op"])
        assertEquals("test-1", daemon.ops.last()["name"])
        assertEquals("develop", daemon.ops.last()["after"])
        assertEquals(listOf("develop", "test-1"), daemon.draft.steps.map { it.first })
        onNodeWithTag("node:test-1").assertExists()

        // Connect it: develop's success edge goes to the new step.
        onNode(hasText("(nowhere)") and hasAnyAncestor(hasTestTag("edge:next"))).performClick()
        onNode(hasText("test-1") and hasAnyAncestor(isPopup())).performClick()
        waitForIdle()
        assertEquals("set_edge", daemon.ops.last()["op"])
        assertEquals(listOf(Triple("develop", "next", "test-1")), daemon.draft.edges)
        assertEquals(listOf("test-1"), model.editor?.edges?.map { it.to })

        // Edit a form field.
        onNode(hasAnyAncestor(hasTestTag("field:model")) and hasSetTextAction()).performTextInput("deep")
        onNode(hasText("Set") and hasAnyAncestor(hasTestTag("field:model"))).performClick()
        waitForIdle()
        assertEquals("set_field", daemon.ops.last()["op"])
        assertEquals("model", daemon.ops.last()["key"])
        assertEquals("deep", daemon.draft.fields["model"])

        // Undo the three edits, each by the Undo button; the draft returns step by step.
        onNodeWithTag("undo").assertIsEnabled().performClick()
        waitForIdle()
        assertEquals(emptyMap(), daemon.draft.fields)
        onNodeWithTag("undo").performClick()
        waitForIdle()
        assertEquals(emptyList(), daemon.draft.edges)
        onNodeWithTag("undo").performClick()
        waitForIdle()
        assertEquals(listOf("develop"), daemon.draft.steps.map { it.first })
        onNodeWithTag("node:test-1").assertDoesNotExist()
        assertEquals(3, daemon.ops.size, "undo is a call, not an operation")
    }
}
