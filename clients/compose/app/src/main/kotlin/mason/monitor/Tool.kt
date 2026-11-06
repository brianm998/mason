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
 *     answer --port P --task N [--option 1]   answer a task's open question as the card does
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
    MonitorConnection(model, port).start()
    until("a connection") { model.connected && model.header != null && model.sidebar != null && model.home != null }
    when (cmd) {
        "conformance" -> conformance(model, File(args.opt("--out") ?: fail("--out DIR")))
        "snapshot" -> snapshots(model, File(args.opt("--out") ?: fail("--out DIR")))
        "answer" -> answer(model, args)
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
    val option = args.opt("--option") ?: "1"
    until("task #$t's decision card") { model.home?.attention?.any { it.task?.id == t && it.card != null } == true }
    val card = model.home!!.attention.first { it.task?.id == t }.card!!
    println("card: ${card.ask}")
    println("options: " + card.options.joinToString(" | ") { it.label + if (it.recommended) " (recommended)" else "" })
    val r = call(model, Command(answer = Answer(task = t, answer = option, card_hash = card.hash), actor = model.identity), "answer")
    if (!r.ok) fail("the daemon refused the answer: ${r.error}")
    until("the task to leave needs-you") { model.home?.attention?.none { it.task?.id == t } == true }
    println("answered task #$t with option $option; it is no longer waiting for the owner")
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
