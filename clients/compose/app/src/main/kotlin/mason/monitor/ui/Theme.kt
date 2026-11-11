// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor.ui

import androidx.compose.runtime.Immutable
import androidx.compose.runtime.compositionLocalOf
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import mason.MasonTokens as T

fun hex(s: String): Color {
    val v = s.trim('#').toLong(16)
    return Color(
        red = ((v shr 16) and 0xFF) / 255f,
        green = ((v shr 8) and 0xFF) / 255f,
        blue = (v and 0xFF) / 255f,
    )
}

/**
 * Colours and sizes from the one token file (`clients/tokens.toml`), light or dark,
 * and text sized by the user's scale. Text reflows with the scale: it is real text
 * at a larger size, never a zoomed picture.
 */
@Immutable
class Theme(val dark: Boolean, val scale: Double) {
    private fun c(light: String, dark: String) = hex(if (this.dark) dark else light)

    val background get() = c(T.colorLightBackground, T.colorDarkBackground)
    val surface get() = c(T.colorLightSurface, T.colorDarkSurface)
    val text get() = c(T.colorLightText, T.colorDarkText)
    val muted get() = c(T.colorLightTextMuted, T.colorDarkTextMuted)
    val accent get() = c(T.colorLightAccent, T.colorDarkAccent)
    val attention get() = c(T.colorLightAttention, T.colorDarkAttention)
    val attentionTint get() = c(T.colorLightAttentionTint, T.colorDarkAttentionTint)
    val slow get() = c(T.colorLightSlow, T.colorDarkSlow)
    val ok get() = c(T.colorLightOk, T.colorDarkOk)
    val border get() = c(T.colorLightBorder, T.colorDarkBorder)

    private fun s(n: Int): Dp = (n * scale).dp
    val tileMinWidth get() = s(T.sizeTileMinWidth)
    val tileHeight get() = s(T.sizeTileHeight)
    val spacing get() = s(T.sizeSpacing)
    val spacingSmall get() = s(T.sizeSpacingSmall)
    val spacingLarge get() = s(T.sizeSpacingLarge)
    val radius get() = T.sizeCornerRadius.dp
    val cell get() = s(T.sizeProgressCell)
    val illustration get() = s(T.sizeMascotIllustration)

    private fun style(size: Int, weight: FontWeight, color: Color, family: FontFamily? = null) =
        TextStyle(fontSize = (size * scale).sp, fontWeight = weight, color = color, fontFamily = family)

    fun body(w: FontWeight = FontWeight.Normal, color: Color = text) = style(T.sizeTextBody, w, color)
    fun small(w: FontWeight = FontWeight.Normal, color: Color = muted) = style(T.sizeTextSmall, w, color)
    fun title(w: FontWeight = FontWeight.SemiBold, color: Color = text) = style(T.sizeTextTitle, w, color)
    fun count(color: Color = text) = style(T.sizeTextCount, FontWeight.SemiBold, color)
    fun mono(color: Color = muted) = style(T.sizeTextSmall, FontWeight.Normal, color, FontFamily.Monospace)
}

val LocalTheme = compositionLocalOf { Theme(dark = false, scale = 1.0) }
