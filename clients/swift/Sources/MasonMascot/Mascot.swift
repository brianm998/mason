// SPDX-License-Identifier: GPL-3.0-or-later
//
// The one way the SwiftUI monitors load the mascot art. Ids are the daemon's own strings;
// an id (or size) that is not in the manifest gives nil, never a crash. Every image is
// decorative: callers put `.accessibilityHidden(true)` on it.

import Foundation
import SwiftUI

#if canImport(AppKit)
    import AppKit
#elseif canImport(UIKit)
    import UIKit
#endif

public enum Mascot {
    public static let resourceBundleName = "MasonMonitor_MasonMascot.bundle"

    /// Where the art folder is: in the app's Resources (MASON.app), beside the executable
    /// (`swift run`, `monitor-tool`), or beside the test bundle. SwiftPM's own `Bundle.module`
    /// is not used: it falls back to the build directory and traps when that is gone.
    static let artRoot: URL? = {
        var dirs: [URL] = []
        if let r = Bundle.main.resourceURL { dirs.append(r) }
        dirs.append(Bundle.main.bundleURL)
        if let e = Bundle.main.executableURL { dirs.append(e.deletingLastPathComponent()) }
        for b in Bundle.allBundles { dirs.append(b.bundleURL.deletingLastPathComponent()) }
        if let o = ProcessInfo.processInfo.environment["MASON_MASCOT_DIR"] {
            dirs.insert(URL(fileURLWithPath: o), at: 0)
        }
        for d in dirs {
            let art = d.appendingPathComponent(resourceBundleName).appendingPathComponent("Art")
            if FileManager.default.fileExists(atPath: art.path) { return art }
        }
        return nil
    }()

    // MARK: Data (what the tests and tools read)

    static func data(_ path: String) -> Data? {
        guard MascotManifest.files.contains(path), let root = artRoot else { return nil }
        return try? Data(contentsOf: root.appendingPathComponent(path))
    }

    /// The PNG files of a state icon at one size (20 or 24), by scale 1...3.
    public static func iconFiles(_ id: String, size: Int) -> [String] {
        guard MascotManifest.stateIds.contains(id) else { return [] }
        return (1...3).map { "icons/\(id)-\(size)@\($0)x.png" }.filter(MascotManifest.files.contains)
    }

    public static func bannerIconFiles(_ id: String) -> [String] {
        guard MascotManifest.bannerIds.contains(id) else { return [] }
        return (1...3).map { "banners/\(id)-20@\($0)x.png" }.filter(MascotManifest.files.contains)
    }

    public static func illustrationFile(_ id: String) -> String? {
        MascotManifest.illustrationIds.contains(id) ? "illustrations/\(id).png" : nil
    }

    // MARK: Images

    /// The icon for a task state (`needs_attention`, `queued`, ...) at 20 or 24 points.
    public static func icon(_ id: String, size: Int) -> Image? {
        make(iconFiles(id, size: size), points: size)
    }

    /// The icon for a banner kind (`connection_lost`, `paused`, ...), 20 points.
    public static func bannerIcon(_ id: String) -> Image? {
        make(bannerIconFiles(id), points: 20)
    }

    /// The 512 x 512 illustration for a state or an empty view (`all_clear`, `no_tasks`, ...).
    public static func illustration(_ id: String) -> Image? {
        guard let f = illustrationFile(id), let d = data(f) else { return nil }
        return image(from: [(1, d)], points: 512)
    }

    /// A brand image (`start_image`), the whole picture at 3:2; nil for an unknown id.
    public static func brand(_ id: String) -> Image? {
        let f = "brand/\(id).png"
        guard MascotManifest.brandIds.contains(id), let d = data(f) else { return nil }
        return image(from: [(1, d)], points: 768)
    }

    /// The 1024 px app icon (`brand/app_icon.png`), for the Dock when the monitor runs unbundled.
    public static func appIconData() -> Data? { data("brand/app_icon.png") }

    /// The macOS-shaped app icon (`brand/app_icon_macos.png`): the same art inside Apple's
    /// 824 px rounded rectangle on a 1024 canvas with a shadow, since macOS does not mask icons.
    /// Made by `scripts/gen-app-icon-macos.swift`.
    public static func macAppIconData() -> Data? { data("brand/app_icon_macos.png") }

    private static func make(_ files: [String], points: Int) -> Image? {
        let reps: [(Int, Data)] = files.enumerated().compactMap { i, f in data(f).map { (i + 1, $0) } }
        return image(from: reps, points: points)
    }

    #if canImport(AppKit)
        private static func image(from reps: [(Int, Data)], points: Int) -> Image? {
            nsImage(from: reps, points: points).map { Image(nsImage: $0) }
        }

        /// `points` is the width in points; the height follows the bitmap's pixel aspect, so the
        /// 1536 x 1024 start image is 768 x 512 points and square art stays square. The first
        /// readable file sets the size; a later rep whose aspect differs by more than a pixel is
        /// skipped, never stretched, so every rep reports the same point size.
        static func nsImage(from reps: [(Int, Data)], points: Int) -> NSImage? {
            var size: NSSize?
            var first: (w: Int, h: Int)?
            var kept: [NSBitmapImageRep] = []
            for (_, d) in reps {
                guard let rep = NSBitmapImageRep(data: d), rep.pixelsWide > 0, rep.pixelsHigh > 0 else { continue }
                if let f = first {
                    let expected = Double(rep.pixelsWide) * Double(f.h) / Double(f.w)
                    if abs(expected - Double(rep.pixelsHigh)) > 1 { continue }
                } else {
                    first = (rep.pixelsWide, rep.pixelsHigh)
                    size = NSSize(width: CGFloat(points), height: CGFloat(points) * CGFloat(rep.pixelsHigh) / CGFloat(rep.pixelsWide))
                }
                rep.size = size!
                kept.append(rep)
            }
            guard let size, !kept.isEmpty else { return nil }
            let img = NSImage(size: size)
            for rep in kept { img.addRepresentation(rep) }
            return img
        }
    #elseif canImport(UIKit)
        private static func image(from reps: [(Int, Data)], points: Int) -> Image? {
            let want = max(1, min(3, Int(UITraitCollection.current.displayScale.rounded())))
            guard let (scale, d) = reps.first(where: { $0.0 == want }) ?? reps.last,
                let ui = UIImage(data: d, scale: CGFloat(scale))
            else { return nil }
            return Image(uiImage: ui)
        }
    #endif

    /// Loads every file in the manifest; prints one line and returns whether all of them loaded.
    /// `MASON.app/Contents/MacOS/MasonMonitor --mascot-check` and `monitor-tool mascot-check`
    /// run it, which is how a built app is shown to carry its art.
    public static func selfCheck() -> Bool {
        let loaded = MascotManifest.files.filter { data($0) != nil }.count
        let ok = loaded == MascotManifest.files.count && icon("queued", size: 24) != nil
        print("mascot: \(loaded) of \(MascotManifest.files.count) files loaded from \(artRoot?.path ?? "nowhere")")
        #if canImport(AppKit)
            if let d = data("brand/start_image.png"), let img = nsImage(from: [(1, d)], points: 768) {
                print("mascot: start_image is \(img.size.width) x \(img.size.height) points")
            } else {
                print("mascot: start_image did not load")
                return false
            }
        #endif
        return ok
    }
}
