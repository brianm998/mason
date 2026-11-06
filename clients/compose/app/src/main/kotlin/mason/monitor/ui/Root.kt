// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.*
import androidx.compose.material3.HorizontalDivider
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.Modifier
import mason.monitor.MonitorModel

/**
 * The window (Part 17): the top bar, the banners (only while true), then the sidebar and
 * the main view, which show exactly one thing at a time.
 */
@Composable
fun RootView(model: MonitorModel, dark: Boolean = isSystemInDarkTheme()) {
    val theme = Theme(dark, model.textScale)
    CompositionLocalProvider(LocalTheme provides theme) {
        Column(Modifier.fillMaxSize().background(theme.background)) {
            TopBar(model)
            HorizontalDivider(color = theme.border)
            BannersView(model)
            Row(Modifier.fillMaxSize()) {
                SidebarPane(model)
                VerticalRule()
                MainPane(model, Modifier.fillMaxSize())
            }
        }
    }
}

@Composable
private fun VerticalRule() {
    val theme = LocalTheme.current
    Box(Modifier.fillMaxHeight().width(androidx.compose.ui.unit.Dp.Hairline).background(theme.border))
}
