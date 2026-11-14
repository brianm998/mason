// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
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
fun CpuSection(model: MonitorModel) {
    val theme = LocalTheme.current
    val layout = cpuLayout(model.cpu, model.daemonNow(), model.cpuColors) ?: return
    var expanded by remember { mutableStateOf(false) }
    var tip by remember { mutableStateOf<String?>(null) }
    Section("CPU") {
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
        val legend: @Composable (CpuRow) -> Unit = { r ->
            Row(
                Modifier.fillMaxWidth().testTag("cpu-legend:${r.label}")
                    .then(if (r.task > 0) Modifier.clickable { model.go(Page.Task(r.task)) } else Modifier),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(theme.spacing),
            ) {
                Box(Modifier.size(theme.cell).background(theme.cpu(r.color), RoundedCornerShape(theme.radius / 2)))
                Text(r.name, style = theme.body(FontWeight.SemiBold, if (r.task > 0) theme.accent else theme.text))
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
