// SPDX-License-Identifier: GPL-3.0-or-later

import MasonTokens
import MonitorCore
import SwiftUI

extension Color {
    /// `#RRGGBB`.
    init(hex: String) {
        var v: UInt64 = 0
        Scanner(string: hex.trimmingCharacters(in: CharacterSet(charactersIn: "#"))).scanHexInt64(&v)
        self.init(
            red: Double((v >> 16) & 0xFF) / 255,
            green: Double((v >> 8) & 0xFF) / 255,
            blue: Double(v & 0xFF) / 255
        )
    }
}

/// Colours and sizes from the one token file (`clients/tokens.toml`), light or
/// dark, and text sized by the user's scale. Text reflows with the scale: it
/// is real text at a larger size, never a zoomed picture.
public struct Theme {
    public var dark: Bool
    public var scale: Double

    public init(dark: Bool, scale: Double) {
        self.dark = dark
        self.scale = scale
    }

    private func c(_ light: String, _ dark: String) -> Color { Color(hex: self.dark ? dark : light) }

    public var background: Color { c(MasonTokens.colorLightBackground, MasonTokens.colorDarkBackground) }
    public var surface: Color { c(MasonTokens.colorLightSurface, MasonTokens.colorDarkSurface) }
    public var text: Color { c(MasonTokens.colorLightText, MasonTokens.colorDarkText) }
    public var muted: Color { c(MasonTokens.colorLightTextMuted, MasonTokens.colorDarkTextMuted) }
    public var accent: Color { c(MasonTokens.colorLightAccent, MasonTokens.colorDarkAccent) }
    public var attention: Color { c(MasonTokens.colorLightAttention, MasonTokens.colorDarkAttention) }
    public var attentionTint: Color { c(MasonTokens.colorLightAttentionTint, MasonTokens.colorDarkAttentionTint) }
    public var slow: Color { c(MasonTokens.colorLightSlow, MasonTokens.colorDarkSlow) }
    public var ok: Color { c(MasonTokens.colorLightOk, MasonTokens.colorDarkOk) }
    public var border: Color { c(MasonTokens.colorLightBorder, MasonTokens.colorDarkBorder) }

    /// A CPU share's colour by `CpuRow.color`: "1"..."10" for tasks, or "masond", "monitor", "sccache", "other".
    public func cpu(_ key: String) -> Color {
        switch key {
        case "1": return c(MasonTokens.colorLightCpu1, MasonTokens.colorDarkCpu1)
        case "2": return c(MasonTokens.colorLightCpu2, MasonTokens.colorDarkCpu2)
        case "3": return c(MasonTokens.colorLightCpu3, MasonTokens.colorDarkCpu3)
        case "4": return c(MasonTokens.colorLightCpu4, MasonTokens.colorDarkCpu4)
        case "5": return c(MasonTokens.colorLightCpu5, MasonTokens.colorDarkCpu5)
        case "6": return c(MasonTokens.colorLightCpu6, MasonTokens.colorDarkCpu6)
        case "7": return c(MasonTokens.colorLightCpu7, MasonTokens.colorDarkCpu7)
        case "8": return c(MasonTokens.colorLightCpu8, MasonTokens.colorDarkCpu8)
        case "9": return c(MasonTokens.colorLightCpu9, MasonTokens.colorDarkCpu9)
        case "10": return c(MasonTokens.colorLightCpu10, MasonTokens.colorDarkCpu10)
        case "masond": return c(MasonTokens.colorLightCpuMasond, MasonTokens.colorDarkCpuMasond)
        case "monitor": return c(MasonTokens.colorLightCpuMonitor, MasonTokens.colorDarkCpuMonitor)
        case "sccache": return c(MasonTokens.colorLightCpuSccache, MasonTokens.colorDarkCpuSccache)
        default: return c(MasonTokens.colorLightCpuOther, MasonTokens.colorDarkCpuOther)
        }
    }
    public var cpuIdle: Color { c(MasonTokens.colorLightCpuIdle, MasonTokens.colorDarkCpuIdle) }

    private func s(_ n: Int) -> CGFloat { CGFloat(Double(n) * scale) }
    public var tileMinWidth: CGFloat { s(MasonTokens.sizeTileMinWidth) }
    public var tileHeight: CGFloat { s(MasonTokens.sizeTileHeight) }
    public var spacing: CGFloat { s(MasonTokens.sizeSpacing) }
    public var spacingSmall: CGFloat { s(MasonTokens.sizeSpacingSmall) }
    public var spacingLarge: CGFloat { s(MasonTokens.sizeSpacingLarge) }
    public var radius: CGFloat { CGFloat(MasonTokens.sizeCornerRadius) }
    public var cell: CGFloat { s(MasonTokens.sizeProgressCell) }
    public var mascotBannerIcon: CGFloat { s(MasonTokens.sizeMascotBannerIcon) }
    public var illustration: CGFloat { s(MasonTokens.sizeMascotIllustration) }
    public var mascotIconSmall: CGFloat { s(MasonTokens.sizeMascotIconSmall) }
    public var mascotIcon: CGFloat { s(MasonTokens.sizeMascotIcon) }

    public func body(_ weight: Font.Weight = .regular) -> Font { .system(size: s(MasonTokens.sizeTextBody), weight: weight) }
    public func small(_ weight: Font.Weight = .regular) -> Font { .system(size: s(MasonTokens.sizeTextSmall), weight: weight) }
    public func title(_ weight: Font.Weight = .semibold) -> Font { .system(size: s(MasonTokens.sizeTextTitle), weight: weight) }
    public func count() -> Font { .system(size: s(MasonTokens.sizeTextCount), weight: .semibold, design: .rounded) }
    public func mono() -> Font { .system(size: s(MasonTokens.sizeTextSmall), design: .monospaced) }
}

private struct ThemeKey: EnvironmentKey {
    static let defaultValue = Theme(dark: false, scale: 1)
}

extension EnvironmentValues {
    var theme: Theme {
        get { self[ThemeKey.self] }
        set { self[ThemeKey.self] = newValue }
    }
}
