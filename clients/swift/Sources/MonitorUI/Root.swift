// SPDX-License-Identifier: GPL-3.0-or-later

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
            HStack(spacing: 0) {
                SidebarPane(model: model)
                Divider()
                MainPane(model: model)
            }
        }
        .environment(\.theme, theme)
        .foregroundStyle(theme.text)
        .background(theme.background)
        .onOpenURL { model.open(url: $0) }
    }
}
