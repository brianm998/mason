// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor.ui

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import mason.monitor.EditorJson
import mason.monitor.MonitorModel
import mason.v1.EditorNode
import mason.v1.EditorView
import org.json.JSONArray
import org.json.JSONObject

/** Canvas coordinates arrive as 64-bit numbers; one unit is one dp. */
private val Long.dp get() = this.toInt().dp

private const val NODE_W = 170
private const val NODE_H = 70
private const val LANE_H = 140

/**
 * The pipeline editor (Part 18). The daemon validates, lays out and computes everything
 * (`EditorView`); this draws it and sends small operations back. Nothing here knows a
 * pipeline rule: a step's form is built from the JSON Schema the daemon sends.
 */
@Composable
fun EditorPage(model: MonitorModel) {
    val theme = LocalTheme.current
    var tab by remember { mutableStateOf(0) }
    var message by remember { mutableStateOf("") }
    val e = model.editor
    Column(Modifier.fillMaxSize().padding(theme.spacingLarge), verticalArrangement = Arrangement.spacedBy(theme.spacing)) {
        if (e == null || e.project.isEmpty()) {
            Text("No project to edit.", style = theme.body(color = theme.muted))
            return@Column
        }
        Row(horizontalArrangement = Arrangement.spacedBy(theme.spacing), verticalAlignment = Alignment.CenterVertically) {
            Text("Pipeline: ${e.project}", style = theme.title())
            if (e.protected_) Text("MASON's own: .mason/ is protected", style = theme.body(color = theme.muted))
            Spacer(Modifier.weight(1f))
            Button(onClick = { model.editorCall("undo") }, enabled = e.can_undo, modifier = Modifier.testTag("undo")) { Text("Undo") }
            Button(onClick = { model.editorCall("redo") }, enabled = e.can_redo, modifier = Modifier.testTag("redo")) { Text("Redo") }
            Button(onClick = { model.editorCall("discard") }, enabled = e.dirty, modifier = Modifier.testTag("discard")) { Text("Discard") }
            Button(onClick = { model.editorCall("simulate"); tab = 3 }, modifier = Modifier.testTag("simulate")) { Text("Simulate") }
            OutlinedTextField(message, { message = it }, label = { Text("Message") }, singleLine = true, modifier = Modifier.width(220.dp))
            Button(onClick = { model.editorCall("apply", mapOf("message" to message)); message = "" }, enabled = e.dirty, modifier = Modifier.testTag("apply")) { Text("Apply") }
        }
        Text(if (e.dirty) "Draft changed" else "Draft is the base", style = theme.body(color = theme.muted))
        e.last_error.forEach { Text("Refused: $it", style = theme.body(color = theme.attention)) }
        model.lastResult?.let { Text(it, style = theme.body(color = theme.attention)) }
        e.problems.forEachIndexed { i, p -> Text(p, style = theme.body(color = theme.attention), modifier = Modifier.testTag("problem:$i")) }
        TabRow(selectedTabIndex = tab, modifier = Modifier.width(520.dp).testTag("editor-tabs")) {
            listOf("Graph", "Tests", "Landing", "Preview").forEachIndexed { i, t -> Tab(selected = tab == i, onClick = { tab = i }, text = { Text(t) }) }
        }
        when (tab) {
            0 -> GraphTab(model, e)
            1 -> TestsTab(model, e)
            2 -> LandingTab(model, e)
            else -> PreviewTab(model, e)
        }
    }
}

@Composable
private fun GraphTab(model: MonitorModel, e: EditorView) {
    val theme = LocalTheme.current
    Row(Modifier.fillMaxSize(), horizontalArrangement = Arrangement.spacedBy(theme.spacingLarge)) {
        BoxWithConstraints(Modifier.weight(1f).fillMaxHeight().background(theme.surface)) {
            val viewport = maxWidth.value.toDouble()
            Box(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).horizontalScroll(rememberScrollState())) {
                Canvas(model, e, viewport)
            }
        }
        Column(Modifier.width(340.dp).fillMaxHeight().verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(theme.spacingLarge)) {
            Palette(model, e)
            if (e.inspector != null) InspectorPanel(model, e)
            else Text("Select a step to edit it.", style = theme.body(color = theme.muted))
        }
    }
}

@Composable
private fun Canvas(model: MonitorModel, e: EditorView, viewport: Double) {
    val theme = LocalTheme.current
    val xs = e.nodes.associate { it.name to it.x.toDouble() }
    val back = e.edges.filter { it.kind == "on_red" }.mapNotNull { ed ->
        val f = xs[ed.from] ?: return@mapNotNull null
        val t = xs[ed.to] ?: return@mapNotNull null
        mason.monitor.GraphLayout.BackEdge(f, t, ed.bound > 0)
    }
    val hz = mason.monitor.GraphLayout.horizontal(xs.values.toList(), back, viewport)
    val w = kotlin.math.ceil(hz.width).toInt()
    val dx = hz.dx.toInt()
    val h = maxOf(e.lanes.size * LANE_H, ((e.nodes.maxOfOrNull { it.y } ?: 0L).toInt()) + NODE_H + 40)
    Box(Modifier.size(w.dp, h.dp)) {
        e.lanes.forEachIndexed { i, l ->
            Box(Modifier.offset(0.dp, (i * LANE_H).dp).size(w.dp, LANE_H.dp).background(if (i % 2 == 0) theme.background else theme.surface))
            Text(l.title, style = theme.body(FontWeight.SemiBold, theme.muted), modifier = Modifier.offset(8.dp, (i * LANE_H + 4).dp).testTag("lane:${l.category}"))
        }
        val byName = e.nodes.associateBy { it.name }
        val muted = theme.muted
        val red = theme.attention
        androidx.compose.foundation.Canvas(Modifier.size(w.dp, h.dp)) {
            e.edges.forEach { edge ->
                val a = byName[edge.from] ?: return@forEach
                val b = byName[edge.to] ?: return@forEach
                val back = edge.kind == "on_red"
                val s = if (back) Offset((a.x + dx).dp.toPx(), (a.y + NODE_H / 2).dp.toPx()) else Offset((a.x + dx + NODE_W / 2).dp.toPx(), (a.y + NODE_H).dp.toPx())
                val t = if (back) Offset((b.x + dx).dp.toPx(), (b.y + NODE_H / 2).dp.toPx()) else Offset((b.x + dx + NODE_W / 2).dp.toPx(), b.y.dp.toPx())
                val path = Path().apply {
                    moveTo(s.x, s.y)
                    if (back) cubicTo(s.x - 70.dp.toPx(), s.y, t.x - 70.dp.toPx(), t.y, t.x, t.y)
                    else cubicTo(s.x, s.y + 40.dp.toPx(), t.x, t.y - 40.dp.toPx(), t.x, t.y)
                }
                drawPath(path, if (back) red else muted, style = Stroke(1.5.dp.toPx(), pathEffect = if (back) PathEffect.dashPathEffect(floatArrayOf(14f, 10f)) else null))
                val arrow = Path().apply {
                    moveTo(t.x, t.y)
                    lineTo(t.x - 5.dp.toPx(), t.y + (if (back) 0f else -9.dp.toPx()))
                    lineTo(t.x + (if (back) -9.dp.toPx() else 5.dp.toPx()), t.y + (if (back) 5.dp.toPx() else -9.dp.toPx()))
                    close()
                }
                drawPath(arrow, if (back) red else muted)
            }
        }
        e.edges.filter { it.kind == "on_red" && it.bound > 0 }.distinctBy { it.from }.forEach { edge ->
            byName[edge.from]?.let { a ->
                Text("red, at most ${edge.bound}×", style = theme.small(color = red), modifier = Modifier.offset((a.x + dx - 100).dp, (a.y + NODE_H / 2 - 18).dp))
            }
        }
        e.nodes.forEach { n -> NodeBox(model, n, e.selected == n.name, dx) }
    }
}

@Composable
private fun NodeBox(model: MonitorModel, n: EditorNode, selected: Boolean, dx: Int) {
    val theme = LocalTheme.current
    var drag by remember(n.name, n.x, n.y) { mutableStateOf(Offset.Zero) }
    val density = LocalDensity.current
    val border = if (selected) theme.accent else if (n.problems.isEmpty()) theme.border else theme.attention
    Column(
        Modifier
            .offset { IntOffset((with(density) { (n.x + dx).dp.toPx() } + drag.x).toInt(), (with(density) { n.y.dp.toPx() } + drag.y).toInt()) }
            .size(NODE_W.dp, NODE_H.dp)
            .background(theme.background, RoundedCornerShape(8.dp))
            .border(if (selected) 3.dp else 1.5.dp, border, RoundedCornerShape(8.dp))
            .clickable { model.selectStep(n.name) }
            .pointerInput(n.name, n.x, n.y) {
                detectDragGestures(
                    onDrag = { c, d -> c.consume(); drag += d },
                    onDragEnd = {
                        val nx = n.x + (drag.x / density.density).toLong()
                        val ny = n.y + (drag.y / density.density).toLong()
                        drag = Offset.Zero
                        model.editorOp(mapOf("op" to "move", "step" to n.name, "x" to nx, "y" to ny))
                    },
                    onDragCancel = { drag = Offset.Zero },
                )
            }
            .padding(8.dp)
            .testTag("node:${n.name}"),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(n.name, style = theme.body(FontWeight.SemiBold), maxLines = 1)
            if (n.is_start) Text("  start", style = theme.small(color = theme.muted))
            Spacer(Modifier.weight(1f))
            if (n.problems.isNotEmpty()) Text("!", style = theme.body(FontWeight.Bold, theme.attention))
        }
        Text(n.kind + if (n.summary.isEmpty()) "" else ": ${n.summary}", style = theme.small(color = theme.muted), maxLines = 1)
        n.problems.take(2).forEach { Text(it, style = theme.small(color = theme.attention), maxLines = 1) }
    }
}

private class PaletteEntry(val type: String, val categories: List<String>, val available: Boolean, val about: String, val fields: JSONObject)

@Composable
private fun Palette(model: MonitorModel, e: EditorView) {
    val theme = LocalTheme.current
    val entries = e.palette.map { s ->
        val d = EditorJson.parse(s)
        PaletteEntry(d.optString("type"), EditorJson.strings(d.optJSONArray("categories")), d.optBoolean("available", true), d.optString("about"), d.optJSONObject("fields") ?: JSONObject())
    }
    val sel = e.nodes.firstOrNull { it.name == e.selected }
    fun fresh(base: String): String {
        var n = 1
        while (e.step_names.contains("$base-$n")) n++
        return "$base-$n"
    }
    fun insert(en: PaletteEntry, extends: Boolean) {
        val s = sel ?: return
        val order = e.lanes.map { it.category }
        val here = order.indexOf(s.category).coerceAtLeast(0)
        val category = if (en.categories.contains(s.category)) s.category else (en.categories.firstOrNull { order.indexOf(it) >= here } ?: en.categories.firstOrNull() ?: s.category)
        val op = mutableMapOf<String, Any?>(
            "op" to "insert_step", "after" to s.name, "name" to fresh(if (extends) s.name else en.type),
            "type" to (if (extends) s.kind else en.type), "category" to (if (extends) s.category else category),
        )
        if (extends) {
            op["extends"] = s.name
        } else {
            val f = mutableMapOf<String, Any?>()
            en.fields.keys().forEach { k -> f[k] = en.fields.get(k) }
            op["fields"] = f
        }
        model.editorOp(op)
    }
    Column(verticalArrangement = Arrangement.spacedBy(theme.spacing)) {
        Text("Add a step", style = theme.title())
        Text(sel?.let { "After `${it.name}`:" } ?: "Select a step; the new one goes after it.", style = theme.body(color = theme.muted))
        entries.filter { it.available }.forEach { en ->
            OutlinedButton(onClick = { insert(en, false) }, enabled = sel != null, modifier = Modifier.testTag("palette:${en.type}")) { Text("+ ${en.type}") }
        }
        OutlinedButton(onClick = { entries.firstOrNull { it.type == sel?.kind }?.let { insert(it, true) } }, enabled = sel != null, modifier = Modifier.testTag("palette:extends")) {
            Text("+ copy of the selected (extends)")
        }
    }
}

@Composable
private fun Choice(selected: String, options: List<String>, none: String? = null, onSelect: (String) -> Unit) {
    var open by remember { mutableStateOf(false) }
    Box {
        OutlinedButton(onClick = { open = true }) { Text(selected.ifEmpty { none ?: "(none)" }) }
        DropdownMenu(open, { open = false }) {
            if (none != null) DropdownMenuItem(text = { Text(none) }, onClick = { open = false; onSelect("") })
            options.forEach { o -> DropdownMenuItem(text = { Text(o) }, onClick = { open = false; onSelect(o) }) }
        }
    }
}

@Composable
private fun CommitField(initial: String, key: Any, modifier: Modifier = Modifier, commit: (String) -> Unit) {
    var text by remember(key, initial) { mutableStateOf(initial) }
    OutlinedTextField(
        text, { text = it }, singleLine = true,
        modifier = modifier.onEnter { commit(text) },
        trailingIcon = { TextButton(onClick = { commit(text) }) { Text("Set") } },
    )
}

private fun Modifier.onEnter(f: () -> Unit): Modifier = this.onKeyEvent { ev ->
    if (ev.key == Key.Enter && ev.type == KeyEventType.KeyUp) { f(); true } else false
}

@Composable
private fun InspectorPanel(model: MonitorModel, e: EditorView) {
    val theme = LocalTheme.current
    val ins = e.inspector ?: return
    var newName by remember(e.selected) { mutableStateOf("") }
    fun target(kind: String) = e.edges.firstOrNull { it.from == e.selected && it.kind == kind }?.to ?: ""
    @Composable
    fun edge(title: String, kind: String) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(theme.spacing), modifier = Modifier.testTag("edge:$kind")) {
            Text(title, style = theme.body())
            Choice(target(kind), e.step_names.filter { it != e.selected || kind == "on_red" }, "(nowhere)") { to ->
                model.editorOp(mapOf("op" to "set_edge", "from" to e.selected, "edge" to kind, "to" to if (to.isEmpty()) null else to))
            }
        }
    }
    Column(verticalArrangement = Arrangement.spacedBy(theme.spacing)) {
        Text("Step ${e.selected} (${ins.kind})", style = theme.title())
        Row(verticalAlignment = Alignment.CenterVertically) {
            CommitField("", key = e.selected, modifier = Modifier.width(200.dp)) { if (it.isNotEmpty()) model.editorOp(mapOf("op" to "rename_step", "from" to e.selected, "to" to it)) }
            Text("rename", style = theme.small(color = theme.muted))
        }
        edge("On success", "next")
        edge("On red", "on_red")
        Row(horizontalArrangement = Arrangement.spacedBy(theme.spacing)) {
            OutlinedButton(onClick = { model.editorOp(mapOf("op" to "set_start", "name" to e.selected)) }, enabled = e.nodes.firstOrNull { it.name == e.selected }?.is_start == false) { Text("Start here") }
            OutlinedButton(onClick = { model.editorOp(mapOf("op" to "remove_step", "name" to e.selected)); model.selectStep("") }, modifier = Modifier.testTag("remove")) { Text("Remove") }
        }
        SchemaForm(model, e.selected, ins.schema, ins.values)
    }
}

/** A form built from a JSON Schema alone. */
@Composable
private fun SchemaForm(model: MonitorModel, step: String, schema: String, values: String) {
    val theme = LocalTheme.current
    val vals = EditorJson.parse(values)
    fun set(key: String, v: Any?) = model.editorOp(
        if (v == null) mapOf("op" to "unset_field", "step" to step, "key" to key) else mapOf("op" to "set_field", "step" to step, "key" to key, "value" to v),
    )
    Column(verticalArrangement = Arrangement.spacedBy(theme.spacing)) {
        EditorJson.fields(schema).forEach { f ->
            Column(Modifier.testTag("field:${f.key}")) {
                Text(f.title, style = theme.body(FontWeight.SemiBold))
                when {
                    f.type == "string" && f.choices.isNotEmpty() -> Choice(vals.optString(f.key, ""), f.choices) { set(f.key, it.ifEmpty { null }) }
                    f.type == "string" -> CommitField(vals.optString(f.key, ""), step + f.key) { set(f.key, it.ifEmpty { null }) }
                    f.type == "integer" -> CommitField(if (vals.has(f.key)) vals.optInt(f.key).toString() else "", step + f.key + vals.optInt(f.key, -1)) { set(f.key, it.trim().toLongOrNull()) }
                    f.type == "boolean" -> Switch(vals.optBoolean(f.key, false), { set(f.key, it) })
                    f.type == "array" && f.choices.isNotEmpty() -> {
                        val cur = EditorJson.strings(vals.optJSONArray(f.key))
                        Row(horizontalArrangement = Arrangement.spacedBy(theme.spacingSmall)) {
                            f.choices.forEach { c ->
                                FilterChip(selected = cur.contains(c), onClick = {
                                    val next = if (cur.contains(c)) cur - c else cur + c
                                    set(f.key, next.ifEmpty { null })
                                }, label = { Text(c) })
                            }
                        }
                    }
                    f.type == "array" -> {
                        val cur = EditorJson.strings(vals.optJSONArray(f.key))
                        CommitField(cur.joinToString(", "), step + f.key + cur.joinToString(",")) { t ->
                            val l = t.split(',').map { it.trim() }.filter { it.isNotEmpty() }
                            set(f.key, l.ifEmpty { null })
                        }
                    }
                    f.type == "object" -> {
                        val o = vals.optJSONObject(f.key) ?: JSONObject()
                        val cur = o.keys().asSequence().sorted().joinToString(", ") { "$it=${o.getInt(it)}" }
                        CommitField(cur, step + f.key + o.length()) { t ->
                            val d = mutableMapOf<String, Any?>()
                            t.split(',').forEach { part ->
                                val kv = part.split('=').map { it.trim() }
                                if (kv.size == 2) kv[1].toLongOrNull()?.let { d[kv[0]] = it }
                            }
                            set(f.key, if (d.isEmpty()) null else d)
                        }
                    }
                }
                if (f.description.isNotEmpty()) Text(f.description, style = theme.small(color = theme.muted))
            }
        }
    }
}

@Composable
private fun TestsTab(model: MonitorModel, e: EditorView) {
    val theme = LocalTheme.current
    val g = e.grid
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(theme.spacing)) {
        Text("Where each suite's level runs. A tick binds the level, so every suite of that level runs there.", style = theme.body(color = theme.muted))
        if (g == null || g.rows.isEmpty()) Text("This pipeline has no suites.", style = theme.body(color = theme.muted))
        g?.let {
            Row(horizontalArrangement = Arrangement.spacedBy(theme.spacingLarge)) {
                Text("Suite", style = theme.body(FontWeight.SemiBold), modifier = Modifier.width(120.dp))
                Text("Level", style = theme.body(FontWeight.SemiBold), modifier = Modifier.width(130.dp))
                it.columns.forEach { c -> Text(c.replace('_', ' '), style = theme.body(FontWeight.SemiBold), modifier = Modifier.width(110.dp)) }
            }
            it.rows.forEach { r ->
                Row(horizontalArrangement = Arrangement.spacedBy(theme.spacingLarge), verticalAlignment = Alignment.CenterVertically, modifier = Modifier.testTag("grid:${r.suite}")) {
                    Text(r.suite, style = theme.body(), modifier = Modifier.width(120.dp))
                    Box(Modifier.width(130.dp)) {
                        Choice(r.level, (it.levels + r.level).distinct().sorted()) { lv -> if (lv.isNotEmpty()) model.editorOp(mapOf("op" to "set_suite_level", "suite" to r.suite, "level" to lv)) }
                    }
                    it.columns.forEachIndexed { i, c ->
                        Box(Modifier.width(110.dp)) {
                            Checkbox(i < r.cells.size && r.cells[i], { on -> model.editorOp(mapOf("op" to "set_binding", "level" to r.level, "when" to c, "on" to on)) }, modifier = Modifier.testTag("cell:${r.suite}:$c"))
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun LandingTab(model: MonitorModel, e: EditorView) {
    val theme = LocalTheme.current
    val f = EditorJson.parse(e.landing)
    val modes = EditorJson.strings(f.optJSONArray("modes"))
    val tiers = f.optJSONArray("tiers") ?: JSONArray()
    val ladder = EditorJson.strings(f.optJSONArray("ladder"))
    val rungs = EditorJson.strings(f.optJSONArray("rungs"))
    var newTier by remember { mutableStateOf("") }
    fun set(key: String, v: Any?) = model.editorOp(mapOf("op" to "landing_set", "key" to key, "value" to v))
    @Composable
    fun row(title: String, initial: String, commit: (String) -> Unit) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(theme.spacing)) {
            Text(title, style = theme.body(), modifier = Modifier.width(340.dp))
            CommitField(initial, title + initial, Modifier.width(240.dp), commit)
        }
    }
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(theme.spacingLarge)) {
        Text("How this project lands", style = theme.title())
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(theme.spacing), modifier = Modifier.testTag("landing:mode")) {
            Text("Mode", style = theme.body(), modifier = Modifier.width(340.dp))
            Choice(f.optString("mode", "direct"), modes) { if (it.isNotEmpty()) set("mode", it) }
        }
        row("Window (collect from the oldest ready task)", "${f.optInt("window_s", 0)}s") { set("window", it) }
        row("Largest batch (0: no cap)", "${f.optInt("max_size", 0)}") { it.toLongOrNull()?.let { n -> set("max_size", n) } }
        row("Batches at once", "${f.optInt("parallel", 1)}") { it.toLongOrNull()?.let { n -> set("parallel", n) } }
        row("Times the base may move before giving up", "${f.optInt("max_fold", 3)}") { it.toLongOrNull()?.let { n -> set("max_fold", n) } }
        row("Conflict resolver command", f.optString("resolver", "").let { if (it == "null") "" else it }) {
            model.editorOp(if (it.isEmpty()) mapOf("op" to "landing_unset", "key" to "resolver") else mapOf("op" to "landing_set", "key" to "resolver", "value" to it))
        }
        Text("Tiers", style = theme.title())
        for (i in 0 until tiers.length()) {
            val t = tiers.getJSONObject(i)
            val name = t.optString("name")
            val have = EditorJson.strings(t.optJSONArray("suites"))
            Column(verticalArrangement = Arrangement.spacedBy(theme.spacingSmall), modifier = Modifier.testTag("tier:$name")) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text(name, style = theme.body(FontWeight.SemiBold))
                    Spacer(Modifier.weight(1f))
                    OutlinedButton(onClick = { model.editorOp(mapOf("op" to "landing_remove_tier", "name" to name)) }) { Text("Remove") }
                }
                Row(horizontalArrangement = Arrangement.spacedBy(theme.spacingSmall)) {
                    e.suites.forEach { s ->
                        FilterChip(selected = have.contains(s), onClick = {
                            model.editorOp(mapOf("op" to "landing_set_tier", "name" to name, "key" to "suites", "value" to if (have.contains(s)) have - s else have + s))
                        }, label = { Text(s) })
                    }
                }
                row("On red (freeze)", t.optString("on_red", "").let { if (it == "null") "" else it }) {
                    model.editorOp(mapOf("op" to "landing_set_tier", "name" to name, "key" to "on_red", "value" to if (it.isEmpty()) null else it))
                }
                row("CI", t.optString("ci", "").let { if (it == "null") "" else it }) {
                    model.editorOp(mapOf("op" to "landing_set_tier", "name" to name, "key" to "ci", "value" to if (it.isEmpty()) null else it))
                }
            }
        }
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(theme.spacing)) {
            OutlinedTextField(newTier, { newTier = it }, label = { Text("New tier (batch, post-land, ...)") }, singleLine = true, modifier = Modifier.width(260.dp))
            OutlinedButton(onClick = { if (newTier.isNotEmpty()) { model.editorOp(mapOf("op" to "landing_add_tier", "name" to newTier, "suites" to emptyList<String>())); newTier = "" } }) { Text("Add tier") }
        }
        Text("When a batch is red, in this order", style = theme.title())
        val all = ladder + rungs.filter { it !in ladder }
        all.forEachIndexed { i, r ->
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(theme.spacing)) {
                Checkbox(ladder.contains(r), { on -> model.editorOp(mapOf("op" to "landing_set_ladder", "rungs" to if (on) ladder + r else ladder - r)) })
                Text(r, style = theme.body(), modifier = Modifier.width(160.dp))
                fun move(d: Int) {
                    val l = ladder.toMutableList()
                    if (i + d in l.indices) { val x = l[i]; l[i] = l[i + d]; l[i + d] = x; model.editorOp(mapOf("op" to "landing_set_ladder", "rungs" to l)) }
                }
                OutlinedButton(onClick = { move(-1) }, enabled = ladder.contains(r) && i > 0) { Text("↑") }
                OutlinedButton(onClick = { move(1) }, enabled = ladder.contains(r) && i < ladder.size - 1) { Text("↓") }
            }
        }
        EditorJson.strings(f.optJSONArray("problems")).forEach { Text(it, style = theme.body(color = theme.attention)) }
    }
}

@Composable
private fun PreviewTab(model: MonitorModel, e: EditorView) {
    val theme = LocalTheme.current
    var scenario by remember { mutableStateOf("") }
    val sims = EditorJson.parse(e.simulation.ifEmpty { "{}" }).optJSONArray("scenarios") ?: JSONArray()
    fun custom() {
        val results = mutableMapOf<String, Any?>()
        scenario.split(';').forEach { part ->
            val kv = part.split(':', limit = 2).map { it.trim() }
            if (kv.size == 2) results[kv[0]] = kv[1].split(',').map { it.trim() }
        }
        model.editorCall("simulate", mapOf("name" to "your scenario", "results" to results))
    }
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(theme.spacingLarge)) {
        Text("What this edit changes", style = theme.title())
        if (e.changes.isEmpty() && e.affected.isEmpty()) Text("Nothing yet.", style = theme.body(color = theme.muted))
        e.changes.forEachIndexed { i, c -> Text("This $c", style = theme.body(), modifier = Modifier.testTag("change:$i")) }
        e.affected.forEach { Text(it, style = theme.body(color = theme.attention)) }
        Text("Simulate a scripted task through the draft", style = theme.title())
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(theme.spacing)) {
            OutlinedButton(onClick = { model.editorCall("simulate") }) { Text("Run the standard scenarios") }
            OutlinedTextField(scenario, { scenario = it }, label = { Text("test: red, red; develop: green") }, singleLine = true, modifier = Modifier.width(320.dp))
            OutlinedButton(onClick = { custom() }) { Text("Run this") }
        }
        for (i in 0 until sims.length()) {
            val s = sims.getJSONObject(i)
            val steps = s.optJSONArray("steps") ?: JSONArray()
            val path = (0 until steps.length()).joinToString(" > ") { j ->
                val st = steps.getJSONObject(j)
                st.optString("step") + (if (st.optString("outcome") == "green") "" else " (${st.optString("outcome")})")
            }
            Column {
                Text(s.optString("name"), style = theme.body(FontWeight.SemiBold))
                Text("$path  =>  ${s.optString("end", s.optString("error"))}", style = theme.mono())
            }
        }
        Text("The change to the files", style = theme.title())
        SelectionContainer { Text(e.diff.ifEmpty { "No change." }, style = theme.mono()) }
    }
}
