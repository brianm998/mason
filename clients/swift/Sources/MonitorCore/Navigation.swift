// SPDX-License-Identifier: GPL-3.0-or-later

import Foundation

/// What the main view shows: exactly one of these at a time.
public enum Page: Hashable, Sendable {
    case home
    case task(UInt64)
    case landing
    case usage
    case pipeline
    case health
    case settings

    /// The `mason://` address of a page, for deep links from notifications.
    public var url: URL {
        switch self {
        case .home: return URL(string: "mason://home")!
        case .task(let id): return URL(string: "mason://task/\(id)")!
        case .landing: return URL(string: "mason://landing")!
        case .usage: return URL(string: "mason://usage")!
        case .pipeline: return URL(string: "mason://pipeline")!
        case .health: return URL(string: "mason://health")!
        case .settings: return URL(string: "mason://settings")!
        }
    }

    /// `mason://task/12`, `mason://landing`, ... `nil` for anything else.
    public static func from(url: URL) -> Page? {
        guard url.scheme == "mason", let host = url.host else { return nil }
        let rest = url.pathComponents.filter { $0 != "/" }
        switch (host, rest.first) {
        case ("home", nil): return .home
        case ("task", let id?): return UInt64(id).map(Page.task)
        case ("landing", nil): return .landing
        case ("usage", nil): return .usage
        case ("pipeline", nil): return .pipeline
        case ("health", nil): return .health
        case ("settings", nil): return .settings
        default: return nil
        }
    }
}

/// Browser-style history: back, forward, home. Selecting anything replaces
/// what was shown and clears what could be gone forward to. Each entry carries
/// the vertical scroll position it was left at, so back and forward put a page
/// where it was (the same page twice in the stack keeps two positions).
public struct NavHistory: Equatable, Sendable {
    public private(set) var current: Page = .home
    public private(set) var back: [Page] = []
    public private(set) var forward: [Page] = []
    /// Saved positions, in step with `back` and `forward`.
    public private(set) var backScroll: [Double] = []
    public private(set) var forwardScroll: [Double] = []
    /// Where the current entry should open: what was saved when it was left, 0 after `go`.
    public private(set) var restore: Double = 0
    /// Counts arrivals, so a view can tell one visit from the next.
    public private(set) var visit: Int = 0

    public init() {}

    public var canGoBack: Bool { !back.isEmpty }
    public var canGoForward: Bool { !forward.isEmpty }

    /// `scroll` is where the current page is scrolled now; it is saved on its entry as it is left.
    public mutating func go(_ page: Page, scroll: Double = 0) {
        guard page != current else { return }
        back.append(current); backScroll.append(scroll)
        forward.removeAll(); forwardScroll.removeAll()
        current = page; restore = 0; visit += 1
    }

    public mutating func goBack(scroll: Double = 0) {
        guard let p = back.popLast(), let s = backScroll.popLast() else { return }
        forward.append(current); forwardScroll.append(scroll)
        current = p; restore = s; visit += 1
    }

    public mutating func goForward(scroll: Double = 0) {
        guard let p = forward.popLast(), let s = forwardScroll.popLast() else { return }
        back.append(current); backScroll.append(scroll)
        current = p; restore = s; visit += 1
    }

    /// Home clears the selection, and is itself a step in the history.
    public mutating func goHome(scroll: Double = 0) { go(.home, scroll: scroll) }
}
