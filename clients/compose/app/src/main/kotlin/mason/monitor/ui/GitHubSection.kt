// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import mason.monitor.GitHubRuns
import mason.monitor.Linkify
import mason.monitor.MonitorModel
import mason.monitor.Page
import mason.v1.RemoteRuns

/** Home's GitHub section (#767): every run MASON has on GitHub; the page adds every shard's job. */
@Composable
internal fun GitHubSection(model: MonitorModel, g: RemoteRuns, open: (() -> Unit)? = null, detail: Boolean = false) {
    val theme = LocalTheme.current
    val uri = LocalUriHandler.current
    Section("GitHub", open = open) {
        Text(GitHubRuns.header(g), style = theme.body(color = theme.muted), modifier = Modifier.testTag("github:header"))
        if (g.runs.isEmpty()) Text(GitHubRuns.EMPTY, style = theme.body(color = theme.muted))
        g.runs.forEach { r ->
            Column(Modifier.testTag("github:${r.id}"), verticalArrangement = Arrangement.spacedBy(theme.spacingSmall)) {
                Row(horizontalArrangement = Arrangement.spacedBy(theme.spacing)) {
                    if (r.task != 0L) Link(GitHubRuns.task(r)) { model.go(Page.Task(r.task)) }
                    else Text(GitHubRuns.task(r), style = theme.body(FontWeight.SemiBold))
                    Text(r.task_state, style = theme.small(color = theme.muted))
                    Text(GitHubRuns.kind(r), style = theme.body())
                    Text(GitHubRuns.state(r), style = theme.body(FontWeight.SemiBold))
                    if (Linkify.isWebUrl(r.url)) Link("open on GitHub") { runCatching { uri.openUri(r.url) } }
                }
                Text(
                    listOf(r.progress, "updated ${GitHubRuns.duration(r.updated_ago_s)} ago").filter { it.isNotEmpty() }.joinToString(" · "),
                    style = theme.small(color = theme.muted),
                )
                if (detail) r.jobs.forEach { j ->
                    Text(GitHubRuns.job(j), style = theme.small(color = theme.muted), modifier = Modifier.padding(start = theme.spacingLarge))
                }
            }
        }
    }
}
