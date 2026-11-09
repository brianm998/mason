// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.*
import androidx.compose.material3.HorizontalDivider
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.Modifier
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.input.pointer.PointerIcon
import androidx.compose.ui.input.pointer.pointerHoverIcon
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.positionInParent
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.dp
import mason.MasonTokens
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
            BoxWithConstraints(Modifier.fillMaxSize()) {
                val factor = MonitorModel.sidebarFactor(model.textScale)
                val width = MonitorModel.fit(model.panelWidth(model.textScale), maxWidth.value.toDouble(), model.textScale)
                Row(Modifier.fillMaxSize()) {
                    SidebarPane(model, width.dp)
                    SidebarHandle(model, factor)
                    MainPane(model, Modifier.fillMaxSize())
                }
            }
        }
    }
}

/**
 * The draggable divider: a hit area `sizeSidebarHandle` wide drawn as the usual thin line, with
 * a horizontal-resize cursor on hover. The pointer is read in the row's coordinates (the
 * handle's own offset is added), so the panel follows it without drift while it resizes.
 */
@Composable
private fun SidebarHandle(model: MonitorModel, factor: Double) {
    val theme = LocalTheme.current
    var left by remember { mutableStateOf(0f) }
    val density = LocalDensity.current
    val handle = MasonTokens.sizeSidebarHandle.dp
    Box(
        Modifier.fillMaxHeight().width(handle)
            .onGloballyPositioned { left = it.positionInParent().x }
            .pointerHoverIcon(PointerIcon(java.awt.Cursor(java.awt.Cursor.E_RESIZE_CURSOR)))
            .pointerInput(factor) {
                detectDragGestures { change, _ ->
                    change.consume()
                    // The line sits at the middle of the handle.
                    val x = (left + change.position.x) / density.density - MasonTokens.sizeSidebarHandle / 2.0
                    model.setSidebarWidth(x / factor)
                }
            },
        contentAlignment = Alignment.Center,
    ) {
        Box(Modifier.fillMaxHeight().width(androidx.compose.ui.unit.Dp.Hairline).background(theme.border))
    }
}
