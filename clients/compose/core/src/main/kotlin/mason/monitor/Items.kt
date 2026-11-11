// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import mason.v1.Approval
import mason.v1.CellState
import mason.v1.DecisionCard
import mason.v1.TaskRow
import org.json.JSONArray

/**
 * One thing on screen, as the conformance runner compares it: an id, the texts a
 * person can read in it, the actions it offers, its place among its siblings.
 * (The same shape `conformance` builds from the server's views, and the SwiftUI
 * client builds from what it draws.)
 */
data class Item(
    val id: String,
    val texts: List<String>,
    val actions: List<String> = emptyList(),
    val order: Int = 0,
) {
    fun toJson(): Map<String, Any> = mapOf("id" to id, "texts" to texts, "actions" to actions.sorted(), "order" to order)
}

fun TaskRow.item(prefix: String, order: Int): Item {
    val texts = mutableListOf(key, title, project, state)
    if (why_waiting.isNotEmpty()) texts += why_waiting
    if (display.isNotEmpty()) texts += display
    // A task the tracker imported shows its issue, and where to read it.
    if (issue != 0L) {
        texts += "issue #$issue"
        if (issue_url.isNotEmpty()) texts += issue_url
    }
    // The task page says what it waits for and what waits for it.
    if (prefix == "detail") {
        if (blockers.isNotEmpty()) texts += "waiting for " + blockers.joinToString(", ") { "#$it" }
        if (blocks.isNotEmpty()) texts += "blocks " + blocks.joinToString(", ") { "#$it" }
    }
    return Item("$prefix:$id", texts, allowed_actions, order)
}

/**
 * What a held task's page shows for its blockers: each with its state and where it came from,
 * and the actions on it (the owner's alone); and the Add blocker field.
 */
fun TaskRow.blockerItems(owner: Boolean): List<Item> {
    val out = mutableListOf<Item>()
    blocker_links.forEachIndexed { i, l ->
        out += Item("blocker:$id:${l.task}", listOf("#${l.task}", l.state, l.origin), if (owner) l.actions else emptyList(), i)
    }
    if ("block" in allowed_actions) {
        out += Item("add-blocker:$id", listOf("Add blocker"), if (owner) listOf("add_blocker") else emptyList())
    }
    return out
}

/** One item per running step with a live record: what its progress says, with no clock text. */
fun TaskRow.progressItems(now: Long): List<Item> =
    progress.filter { it.state == CellState.PART && it.run != 0L }
        .map { Item("progress:$id:${it.step}", it.progressTexts(now)) }

fun DecisionCard.item(task: Long, owner: Boolean = true): Item {
    val texts = mutableListOf(ask, why)
    options.forEachIndexed { i, o -> texts += "${i + 1}. ${o.label}${if (o.recommended) " (recommended)" else ""}" }
    texts += evidence
    val actions = if (owner) options.indices.map { "option:${it + 1}" } + "other" else emptyList()
    return Item("card:$task", texts, actions)
}

/** What an approval shows: what the task waits on in full; Approve and Reject for the owner only. */
fun Approval.item(task: Long, owner: Boolean = true): Item {
    val texts = mutableListOf(waiting_for)
    texts += lines
    if (diff_hash.isNotEmpty()) texts += "diff $diff_hash"
    if (branch.isNotEmpty()) texts += branch
    if (diff_command.isNotEmpty()) texts += diff_command
    return Item("approval:$task", texts, if (owner) listOf("approve", "reject") else emptyList())
}

/**
 * Everything this client is showing, as items: what the conformance run reads.
 * Built from the same state the views draw, so the two cannot disagree; every view
 * also carries the item's id as its test tag, so a harness reading the real
 * accessibility tree finds the same ids.
 */
fun MonitorModel.visibleItems(): List<Item> {
    val out = mutableListOf<Item>()
    header?.let { h ->
        out += Item("status", listOf(if (connected) h.status else "Not connected"))
        h.counts.forEachIndexed { i, c -> out += Item("tile:${c.state}", listOf(tileCount(c), tileLabel(c)), order = i) }
        h.banners.forEachIndexed { i, b -> out += Item("banner:$i", listOf(b.text), order = i) }
        // Issues the tracker has not imported: a count tile and a list, only while true.
        if (h.pending_issues.isNotEmpty()) {
            out += Item("tile:not_imported", listOf(h.pending_issues.size.toString(), "not imported"), order = h.counts.size)
            h.pending_issues.forEachIndexed { i, p ->
                val texts = mutableListOf(p.project, "#${p.number}", p.title, p.reason)
                if (p.url.isNotEmpty()) texts += p.url
                out += Item("pending:${p.project}:${p.number}", texts, order = i)
            }
        }
    }
    sidebar?.let { s ->
        s.sections.forEachIndexed { i, sec ->
            val open = isExpanded(sec)
            out += Item(
                "section:${sec.state}",
                listOf(sec.title, sec.count.toString(), if (open) "expanded" else "collapsed"),
                order = i,
            )
            if (open) filteredRows(sec).forEachIndexed { j, r ->
                out += r.item("row:${sec.state}", j)
                out += r.progressItems(daemonNow())
            }
        }
    }
    if (page == Page.Home) home?.let { h ->
        h.attention.forEach { a ->
            val t = a.task
            val c = a.card
            if (t != null && c != null) out += c.item(t.id, canAct)
        }
        h.attention.forEach { a ->
            val t = a.task
            val x = a.approval
            if (t != null && x != null) out += x.item(t.id, canAct)
        }
        h.active.forEachIndexed { i, a ->
            out += Item("active:${a.task}", listOf(a.display.ifEmpty { a.model }, a.project, "#${a.task}", a.step), order = i)
        }
        out += Item("sessions", listOf("${h.sessions_used} of ${h.sessions_limit}"), listOf("increase", "decrease"))
        h.compilation?.let { c ->
            out += Item("compile", listOf("${c.used} of ${c.limit}", c.cpuLine), listOf("increase", "decrease"))
            val now = daemonNow()
            c.running.forEachIndexed { i, r ->
                out += Item("compile:running:$i", listOf(r.label, Format.duration(maxOf(0L, now - r.since_ms))), order = i)
            }
            c.pending.forEachIndexed { i, w ->
                out += Item("compile:pending:${w.position}", listOf(w.label, w.reason), order = i)
            }
        }
    }
    if (page == Page.Usage) usage?.let { out += it.items() }
    if (page == Page.Pipeline) editor?.let { out += it.items() }
    if (page is Page.Task) taskDetail?.let { d ->
        val r = d.row
        if (r != null) {
            out += r.item("detail", 0)
            out += r.progressItems(daemonNow())
            out += r.blockerItems(canAct)
            d.steps.forEachIndexed { i, s -> out += Item("step:${s.run}", listOf(s.name, s.outcome) + s.launches.map { it.display }, order = i) }
            d.card?.let { out += it.item(r.id, canAct) }
            d.approval?.let { out += it.item(r.id, canAct) }
        }
    }
    return out
}

/** The items as JSON text, for the conformance dump. */
fun List<Item>.toJsonText(): String {
    val a = JSONArray()
    forEach { a.put(org.json.JSONObject(it.toJson())) }
    return a.toString(2)
}

/**
 * What the pipeline editor shows, as items (the same list the daemon's
 * `conformance::editor_items` builds from the same view).
 */
fun mason.v1.EditorView.items(): List<Item> {
    val out = mutableListOf<Item>()
    lanes.forEachIndexed { i, l -> out += Item("lane:${l.category}", listOf(l.title), order = i) }
    nodes.forEachIndexed { i, n ->
        val texts = mutableListOf(n.name, n.kind, n.category)
        if (n.summary.isNotEmpty()) texts += n.summary
        if (n.is_start) texts += "start"
        texts += n.problems
        out += Item("node:${n.name}", texts, order = i)
    }
    edges.forEachIndexed { i, x ->
        val texts = mutableListOf(x.from, x.to, if (x.kind == "on_red") "on red" else "next")
        if (x.bound > 0) texts += "at most ${x.bound} times"
        out += Item("edge:${x.from}:${x.kind}:${x.to}", texts, order = i)
    }
    grid?.let { g ->
        g.rows.forEachIndexed { i, r ->
            val texts = mutableListOf(r.suite, r.level)
            g.columns.zip(r.cells).filter { it.second }.forEach { texts += it.first.replace('_', ' ') }
            out += Item("grid:${r.suite}", texts, order = i)
        }
    }
    problems.forEachIndexed { i, p -> out += Item("problem:$i", listOf(p), order = i) }
    val actions = mutableListOf("simulate")
    if (can_undo) actions += "undo"
    if (can_redo) actions += "redo"
    if (dirty) actions += listOf("discard", "apply")
    out += Item("draft", listOf(if (dirty) "changed" else "unchanged"), actions)
    changes.forEachIndexed { i, c -> out += Item("change:$i", listOf(c), order = i) }
    return out
}

/**
 * What the LLM usage page shows that does not move with the clock (the same list the daemon's
 * `conformance::usage_items` builds from the same view).
 */
fun mason.v1.UsageView.items(): List<Item> {
    val out = mutableListOf<Item>()
    providers.forEachIndexed { i, p ->
        val head: List<String> = listOf(p.name, p.adapter_name, p.billing, "${p.sessions_used} of ${p.sessions_limit} sessions")
        out += Item("usage:${p.name}", head, order = i)
        p.windows.forEachIndexed { j, w ->
            val pct = when {
                !w.reported -> "not reported yet"
                w.has_percent -> "${Math.round(w.percent)}%"
                else -> "no percentage reported"
            }
            val texts = mutableListOf(w.label, pct)
            if (w.reported) texts += w.state
            out += Item("usage-window:${p.name}:${w.id}", texts, order = j)
        }
    }
    return out
}
