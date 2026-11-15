// SPDX-License-Identifier: GPL-3.0-or-later

#if canImport(AppKit)
import AppKit
import MonitorCore
import SwiftUI

/// Offscreen pictures of the phone's screens at the two portrait sizes (no simulator; the
/// window is never ordered front, so nothing appears on the Mac's screen).
@MainActor
public enum PhoneSnapshot {
    /// iPhone 15 Pro and iPhone SE (3rd generation), in points.
    public static let sizes: [CGSize] = [CGSize(width: 393, height: 852), CGSize(width: 375, height: 667)]

    public static func sizeName(_ size: CGSize) -> String { "\(Int(size.width))x\(Int(size.height))" }

    /// The phone root laid out at exactly `size`, with `pushed` on the stack over the sidebar.
    public static func bitmap(_ model: MonitorModel, pushed: [Page], size: CGSize, dark: Bool) -> NSBitmapImageRep? {
        _ = NSApplication.shared
        let host = NSHostingView(rootView: PhoneRootView(model: model, pushed: pushed))
        host.frame = CGRect(origin: .zero, size: size)
        let window = NSWindow(contentRect: host.frame, styleMask: [.titled], backing: .buffered, defer: false)
        window.appearance = NSAppearance(named: dark ? .darkAqua : .aqua)
        window.contentView = host
        host.layoutSubtreeIfNeeded()
        RunLoop.main.run(until: Date().addingTimeInterval(0.3))
        host.layoutSubtreeIfNeeded()
        guard let rep = host.bitmapImageRepForCachingDisplay(in: host.bounds) else { return nil }
        host.cacheDisplay(in: host.bounds, to: rep)
        return rep
    }

    /// Write `name-393x852-light.png` (and dark) for both sizes into `dir`; returns the file names.
    public static func write(_ model: MonitorModel, name: String, pushed: [Page], to dir: URL) throws -> [String] {
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        var names: [String] = []
        for size in sizes {
            for dark in [false, true] {
                guard let rep = bitmap(model, pushed: pushed, size: size, dark: dark),
                      let png = rep.representation(using: .png, properties: [:]) else { throw CocoaError(.fileWriteUnknown) }
                let file = "\(name)-\(sizeName(size))-\(dark ? "dark" : "light").png"
                try png.write(to: dir.appendingPathComponent(file))
                names.append(file)
            }
        }
        return names
    }
}
#endif
