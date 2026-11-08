// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import androidx.compose.ui.ExperimentalComposeUiApi
import androidx.compose.ui.ImageComposeScene
import androidx.compose.ui.unit.Density
import mason.monitor.ui.RootView
import mason.v1.Answer
import mason.v1.Command
import mason.v1.CommandResult
import org.jetbrains.skia.EncodedImageFormat
import java.io.File
import kotlin.system.exitProcess

/**
 * `monitor-tool`: what the exit tests use, the same commands as the SwiftUI client's tool.
 *
 *     conformance --port P --out DIR     dump what the client shows at every demo checkpoint
 *     snapshot --port P --out DIR        render screenshots from `masond --demo-manual`
 *     answer --port P --task N [--option 1 | --text WORDS] [--hash H]   answer a task's open question as the card does
 *     approve --port P --task N [--diff HASH]   what the Approve button sends (the hold's diff hash, as shown)
 *     reject --port P --task N [--reason WORDS]   what the Reject button sends
 *     dump --port P --out DIR            write what the home page shows now (a real daemon), as items
 *     cancel --port P --task N           cancel a task as the Cancel button does
 *     skip --port P --task N [--step S]  skip a test step as the Skip button does
 */
private fun fail(m: String): Nothing {
    System.err.println("monitor-tool: $m")
    exitProcess(2)
}

private fun List<String>.opt(name: String) = indexOf(name).takeIf { it >= 0 }?.let { getOrNull(it + 1) }

private fun until(what: String, seconds: Int = 15, cond: () -> Boolean) {
    val end = System.currentTimeMillis() + seconds * 1000
    while (!cond()) {
        if (System.currentTimeMillis() > end) fail("timed out waiting for $what")
        Thread.sleep(50)
    }
}

/** Let the views settle: no new sequence number for a moment. */
private fun settle(model: MonitorModel) {
    var last = model.lastSeq
    var quiet = 0
    while (quiet < 6) {
        Thread.sleep(50)
        if (model.lastSeq == last) quiet++ else { quiet = 0; last = model.lastSeq }
    }
}

/** Send a command and wait for its result. */
private fun call(model: MonitorModel, c: Command, id: String): CommandResult {
    var result: CommandResult? = null
    model.onResult = { r -> if (r.id == id) result = r }
    model.send(c.copy(id = id))
    until("the result of $id") { result != null }
    return result!!
}

private fun step(model: MonitorModel): Boolean {
    val r = call(model, Command(raw = mason.v1.Raw(method = "POST", path = "/demo/next")), "step")
    val advanced = Codec.json(r.json)?.get("advanced") == true
    settle(model)
    return advanced
}

fun main(argv: Array<String>) {
    val args = argv.toList()
    if (args.isEmpty()) fail("usage: conformance|snapshot|answer|cancel|skip --port P ...")
    val cmd = args.first()
    val port = Discovery.port(args) ?: fail("--port P (or --data-dir D)")
    val model = MonitorModel()
    MonitorConnection(model, { Discovery.info(args) ?: DaemonInfo(port) }).start()
    until("a connection") { model.connected && model.header != null && model.sidebar != null && model.home != null }
    when (cmd) {
        "conformance" -> conformance(model, File(args.opt("--out") ?: fail("--out DIR")))
        "editor" -> editor(model, args, File(args.opt("--out") ?: fail("--out DIR")))
        "snapshot" -> snapshots(model, File(args.opt("--out") ?: fail("--out DIR")))
        "answer" -> answer(model, args)
        "approve", "reject" -> approval(model, cmd, args)
        "dump" -> {
            val out = File(args.opt("--out") ?: fail("--out DIR"))
            out.mkdirs()
            settle(model)
            val items = model.visibleItems()
            File(out, "checkpoint-0.json").writeText(items.toJsonText())
            println("the monitor shows ${items.size} items")
        }
        "usage" -> {
            // The LLM usage page, as items (compared with `conformance usage-expected`).
            val out = File(args.opt("--out") ?: fail("--out DIR"))
            out.mkdirs()
            model.go(Page.Usage)
            settle(model)
            val items = model.visibleItems()
            File(out, "checkpoint-0.json").writeText(items.toJsonText())
            println("the usage page shows ${items.size} items")
        }
        "cancel" -> {
            val t = args.opt("--task")?.toLongOrNull() ?: fail("--task N")
            val r = call(model, Command(cancel = mason.v1.TaskRef(task = t), actor = model.identity), "cancel")
            if (!r.ok) fail("the daemon refused: ${r.error}")
            println("cancelled task #$t")
        }
        "skip" -> {
            val t = args.opt("--task")?.toLongOrNull() ?: fail("--task N")
            val s = args.opt("--step") ?: "test"
            val r = call(model, Command(skip = mason.v1.Skip(task = t, step = s), actor = model.identity), "skip")
            if (!r.ok) fail("the daemon refused: ${r.error}")
            println("skipped step $s of task #$t")
        }
        else -> fail("unknown command $cmd")
    }
    exitProcess(0)
}

private fun answer(model: MonitorModel, args: List<String>) {
    val t = args.opt("--task")?.toLongOrNull() ?: fail("--task N")
    // An option number, or text for the Other box (sent as free text, as the CLI does).
    val text = args.opt("--text")
    val option = text ?: args.opt("--option") ?: "1"
    until("task #$t's decision card") { model.home?.attention?.any { it.task?.id == t && it.card != null } == true }
    val card = model.home!!.attention.first { it.task?.id == t }.card!!
    println("card: ${card.ask}")
    if (!model.canAct) { println("not the owner: the card shows no buttons"); return }
    println("options: " + card.options.joinToString(" | ") { it.label + if (it.recommended) " (recommended)" else "" })
    // --hash sends a card hash the owner was not looking at, to see a stale card refused.
    val r = call(model, Command(answer = Answer(task = t, answer = option, card_hash = args.opt("--hash") ?: card.hash), actor = model.identity), "answer")
    if (!r.ok) fail("the daemon refused the answer (${r.status}): ${r.error}")
    // What the daemon recorded as the answer ("Other: ..." for free text).
    (Codec.json(r.json)?.get("answer") as? String)?.let { println("the daemon recorded: $it") }
    until("the task to leave needs-you") { model.home?.attention?.none { it.task?.id == t } == true }
    println("answered task #$t with ${if (text != null) "text \"$option\"" else "option $option"}; it is no longer waiting for the owner")
}

/**
 * What the Approve and Reject buttons do: find the task's approval in the Home view, print what it
 * shows, and send Approve (with the diff hash shown, or `--diff` to try another) or Reject.
 */
private fun approval(model: MonitorModel, cmd: String, args: List<String>) {
    val t = args.opt("--task")?.toLongOrNull() ?: fail("--task N")
    until("task #$t's approval") { model.home?.attention?.any { it.task?.id == t && it.approval != null } == true }
    val a = model.home!!.attention.first { it.task?.id == t }.approval!!
    println("waiting for: ${a.waiting_for}")
    a.lines.forEach { println("  - $it") }
    if (a.diff_hash.isNotEmpty()) println("diff: ${a.diff_hash}")
    if (a.diff_command.isNotEmpty()) println("show it: ${a.diff_command}")
    if (!model.canAct) { println("not the owner: the view shows no buttons"); return }
    val c = if (cmd == "approve") {
        val d = args.opt("--diff")
        when {
            d != null -> Command(approve_hold = mason.v1.ApproveHold(task = t, diff_hash = d), actor = model.identity)
            a.diff_hash.isEmpty() -> Command(approve = mason.v1.TaskRef(task = t), actor = model.identity)
            else -> Command(approve_hold = mason.v1.ApproveHold(task = t, diff_hash = a.diff_hash), actor = model.identity)
        }
    } else {
        Command(reject = mason.v1.Reject(task = t, reason = args.opt("--reason") ?: ""), actor = model.identity)
    }
    val r = call(model, c, cmd)
    if (!r.ok) fail("the daemon refused (${r.status}): ${r.error}")
    until("the task to leave needs-you") { model.home?.attention?.none { it.task?.id == t && it.approval != null } == true }
    println("${if (cmd == "approve") "approved" else "rejected"} task #$t; it no longer waits for the owner")
}

/**
 * What the pipeline editor shows for a project's draft, as items: the pipeline page,
 * opened on a project with a step selected, and its `visibleItems()` written out.
 * (Needs a real daemon: the demo has no repositories.)
 */
private fun editor(model: MonitorModel, args: List<String>, out: File) {
    val project = args.opt("--project") ?: fail("--project NAME")
    val step = args.opt("--step") ?: ""
    out.mkdirs()
    model.project = project
    model.selectStep(step)
    model.go(Page.Pipeline)
    until("the editor for $project") { model.editor?.project == project && model.editor?.selected == step }
    settle(model)
    val items = model.visibleItems()
    File(out, "checkpoint-0.json").writeText(items.toJsonText())
    println("the editor shows ${items.size} items")
}

private fun conformance(model: MonitorModel, out: File) {
    out.mkdirs()
    var k = 0
    while (true) {
        settle(model)
        val items = model.visibleItems()
        File(out, "checkpoint-$k.json").writeText(items.toJsonText())
        println("checkpoint $k: ${items.size} visible items")
        if (!step(model)) break
        k++
    }
}

private fun snapshots(model: MonitorModel, out: File) {
    out.mkdirs()
    // Checkpoint 4: a question is open, tasks are in several stages.
    repeat(4) { step(model) }
    model.setExpanded("needs_attention", true)
    shot(model, out, "home", 1440, 900, dark = false)
    shot(model, out, "home-dark", 1440, 900, dark = true)
    // The rest of the script: failed and landed tasks exist; open every section.
    repeat(3) { step(model) }
    model.sidebar?.sections?.forEach { model.setExpanded(it.state, true) }
    shot(model, out, "sidebar-expanded", 1440, 1100, dark = false)
    model.go(Page.Task(1))
    until("task #1") { model.taskDetail?.row?.id == 1L }
    settle(model)
    shot(model, out, "task-detail", 1440, 900, dark = false)
    model.go(Page.Task(2))
    until("task #2") { model.taskDetail?.row?.id == 2L }
    settle(model)
    shot(model, out, "decision-card", 1440, 900, dark = false)
    repeat(3) { model.zoomIn() }
    model.goHome()
    settle(model)
    shot(model, out, "home-larger-text", 1440, 900, dark = false)
}

/** Render the real views to a PNG, off screen. */
@OptIn(ExperimentalComposeUiApi::class)
private fun shot(model: MonitorModel, out: File, name: String, w: Int, h: Int, dark: Boolean) {
    val scene = ImageComposeScene(width = w * 2, height = h * 2, density = Density(2f)) { RootView(model, dark) }
    try {
        // A few frames, so layout and effects settle.
        var t = 0L
        repeat(6) { scene.render(t); t += 50_000_000L }
        val png = scene.render(t).encodeToData(EncodedImageFormat.PNG) ?: fail("cannot encode $name")
        File(out, "$name.png").writeBytes(png.bytes)
        println("wrote $name.png")
    } finally {
        scene.close()
    }
}
