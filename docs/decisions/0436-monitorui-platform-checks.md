# 0436: MonitorUI uses platform checks, not a separate iOS target

AppKit-only calls in MonitorUI are wrapped in `#if canImport(AppKit)` with UIKit/SwiftUI equivalents, so the Mac and iPad monitors share one set of views. Mac behaviour is unchanged; a tile's tap goes through one shared function on both platforms.
