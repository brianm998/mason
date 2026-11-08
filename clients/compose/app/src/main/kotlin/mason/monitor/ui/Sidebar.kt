// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import mason.monitor.*
import mason.v1.SidebarSection
import mason.v1.TaskRow

private sealed class Entry {
    data object HomeRow : Entry()
    data object Search : Entry()
    data class Header(val section: SidebarSection) : Entry()
    data class Row(val row: TaskRow) : Entry()
    data object Rule : Entry()
    data class PageLink(val title: String, val page: Page) : Entry()
}

/**
 * The sidebar, on every page: a Home row, search, one collapsible section per state (the
 * daemon's order and sorting), then the other pages.
 */
@Composable
fun SidebarPane(model: MonitorModel) {
    val theme = LocalTheme.current
    val entries = buildList {
        add(Entry.HomeRow); add(Entry.Search)
        model.sidebar?.sections?.forEach { sec ->
            add(Entry.Header(sec))
            if (model.isExpanded(sec)) model.filteredRows(sec).forEach { add(Entry.Row(it)) }
        }
        add(Entry.Rule)
        add(Entry.PageLink("Landing", Page.Landing))
        add(Entry.PageLink("LLM usage", Page.Usage))
        add(Entry.PageLink("Pipeline", Page.Pipeline))
        add(Entry.PageLink("Health", Page.Health))
    }
    val state = rememberLazyListState()
    // A tile was clicked: scroll to its section.
    val target = model.scrollTarget
    LaunchedEffect(target, entries.size) {
        if (target != null) {
            val i = entries.indexOfFirst { it is Entry.Header && it.section.state == target }
            // Clear first: a cancelled animation must not leave the target set.
            model.scrollTarget = null
            if (i >= 0) state.animateScrollToItem(i)
        }
    }
    LazyColumn(
        state = state,
        modifier = Modifier.width(theme.sidebarWidth).fillMaxHeight().background(theme.surface.copy(alpha = 0.6f)),
    ) {
        items(entries) { e ->
            when (e) {
                Entry.HomeRow -> NavRow("⌂  Home", "sidebar:home", model.page == Page.Home, bold = true) { model.goHome() }
                Entry.Search -> SearchBox(model)
                is Entry.Header -> SectionHeader(model, e.section)
                is Entry.Row -> RowView(model, e.row)
                Entry.Rule -> HorizontalDivider(Modifier.padding(vertical = theme.spacing), color = theme.border)
                is Entry.PageLink -> NavRow(e.title, "sidebar:${e.title.lowercase().replace(' ', '-')}", model.page == e.page) { model.go(e.page) }
            }
        }
    }
}

@Composable
private fun NavRow(text: String, tag: String, selected: Boolean, bold: Boolean = false, onClick: () -> Unit) {
    val theme = LocalTheme.current
    Text(
        text,
        style = theme.body(if (bold) FontWeight.SemiBold else FontWeight.Normal),
        modifier = Modifier.fillMaxWidth().background(if (selected) theme.accent.copy(alpha = 0.18f) else androidx.compose.ui.graphics.Color.Transparent)
            .clickable(onClick = onClick).padding(horizontal = theme.spacing, vertical = theme.spacingSmall + 2.dp).testTag(tag),
    )
}

@Composable
private fun SearchBox(model: MonitorModel) {
    val theme = LocalTheme.current
    val shape = RoundedCornerShape(theme.radius)
    Column(Modifier.padding(theme.spacing), verticalArrangement = Arrangement.spacedBy(theme.spacingSmall)) {
        androidx.compose.foundation.text.BasicTextField(
            value = model.search.text,
            onValueChange = { model.search = model.search.copy(text = it) },
            textStyle = theme.body(),
            singleLine = true,
            decorationBox = { inner ->
                Box(Modifier.fillMaxWidth().background(theme.background, shape).border(1.dp, theme.border, shape).padding(theme.spacingSmall + 2.dp)) {
                    if (model.search.text.isEmpty()) Text("Search: name, title, body", style = theme.body(color = theme.muted))
                    inner()
                }
            },
            modifier = Modifier.testTag("search"),
        )
        Row(horizontalArrangement = Arrangement.spacedBy(theme.spacing), verticalAlignment = Alignment.CenterVertically) {
            Check("name", model.search.inName) { model.search = model.search.copy(inName = it) }
            Check("title", model.search.inTitle) { model.search = model.search.copy(inTitle = it) }
            Check("body", model.search.inBody) { model.search = model.search.copy(inBody = it) }
        }
    }
}

@Composable
private fun Check(label: String, on: Boolean, change: (Boolean) -> Unit) {
    val theme = LocalTheme.current
    Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.clickable { change(!on) }.testTag("search:$label")) {
        Text(if (on) "☑" else "☐", style = theme.small(color = theme.text))
        Spacer(Modifier.width(theme.spacingSmall))
        Text(label, style = theme.small(color = theme.text))
    }
}

@Composable
private fun SectionHeader(model: MonitorModel, s: SidebarSection) {
    val theme = LocalTheme.current
    val open = model.isExpanded(s)
    val count = model.visibleCount(s)
    Row(
        verticalAlignment = Alignment.CenterVertically,
        modifier = Modifier.fillMaxWidth().clickable { model.toggle(s) }
            .padding(horizontal = theme.spacing, vertical = theme.spacingSmall + 2.dp)
            .testTag("section:${s.state}")
            .semantics { contentDescription = "${s.title}, $count, ${if (open) "expanded" else "collapsed"}" },
    ) {
        Text(if (open) "⌄" else "›", style = theme.body(), modifier = Modifier.width(theme.cell + theme.spacingSmall))
        Text(s.title, style = theme.body(FontWeight.SemiBold), modifier = Modifier.weight(1f))
        Text(
            count.toString(),
            style = theme.body(FontWeight.SemiBold, if (s.state == "needs_attention" && s.count > 0) theme.attention else theme.muted),
        )
    }
}

/**
 * A sidebar row: key and title, project, activity, time in state, progress cells, the model
 * when one is running, and a link for each blocker.
 */
@Composable
private fun RowView(model: MonitorModel, row: TaskRow) {
    val theme = LocalTheme.current
    val selected = model.page == Page.Task(row.id)
    Column(
        verticalArrangement = Arrangement.spacedBy(2.dp),
        modifier = Modifier.fillMaxWidth()
            .background(if (selected) theme.accent.copy(alpha = 0.18f) else androidx.compose.ui.graphics.Color.Transparent)
            .padding(horizontal = theme.spacing, vertical = theme.spacingSmall + 1.dp)
            .testTag("row:${row.state}:${row.id}")
            .semantics { contentDescription = "${row.key} ${row.title}, ${row.project}, ${row.state}" },
    ) {
        Column(Modifier.clickable { model.go(Page.Task(row.id)) }, verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Row(horizontalArrangement = Arrangement.spacedBy(theme.spacingSmall)) {
                Text(row.key, style = theme.body(FontWeight.SemiBold, theme.muted))
                Text(row.title, style = theme.body(), maxLines = 2, overflow = TextOverflow.Ellipsis)
            }
            Text("${row.project}, ${row.why_waiting.ifEmpty { row.activity }}", style = theme.small(), maxLines = 2, overflow = TextOverflow.Ellipsis)
            if (row.proposal.isNotEmpty()) Text(row.proposal, style = theme.small(), modifier = Modifier.testTag("row:${row.id}:proposal"))
            Row(horizontalArrangement = Arrangement.spacedBy(theme.spacing), verticalAlignment = Alignment.CenterVertically) {
                Text(Format.duration(row.in_state_ms), style = theme.small(color = if (row.slow) theme.slow else theme.muted))
                if (row.typical.isNotEmpty()) Text(row.typical, style = theme.small())
                ProgressCells(model, row.progress, row.slow)
                if (row.model.isNotEmpty()) Text(row.model, style = theme.small())
            }
            row.progress.running?.let { ProgressLine(model, it) }
        }
        if (row.blockers.isNotEmpty()) {
            Row(horizontalArrangement = Arrangement.spacedBy(theme.spacingSmall)) {
                Text("blocked by", style = theme.small())
                row.blockers.forEach { b ->
                    Text("#$b", style = theme.small(color = theme.accent), modifier = Modifier.clickable { model.go(Page.Task(b)) })
                }
            }
        }
    }
}
