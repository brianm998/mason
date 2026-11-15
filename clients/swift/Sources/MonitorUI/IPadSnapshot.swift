// SPDX-License-Identifier: GPL-3.0-or-later

import CoreGraphics
import Foundation
import ImageIO
import MonitorCore
import SwiftUI
import UniformTypeIdentifiers

/// Offscreen pictures of the monitor at iPad landscape sizes (no simulator, nothing on screen).
@MainActor
public enum IPadSnapshot {
    /// The sizes in points: 12.9" and 11" iPad, landscape.
    public static let sizes: [CGSize] = [CGSize(width: 1366, height: 1024), CGSize(width: 1180, height: 820)]
    public static let scale: CGFloat = 2

    public static func sizeName(_ size: CGSize) -> String { "\(Int(size.width))x\(Int(size.height))" }

    /// `RootView` laid out at exactly `size`, light or dark.
    public static func view(_ model: MonitorModel, size: CGSize, dark: Bool) -> some View {
        RootView(model: model)
            .environment(\.colorScheme, dark ? .dark : .light)
            .frame(width: size.width, height: size.height)
    }

    /// The picture, at `scale` (so pixels are size × 2).
    public static func image(_ model: MonitorModel, size: CGSize, dark: Bool) -> CGImage? {
        let r = ImageRenderer(content: view(model, size: size, dark: dark))
        r.scale = scale
        return r.cgImage
    }

    public static func png(_ image: CGImage) -> Data? {
        let data = NSMutableData()
        guard let d = CGImageDestinationCreateWithData(data, UTType.png.identifier as CFString, 1, nil) else { return nil }
        CGImageDestinationAddImage(d, image, nil)
        return CGImageDestinationFinalize(d) ? data as Data : nil
    }

    /// File name for a shot: the size is in it, e.g. `home-1366x1024-dark.png`.
    public static func fileName(_ page: String, size: CGSize, dark: Bool) -> String {
        "\(page)-\(sizeName(size))-\(dark ? "dark" : "light").png"
    }

    /// Render `page` at every size, light and dark, into `dir`. Returns the file names written.
    public static func write(_ model: MonitorModel, page: String, to dir: URL) throws -> [String] {
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        var names: [String] = []
        for size in sizes {
            for dark in [false, true] {
                guard let img = image(model, size: size, dark: dark), let png = png(img) else {
                    throw CocoaError(.fileWriteUnknown)
                }
                let name = fileName(page, size: size, dark: dark)
                try png.write(to: dir.appendingPathComponent(name))
                names.append(name)
            }
        }
        return names
    }

    /// The width the top bar needs when squeezed as far as it goes (text may truncate, nothing else
    /// gives): the measurement `TopBarFitTests` uses. It fits an iPad when this is ≤ the width.
    public static func topBarLeastWidth(_ model: MonitorModel) -> CGFloat {
        let host = PlatformHost(rootView: TopBar(model: model).environment(\.theme, Theme(dark: false, scale: model.textScale)))
        return host.sizeThatFits(in: CGSize(width: 0, height: 10_000)).width
    }
}

#if canImport(AppKit)
import AppKit
typealias PlatformHost = NSHostingController
#else
import UIKit
typealias PlatformHost = UIHostingController
#endif
