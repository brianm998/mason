// SPDX-License-Identifier: GPL-3.0-or-later

import MonitorCore
import MonitorPhone
import SwiftUI

/// The iPhone app. It is built from this source tree by an Xcode project that has not been
/// made here (see docs/stages/s24.md): add the `MonitorCore` and `MonitorPhone` products of
/// `clients/swift`, and use `Info.plist` beside this file.
@main
struct MasonIOSApp: App {
    @StateObject private var model = MonitorModel(preferences: DefaultsPreferences())
    var body: some Scene {
        WindowGroup { PhoneRootView(model: model) }
    }
}
