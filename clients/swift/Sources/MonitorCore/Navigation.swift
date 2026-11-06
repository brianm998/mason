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

    /// The `mason://` address of a page, for deep links from notifications.
    public var url: URL {
        switch self {
        case .home: return URL(string: "mason://home")!
        case .task(let id): return URL(string: "mason://task/\(id)")!
        case .landing: return URL(string: "mason://landing")!
        case .usage: return URL(string: "mason://usage")!
        case .pipeline: return URL(string: "mason://pipeline")!
        case .health: return URL(string: "mason://health")!
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
        default: return nil
        }
    }
}

/// Browser-style history: back, forward, home. Selecting anything replaces
/// what was shown and clears what could be gone forward to.
public struct NavHistory: Equatable, Sendable {
    public private(set) var current: Page = .home
    public private(set) var back: [Page] = []
    public private(set) var forward: [Page] = []

    public init() {}

    public var canGoBack: Bool { !back.isEmpty }
    public var canGoForward: Bool { !forward.isEmpty }

    public mutating func go(_ page: Page) {
        guard page != current else { return }
        back.append(current)
        forward.removeAll()
        current = page
    }

    public mutating func goBack() {
        guard let p = back.popLast() else { return }
        forward.append(current)
        current = p
    }

    public mutating func goForward() {
        guard let p = forward.popLast() else { return }
        back.append(current)
        current = p
    }

    /// Home clears the selection, and is itself a step in the history.
    public mutating func goHome() { go(.home) }
}
