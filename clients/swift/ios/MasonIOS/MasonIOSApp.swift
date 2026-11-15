// SPDX-License-Identifier: GPL-3.0-or-later

import MonitorCore
import MonitorPhone
import MonitorUI
import SwiftUI

/// The iOS app, one universal build for iPhone and iPad. The Xcode project is generated from
/// `ios/project.yml` by XcodeGen (`make ios`); it is not committed.
@main
struct MasonIOSApp: App {
    @StateObject private var model = MonitorModel(preferences: DefaultsPreferences())

    var body: some Scene {
        WindowGroup { AppRoot(model: model) }
    }
}

/// Picks the layout by horizontal size class: regular is the iPad root (the shared
/// `RootView` until the iPad layout exists), compact is the phone's.
struct AppRoot: View {
    @ObservedObject var model: MonitorModel
    @Environment(\.horizontalSizeClass) private var sizeClass

    var body: some View {
        switch RootChoice.choose(regularWidth: sizeClass.map { $0 == .regular }) {
        case .pad: RootView(model: model)
        case .phone: PhoneRootView(model: model)
        }
    }
}
