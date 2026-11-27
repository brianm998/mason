// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor.ui

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.TooltipArea
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Checkbox
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import mason.monitor.*

/**
 * Home's disk section: the newest free space of each shown volume, a span picker, a line chart of
 * free space per volume (y from 0 to the largest total shown) and a legend with a checkbox per
 * volume when there is more than one. Hidden hides the section on a daemon that sends no disk history.
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
fun DiskSection(model: MonitorModel, open: (() -> Unit)? = null) {
    if (!model.diskSeen) return
    val theme = LocalTheme.current
    val history = model.diskHistory
    val hidden = model.hiddenVolumes
    val colors = model.diskColors
    var chart by remember { mutableStateOf<DiskChart?>(null) }
    LaunchedEffect(history, hidden, colors) {
        chart = if (history == null) null else withContext(Dispatchers.Default) { diskChart(history, hidden, colors) }
    }
    var tip by remember { mutableStateOf<String?>(null) }
    Section("Disk (all projects)", open = open) {
        val volumes = history?.volumes.orEmpty()
        val shown = volumes.filter { it.id !in hidden }
        val header = shown.mapNotNull { v -> history?.let { h -> diskLatest(h, v.id) }?.let { "${v.name} ${Format.freeOf(it.first, it.second)}" } }
        if (header.isNotEmpty()) Text(header.joinToString(" · "), style = theme.body(color = theme.muted), modifier = Modifier.testTag("disk-header"))
        Row(horizontalArrangement = Arrangement.spacedBy(theme.spacing), modifier = Modifier.testTag("disk-span")) {
            DiskSpan.entries.forEach { sp ->
                Text(
                    sp.label,
                    style = theme.body(if (sp == model.diskSpan) FontWeight.Bold else FontWeight.Normal,
                        if (sp == model.diskSpan) theme.text else theme.accent),
                    modifier = Modifier.clickable { model.pickDiskSpan(sp) }.testTag("disk-span:${sp.label}"),
                )
            }
        }
        val c = chart?.takeIf { it.times.size > 1 && it.spanMs == model.diskSpan.ms }
        if (volumes.isNotEmpty() && shown.isEmpty()) {
            Text("No volume selected.", style = theme.small(color = theme.muted), modifier = Modifier.testTag("disk-none"))
        } else if (c == null) {
            Text("Collecting disk history…", style = theme.small(color = theme.muted), modifier = Modifier.testTag("disk-collecting"))
        } else {
            Canvas(
                Modifier.fillMaxWidth().height(theme.cell * 5).background(theme.cpuIdle, RoundedCornerShape(theme.radius))
                    .testTag("disk-chart")
                    .pointerInput(c) {
                        detectTapGestures { p ->
                            val i = diskChartIndex(c, (p.x / size.width).toDouble()) ?: return@detectTapGestures
                            val level = (1 - p.y / size.height) * c.maxTotal
                            val hit = c.series.filter { it.free[i] != null }.minByOrNull { kotlin.math.abs(it.free[i]!! - level) }
                            val time = java.text.SimpleDateFormat("HH:mm:ss").format(java.util.Date(c.times[i]))
                            tip = hit?.let { "$time ${it.tip(i)}" } ?: time
                        }
                    },
            ) {
                val n = c.times.size
                val t0 = c.times.first().toDouble()
                val dt = maxOf(1.0, c.times.last() - t0)
                fun x(i: Int) = size.width * ((c.times[i] - t0) / dt).toFloat()
                for (s in c.series) {
                    val path = Path()
                    var pen = false
                    for (i in 0 until n) {
                        val f = s.free[i]
                        if (f == null) { pen = false; continue }
                        val y = size.height * (1 - f.toFloat() / c.maxTotal.toFloat())
                        if (pen) path.lineTo(x(i), y) else path.moveTo(x(i), y)
                        pen = true
                    }
                    drawPath(path, theme.cpu(s.color), style = Stroke(width = 2f * theme.scale.toFloat()))
                }
            }
            tip?.let { Text(it, style = theme.small(color = theme.text), modifier = Modifier.testTag("disk-tip")) }
        }
        volumes.forEach { v ->
            val latest = history?.let { diskLatest(it, v.id) }
            val color = theme.cpu(((colors[v.id] ?: 0) + 1).toString())
            Row(
                Modifier.fillMaxWidth().testTag("disk-legend:${v.name}"),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(theme.spacing),
            ) {
                if (volumes.size > 1) {
                    Checkbox(
                        checked = v.id !in hidden,
                        onCheckedChange = { model.toggleVolume(v.id) },
                        modifier = Modifier.testTag("disk-check:${v.name}"),
                    )
                }
                Box(Modifier.size(theme.cell).background(color, RoundedCornerShape(theme.radius / 2)))
                TooltipArea(
                    tooltip = {
                        Text(diskVolumeTip(v.mount, v.roles), style = theme.small(color = theme.text),
                            modifier = Modifier.background(theme.surface).padding(theme.spacing))
                    },
                    modifier = Modifier.weight(1f),
                ) {
                    Text(v.name, style = theme.body(FontWeight.SemiBold), maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
                Text(latest?.let { Format.freeOf(it.first, it.second) } ?: "not read", style = theme.body(color = theme.muted), maxLines = 1)
            }
        }
        history?.let(::diskWorktreeLines).orEmpty().forEachIndexed { i, line ->
            Text(line, style = theme.small(color = if (i == 0) theme.text else theme.muted),
                modifier = Modifier.testTag(if (i == 0) "disk-worktrees" else "disk-worktrees:$i"))
        }
    }
}
