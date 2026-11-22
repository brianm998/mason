// SPDX-License-Identifier: GPL-3.0-or-later

#if canImport(AppKit)
import AppKit
#else
import UIKit
#endif
import MasonTokens
import MasonMascot
import MonitorCore
import SwiftUI

/// The window (Part 17): the top bar, the banners (only while true), then the
/// sidebar and the main view, which show exactly one thing at a time.
public struct RootView: View {
    let model: MonitorModel
    @Environment(\.colorScheme) private var scheme
    @State private var windowHeight: CGFloat = 800
    #if !canImport(AppKit)
    @Environment(\.scenePhase) private var scenePhase
    #endif

    let features: PlatformFeatures

    /// `features` is what the platform offers: the Mac's everything, or the iPad's subset.
    public init(model: MonitorModel, features: PlatformFeatures = .mac) {
        self.model = model
        self.features = features
    }

    public var body: some View {
        let theme = Theme(dark: scheme == .dark, scale: model.textScale)
        Group {
            if model.header == nil {
                StartView(model: model)
            } else {
                mainLayout
            }
        }
        .environment(\.theme, theme)
        .environment(\.features, features)
        .foregroundStyle(theme.text)
        .background(theme.background)
        .onOpenURL { model.open(url: $0) }
        .onAppear { SharedTick.shared.model = model }
        #if !canImport(AppKit)
        .onChange(of: scenePhase, initial: true) { _, phase in SharedTick.shared.setVisible(phase == .active) }
        #endif
    }

    private var mainLayout: some View {
        VStack(spacing: 0) {
            TopBar(model: model)
            Divider()
            BannersView(model: model, windowHeight: windowHeight)
            GeometryReader { geo in
                let factor = MonitorModel.sidebarFactor(scale: model.textScale)
                let width = MonitorModel.fit(
                    desired: model.panelWidth(scale: model.textScale), available: geo.size.width, scale: model.textScale)
                HStack(spacing: 0) {
                    SidebarPane(model: model, width: width)
                    SidebarHandle(model: model, factor: factor, width: width).zIndex(1)
                    MainPane(model: model)
                }
                .coordinateSpace(name: SidebarHandle.space)
            }
        }
        .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { windowHeight = $0 }
    }
}

/// What the start view says, from the connection alone (never an estimate of progress).
func startStatus(connected: Bool, reconnecting: Bool, link: String? = nil) -> String {
    if let link { return link }
    if connected { return "Connecting…" }
    return reconnecting ? "Daemon restarting, reconnecting…" : "Looking for the daemon…"
}

/// Shown instead of the window until the first header arrives. The image is decorative and
/// whole (the robot is left, the word right: a crop would lose both); there is no progress bar.
struct StartView: View {
    @Environment(\.theme) var theme
    let model: MonitorModel

    var body: some View {
        VStack(spacing: CGFloat(MasonTokens.sizeSpacingLarge) * CGFloat(theme.scale)) {
            if let img = Mascot.brand("start_image") {
                img.resizable().scaledToFit()
                    .frame(maxWidth: CGFloat(MasonTokens.sizeMascotStartWidth) * CGFloat(theme.scale))
                    .accessibilityHidden(true)
            }
            Text(startStatus(connected: model.connected, reconnecting: model.reconnecting, link: model.linkStatus))
                .accessibilityIdentifier("start-status")
        }
        .padding(CGFloat(MasonTokens.sizeSpacingLarge) * CGFloat(theme.scale))
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

/// The draggable divider: a hit area `sizeSidebarHandle` wide drawn as the usual thin line,
/// with a horizontal-resize cursor on hover. The pointer position is read in the row's own
/// coordinate space, so the panel follows it without drift while the panel itself resizes.
struct SidebarHandle: View {
    static let space = "sidebar-row"
    @Environment(\.theme) var theme
    let model: MonitorModel
    let factor: Double
    let width: CGFloat
    @State private var inside = false

    var body: some View {
        Color.clear
            .frame(width: CGFloat(MasonTokens.sizeSidebarHandle))
            .overlay(Divider())
            .contentShape(Rectangle())
            #if !canImport(AppKit)
            // Touch: a 44 pt strip centred on the line (an overlay, so the layout width is unchanged).
            .overlay(
                Color.clear
                    .frame(width: CGFloat(PlatformFeatures.minTouchTarget))
                    .contentShape(Rectangle())
                    .gesture(drag)
            )
            #endif
            #if canImport(AppKit)
            .onHover { over in
                if over && !inside { NSCursor.resizeLeftRight.push() }
                if !over && inside { NSCursor.pop() }
                inside = over
            }
            .onDisappear { if inside { NSCursor.pop(); inside = false } }
            #endif
            .gesture(drag)
    }

    private var drag: some Gesture {
        DragGesture(minimumDistance: 0, coordinateSpace: .named(Self.space))
            .onChanged { v in
                // The handle is `handle` wide: the line sits at its middle.
                let x = Double(v.location.x) - Double(MasonTokens.sizeSidebarHandle) / 2
                model.setSidebarWidth(x / factor)
            }
    }
}
