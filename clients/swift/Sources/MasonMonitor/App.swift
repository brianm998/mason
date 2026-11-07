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

    /// The update button: build the new monitor from source, launch it, close this one.
    func updateMonitor() {
        switch ClientUpdate.plan(env: ProcessInfo.processInfo.environment) {
        case .unavailable(let why):
            model.updating = why
        case .openPage(let url):
            if let u = URL(string: url) { NSWorkspace.shared.open(u) }
        case .buildFromSource(let dir):
            model.updating = "Building the new monitor…"
            let out = NSTemporaryDirectory() + "mason-monitor-update-\(ProcessInfo.processInfo.processIdentifier)"
            Task.detached {
                let p = Process()
                p.executableURL = URL(fileURLWithPath: "/bin/sh")
                p.arguments = ["-c", "cd '\(dir)' && scripts/bundle-monitor.sh '\(out)'"]
                p.standardOutput = FileHandle.nullDevice
                p.standardError = FileHandle.nullDevice
                try? p.run()
                p.waitUntilExit()
                await MainActor.run {
                    guard p.terminationStatus == 0 else {
                        self.model.updating = "The build of the new monitor failed; this one keeps running."
                        return
                    }
                    self.model.updating = "Starting the new monitor…"
                    NSWorkspace.shared.openApplication(
                        at: URL(fileURLWithPath: out + "/MASON.app"),
                        configuration: NSWorkspace.OpenConfiguration()
                    ) { _, error in
                        DispatchQueue.main.async {
                            if error == nil { NSApp.terminate(nil) }
                            else { self.model.updating = "The new monitor would not start; this one keeps running." }
                        }
                    }
                }
            }
        }
    }

    func start() {
        model.onUpdate = { [weak self] in self?.updateMonitor() }
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
