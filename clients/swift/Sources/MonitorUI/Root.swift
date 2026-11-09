// SPDX-License-Identifier: GPL-3.0-or-later

import AppKit
import MasonTokens
import MonitorCore
import SwiftUI

/// The window (Part 17): the top bar, the banners (only while true), then the
/// sidebar and the main view, which show exactly one thing at a time.
public struct RootView: View {
    @ObservedObject var model: MonitorModel
    @Environment(\.colorScheme) private var scheme

    public init(model: MonitorModel) { self.model = model }

    public var body: some View {
        let theme = Theme(dark: scheme == .dark, scale: model.textScale)
        VStack(spacing: 0) {
            TopBar(model: model)
            Divider()
            BannersView(model: model)
            GeometryReader { geo in
                let factor = MonitorModel.sidebarFactor(scale: model.textScale)
                let width = MonitorModel.fit(
                    desired: model.panelWidth(scale: model.textScale), available: geo.size.width, scale: model.textScale)
                HStack(spacing: 0) {
                    SidebarPane(model: model, width: width)
                    SidebarHandle(model: model, factor: factor, width: width)
                    MainPane(model: model)
                }
                .coordinateSpace(name: SidebarHandle.space)
            }
        }
        .environment(\.theme, theme)
        .foregroundStyle(theme.text)
        .background(theme.background)
        .onOpenURL { model.open(url: $0) }
    }
}

/// The draggable divider: a hit area `sizeSidebarHandle` wide drawn as the usual thin line,
/// with a horizontal-resize cursor on hover. The pointer position is read in the row's own
/// coordinate space, so the panel follows it without drift while the panel itself resizes.
struct SidebarHandle: View {
    static let space = "sidebar-row"
    @Environment(\.theme) var theme
    @ObservedObject var model: MonitorModel
    let factor: Double
    let width: CGFloat
    @State private var inside = false

    var body: some View {
        Color.clear
            .frame(width: CGFloat(MasonTokens.sizeSidebarHandle))
            .overlay(Divider())
            .contentShape(Rectangle())
            .onHover { over in
                if over && !inside { NSCursor.resizeLeftRight.push() }
                if !over && inside { NSCursor.pop() }
                inside = over
            }
            .onDisappear { if inside { NSCursor.pop(); inside = false } }
            .gesture(
                DragGesture(minimumDistance: 0, coordinateSpace: .named(Self.space))
                    .onChanged { v in
                        // The handle is `handle` wide: the line sits at its middle.
                        let x = Double(v.location.x) - Double(MasonTokens.sizeSidebarHandle) / 2
                        model.setSidebarWidth(x / factor)
                    }
            )
    }
}
