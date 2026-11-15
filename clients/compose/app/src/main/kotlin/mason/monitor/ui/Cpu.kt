// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.input.pointer.pointerInput
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import mason.monitor.*

/**
 * Home's CPU section: one bar standing for the whole machine, a segment per share from
 * the left in the daemon's order, and a legend in the same order. It reads only the CPU
 * state (`model.cpu`, `model.cpuColors`), so a new reading redraws this and nothing else.
 * A segment shows its tip when tapped.
 */
@Composable
fun CpuSection(model: MonitorModel, open: (() -> Unit)? = null) {
    val theme = LocalTheme.current
    val layout = cpuLayout(model.cpu, model.daemonNow(), model.cpuColors) ?: return
    var expanded by remember { mutableStateOf(false) }
    var tip by remember { mutableStateOf<String?>(null) }
    Section("CPU (all projects)", open = open) {
        Text(layout.header, style = theme.body(color = theme.muted))
        val hidden = layout.hidden
        if (hidden != null) {
            Text(hidden, style = theme.body(color = theme.muted), modifier = Modifier.testTag("cpu-hidden"))
            return@Section
        }
        Row(
            Modifier.fillMaxWidth().height(theme.cell + theme.spacing).background(theme.cpuIdle, RoundedCornerShape(theme.radius))
                .testTag("cpu-bar"),
        ) {
            layout.segments.forEach { s ->
                Box(
                    Modifier.weight(s.permille.toFloat()).fillMaxHeight().background(theme.cpu(s.color))
                        .clickable { tip = if (tip == s.tip) null else s.tip }
                        .semantics { contentDescription = s.tip }
                        .testTag("cpu-segment:${s.label}"),
                )
            }
            val rest = 1000 - layout.segments.sumOf { it.permille }
            if (rest > 0) Spacer(Modifier.weight(rest.toFloat()))
        }
        tip?.let { Text(it, style = theme.small(color = theme.text)) }
        CpuHistoryGraph(model)
        val legend: @Composable (CpuRow) -> Unit = { r ->
            Row(
                Modifier.fillMaxWidth().testTag("cpu-legend:${r.label}")
                    .then(if (r.task > 0) Modifier.clickable { model.go(Page.Task(r.task)) } else Modifier),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(theme.spacing),
            ) {
                Box(Modifier.size(theme.cell).background(theme.cpu(r.color), RoundedCornerShape(theme.radius / 2)))
                Text(r.heading, style = theme.body(FontWeight.SemiBold, if (r.task > 0) theme.accent else theme.text))
                Text(r.title, style = theme.body(color = theme.muted), maxLines = 1, overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f))
                Text(Format.permille(r.permille), style = theme.body())
            }
        }
        layout.rows.forEach { legend(it) }
        if (layout.more.isNotEmpty()) {
            Text(
                (if (expanded) "Hide: " else "") + layout.moreText,
                style = theme.body(color = theme.accent),
                modifier = Modifier.clickable { expanded = !expanded }.testTag("cpu-more"),
            )
            if (expanded) layout.more.forEach { legend(it) }
        }
    }
}

/**
 * The CPU history as a stacked area chart under the bar, one series per share in the bar's colours,
 * with a span picker. The points are prepared on a background dispatcher; a newer history arriving
 * first cancels the build, so updates coalesce. Tapping shows the time and the share's value there.
 */
@Composable
private fun CpuHistoryGraph(model: MonitorModel) {
    val theme = LocalTheme.current
    val history = model.cpuHistory
    val usage = model.cpu
    val colors = model.cpuColors
    var chart by remember { mutableStateOf<CpuChart?>(null) }
    LaunchedEffect(history, usage, colors) {
        chart = if (history == null) null else withContext(Dispatchers.Default) { cpuChart(history, usage, colors) }
    }
    var tip by remember { mutableStateOf<String?>(null) }
    Row(horizontalArrangement = Arrangement.spacedBy(theme.spacing), modifier = Modifier.testTag("cpu-span")) {
        CpuSpan.entries.forEach { sp ->
            Text(
                sp.label,
                style = theme.body(if (sp == model.cpuSpan) FontWeight.Bold else FontWeight.Normal,
                    if (sp == model.cpuSpan) theme.text else theme.accent),
                modifier = Modifier.clickable { model.pickCpuSpan(sp) }.testTag("cpu-span:${sp.label}"),
            )
        }
    }
    val c = chart?.takeIf { it.times.size > 1 && it.spanMs == model.cpuSpan.ms }
    if (c == null) {
        Text("Collecting CPU history…", style = theme.small(color = theme.muted))
        return
    }
    val paths = remember(c) { c }
    Canvas(
        Modifier.fillMaxWidth().height(theme.cell * 5).background(theme.cpuIdle, RoundedCornerShape(theme.radius))
            .testTag("cpu-chart")
            .pointerInput(c) {
                detectTapGestures { p ->
                    val i = cpuChartIndex(c, (p.x / size.width).toDouble()) ?: return@detectTapGestures
                    val level = (1 - p.y / size.height) * maxOf(1000, c.maxStack)
                    var acc = 0f
                    var hit = c.series.lastOrNull()
                    for (s in c.series) {
                        acc += s.values[i]
                        if (level <= acc) { hit = s; break }
                    }
                    val time = java.text.SimpleDateFormat("HH:mm:ss").format(java.util.Date(c.times[i]))
                    tip = hit?.let { "$time ${it.tip(i)}" } ?: time
                }
            },
    ) {
        val n = paths.times.size
        val t0 = paths.times.first().toDouble()
        val dt = maxOf(1.0, paths.times.last() - t0)
        val top = maxOf(1000, paths.maxStack).toFloat()
        val base = FloatArray(n)
        fun x(i: Int) = size.width * ((paths.times[i] - t0) / dt).toFloat()
        for (s in paths.series) {
            val path = Path()
            for (i in 0 until n) {
                val y = size.height * (1 - (base[i] + s.values[i]) / top)
                if (i == 0) path.moveTo(x(i), y) else path.lineTo(x(i), y)
            }
            for (i in n - 1 downTo 0) {
                path.lineTo(x(i), size.height * (1 - base[i] / top))
                base[i] += s.values[i]
            }
            path.close()
            drawPath(path, theme.cpu(s.color))
        }
    }
    tip?.let { Text(it, style = theme.small(color = theme.text)) }
}
