// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.LinkAnnotation
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextLinkStyles
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.style.TextDecoration
import mason.monitor.LinkSegment
import mason.monitor.Linkify
import mason.monitor.MonitorModel
import mason.monitor.Page
import mason.v1.CiLink

/** What a click on a link does: open a task's page in the monitor, or a URL in the browser. */
interface LinkActions {
    fun openTask(id: Long)
    fun openUrl(url: String)
}

/** The links of an [AnnotatedString] as (text, task or url) pairs, in order: what a test reads. */
fun linkTargets(a: AnnotatedString): List<Pair<String, String>> =
    a.getLinkAnnotations(0, a.length).map { r ->
        val item = r.item
        val target = when (item) {
            is LinkAnnotation.Clickable -> item.tag
            is LinkAnnotation.Url -> item.url
            else -> ""
        }
        a.text.substring(r.start, r.end) to target
    }

private const val TASK_TAG = "task:"

/** [base] (already styled, or plain) with a link on the part each task or URL segment covers. */
fun linkedString(base: AnnotatedString, segments: List<LinkSegment>, linkColor: Color, actions: LinkActions): AnnotatedString {
    val style = TextLinkStyles(SpanStyle(color = linkColor, textDecoration = TextDecoration.Underline))
    return buildAnnotatedString {
        append(base)
        var pos = 0
        for (s in segments) {
            val end = pos + s.text.length
            when (s) {
                is LinkSegment.Plain -> {}
                is LinkSegment.Task -> addLink(
                    LinkAnnotation.Clickable(TASK_TAG + s.id, style) { actions.openTask(s.id) }, pos, end,
                )
                is LinkSegment.Url -> addLink(
                    LinkAnnotation.Clickable(s.url, style) { actions.openUrl(s.url) }, pos, end,
                )
            }
            pos = end
        }
    }
}

@Composable
private fun rememberActions(model: MonitorModel): LinkActions {
    val uri = LocalUriHandler.current
    return object : LinkActions {
        override fun openTask(id: Long) = model.go(Page.Task(id))
        override fun openUrl(url: String) {
            if (Linkify.isWebUrl(url)) runCatching { uri.openUri(url) }
        }
    }
}

/**
 * Text whose task numbers and CI references are links ([Linkify]): a task opens its page in the
 * monitor (so back and forward work), a CI reference opens in the default browser. Selectable
 * when [selectable]. [base] is styled text (the plan's Markdown) that links are added on top of.
 */
@Composable
fun LinkedText(
    model: MonitorModel, text: String, style: TextStyle, modifier: Modifier = Modifier,
    selectable: Boolean = false, base: AnnotatedString? = null,
) {
    val theme = LocalTheme.current
    val actions = rememberActions(model)
    val plain = base ?: AnnotatedString(text)
    val shown = linkedString(plain, model.linkify(plain.text), theme.accent, actions)
    if (selectable) SelectionContainer { Text(shown, style = style, modifier = modifier) }
    else Text(shown, style = style, modifier = modifier)
}

/** The "CI" line and the tracker issue of a task's page: each reference a link. */
@Composable
fun CiLinksLine(links: List<CiLink>, issue: Long, issueUrl: String) {
    val theme = LocalTheme.current
    val uri = LocalUriHandler.current
    val shown = links.filter { Linkify.isWebUrl(it.url) }
    Row(
        horizontalArrangement = Arrangement.spacedBy(theme.spacingSmall), verticalAlignment = Alignment.CenterVertically,
        modifier = Modifier.testTag("detail:ci"),
    ) {
        if (issue != 0L) {
            if (Linkify.isWebUrl(issueUrl)) {
                Text("issue #$issue", style = theme.body(color = theme.accent),
                    modifier = Modifier.testTag("detail:issue").clickable { runCatching { uri.openUri(issueUrl) } })
            } else {
                Text("issue #$issue", style = theme.body(color = theme.muted))
            }
        }
        if (shown.isNotEmpty()) {
            Text("CI", style = theme.body(color = theme.muted))
            shown.forEach { l ->
                Text(l.label.ifEmpty { l.url }, style = theme.body(color = theme.accent),
                    modifier = Modifier.testTag("ci:${l.label}").clickable { runCatching { uri.openUri(l.url) } })
            }
        }
    }
}
