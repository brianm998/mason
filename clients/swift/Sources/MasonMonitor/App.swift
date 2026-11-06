// SPDX-License-Identifier: GPL-3.0-or-later

import AppKit
import MonitorCore
import MonitorUI
import SwiftUI

/// The model and its connection to `masond`, kept for the life of the app.
@MainActor
final class Session: ObservableObject {
    let model = MonitorModel(preferences: DefaultsPreferences())
    private var connection: MonitorConnection?
    private var finder: Task<Void, Never>?

    func start() {
        guard connection == nil, finder == nil else { return }
        // The daemon may not be up yet: look for it until it is.
        finder = Task { @MainActor in
            while !Task.isCancelled {
                if let port = Discovery.port(arguments: CommandLine.arguments) {
                    let c = MonitorConnection(model: model, port: port)
                    connection = c
                    c.start()
                    return
                }
                try? await Task.sleep(nanoseconds: 1_000_000_000)
            }
        }
    }
}

@main
struct MasonMonitorApp: App {
    @StateObject private var session = Session()

    init() {
        // A plain executable needs to say it is an app with windows.
        NSApplication.shared.setActivationPolicy(.regular)
    }

    var body: some Scene {
        WindowGroup("MASON") {
            RootView(model: session.model)
                .frame(minWidth: 1000, minHeight: 640)
                .onAppear {
                    session.start()
                    NSApplication.shared.activate(ignoringOtherApps: true)
                }
        }
        .defaultSize(width: 1360, height: 860)
        .commands {
            CommandMenu("View") {
                Button("Bigger Text") { session.model.zoomIn() }.keyboardShortcut("+", modifiers: .command)
                Button("Bigger Text") { session.model.zoomIn() }.keyboardShortcut("=", modifiers: .command)
                Button("Smaller Text") { session.model.zoomOut() }.keyboardShortcut("-", modifiers: .command)
                Button("Actual Size") { session.model.zoomReset() }.keyboardShortcut("0", modifiers: .command)
            }
            CommandMenu("Go") {
                Button("Back") { session.model.goBack() }.keyboardShortcut("[", modifiers: .command)
                Button("Forward") { session.model.goForward() }.keyboardShortcut("]", modifiers: .command)
                Button("Home") { session.model.goHome() }.keyboardShortcut("h", modifiers: [.command, .shift])
            }
        }
    }
}
