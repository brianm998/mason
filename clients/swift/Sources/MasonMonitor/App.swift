// SPDX-License-Identifier: GPL-3.0-or-later

import AppKit
import MasonMascot
import MonitorCore
import MonitorUI
import SwiftUI

/// The model and its connection to `masond`, kept for the life of the app.
@MainActor
final class Session: ObservableObject {
    let model = MonitorModel(preferences: DefaultsPreferences())
    private var connection: MonitorConnection?
    private var finder: Task<Void, Never>?

    private var pulse: Timer?

    init() {
        // Sounds and sentences come from the daemon's announcements, spoken by this sink when this
        // monitor is the one that speaks. The plain chime stays for a daemon that sends none.
        model.announcer.sink = SystemSink()
        // The commit stamped on this build by scripts/bundle-monitor.sh.
        if model.ownVersion == "dev", let build = Bundle.main.object(forInfoDictionaryKey: "MasonBuild") as? String, !build.isEmpty {
            model.ownVersion = build
        }
        model.onAttention = {
            if let s = NSSound(named: NSSound.Name("Glass")) { s.play() } else { NSSound.beep() }
        }
        model.copier = { copyToPasteboard($0) }
        model.launcher = { Self.openTerminal(running: $0) }
        pulse = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak model] _ in
            Task { @MainActor in model?.pulse() }
        }
    }

    /// A Terminal window running `argv`: a temporary .command file, which Terminal opens. False
    /// when the file could not be written or opened. The caller copies the command then.
    static func openTerminal(running argv: [String]) -> Bool {
        let line = argv.map { "'" + $0.replacingOccurrences(of: "'", with: "'\\''") + "'" }.joined(separator: " ")
        let dir = FileManager.default.temporaryDirectory
        let file = dir.appendingPathComponent("mason-open-\(UUID().uuidString).command")
        let script = "#!/bin/bash\nrm -f \"$0\"\n\(line)\n"
        do {
            try script.write(to: file, atomically: true, encoding: .utf8)
            try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: file.path)
        } catch { return false }
        let terminal = URL(fileURLWithPath: "/System/Applications/Utilities/Terminal.app")
        let ok = NSWorkspace.shared.open([file], withApplicationAt: terminal, configuration: NSWorkspace.OpenConfiguration())
        return ok
    }

    /// The daemon has built a new monitor at `path`: put it where this one is, so
    /// that the Dock and `mason://` links get it too, start it, and close this one.
    func install(_ path: String) {
        let built = URL(fileURLWithPath: path)
        let own = Bundle.main.bundleURL
        Task.detached {
            let target = (try? Self.replace(own, with: built)) ?? built
            await MainActor.run {
                let launch = NSWorkspace.OpenConfiguration()
                // This one is still running from the same place: without this, the system just brings it forward.
                launch.createsNewApplicationInstance = true
                NSWorkspace.shared.openApplication(at: target, configuration: launch) { _, error in
                    DispatchQueue.main.async {
                        if error == nil { NSApp.terminate(nil) }
                        else { self.model.monitorUpdateFailed("The new monitor would not start; this one keeps running.") }
                    }
                }
            }
        }
    }

    /// Swap the build into this app's place and return it. Throws, leaving the app as it
    /// was, if there is no such place: not run from a bundle, already in the daemon's
    /// builds, or somewhere that cannot be written.
    nonisolated static func replace(_ own: URL, with built: URL) throws -> URL {
        func home(_ u: URL) -> URL { u.deletingLastPathComponent().deletingLastPathComponent().standardizedFileURL }
        guard own.pathExtension == "app", home(own) != home(built) else { throw CocoaError(.fileWriteNoPermission) }
        let fm = FileManager.default
        let copy = own.deletingLastPathComponent().appendingPathComponent(".MASON-update-\(getpid()).app")
        try? fm.removeItem(at: copy)
        try fm.copyItem(at: built, to: copy)
        do {
            return try fm.replaceItemAt(own, withItemAt: copy) ?? own
        } catch {
            try? fm.removeItem(at: copy)
            throw error
        }
    }

    func start() {
        model.onInstall = { [weak self] in self?.install($0) }
        guard connection == nil, finder == nil else { return }
        // The daemon may not be up yet: look for it until it is.
        finder = Task { @MainActor in
            while !Task.isCancelled {
                if Discovery.info(arguments: CommandLine.arguments) != nil {
                    let c = MonitorConnection(model: model, discover: { Discovery.info(arguments: CommandLine.arguments) })
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
        // `MasonMonitor --mascot-check`: load every mascot image and exit (no window).
        if CommandLine.arguments.contains("--mascot-check") { exit(Mascot.selfCheck() ? 0 : 1) }
        // A plain executable needs to say it is an app with windows.
        NSApplication.shared.setActivationPolicy(.regular)
        // Unbundled there is no Info.plist icon; MASON.app has one and keeps it.
        if Bundle.main.object(forInfoDictionaryKey: "CFBundleIconFile") == nil,
           let d = Mascot.macAppIconData(), let img = NSImage(data: d) {
            NSApplication.shared.applicationIconImage = img
        }
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
