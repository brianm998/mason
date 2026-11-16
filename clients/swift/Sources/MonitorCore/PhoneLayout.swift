// SPDX-License-Identifier: GPL-3.0-or-later

import Foundation

/// The phone layout (Part 17, narrow screens), with nothing drawn. The sidebar is the first
/// screen: count tiles scroll sideways above a Home row, then the state sections and the other
/// pages. Choosing a row pushes its detail; Back pops. The Compose core has the same rules in
/// `PhoneLayout.kt`, and the conformance items hold them to the same order.
public enum PhoneBlock: Equatable, Sendable {
    /// The count tiles, one row that scrolls sideways.
    case tiles
    /// The Home row, the sidebar's top row.
    case home
    /// The state sections with their tasks.
    case sections
    case landing
    case usage
}

public enum PhoneLayout {
    /// The sidebar screen, top to bottom.
    public static let sidebarOrder: [PhoneBlock] = [.tiles, .home, .sections, .landing, .usage]

    /// The first screen is always the sidebar.
    public static let firstScreen = PhoneScreen.sidebar

    /// Graph editing, log deep-dives and agent sessions are not on a phone (deliberate
    /// differences, recorded in `clients/features.toml`).
    public static let phoneHasNo: [String] = ["graph editing", "log deep-dives", "agent sessions"]

    /// Pages listed after Usage at the foot of the sidebar screen. Android's core does not list
    /// them, so they are outside `sidebarOrder`, which the conformance items hold equal.
    public static let morePages: [Page] = [.health, .settings]

    /// Whether the phone has this page. Every page the Mac monitor has is pushed on a phone
    /// except the pipeline editor (graph editing, in `phoneHasNo`).
    public static func hasPage(_ page: Page) -> Bool {
        if case .pipeline = page { return false }
        return true
    }

    /// The compact status line in the navigation bar: the connection, "Paused" while a pause
    /// banner is up, and how many tasks need the owner (only while any do).
    public static func statusLine(connected: Bool, reconnecting: Bool, paused: Bool, attention: UInt32) -> String {
        var parts = [connected ? "Connected" : (reconnecting ? "Reconnecting…" : "Offline")]
        if paused { parts.append("Paused") }
        if attention > 0 { parts.append("\(attention) need\(attention == 1 ? "s" : "") you") }
        return parts.joined(separator: " · ")
    }

    /// What tapping a count tile pushes: the lone task when the state holds exactly one, else
    /// the state's own page (the Mac's double click, as one tap). `nil` while disconnected and
    /// for a state with no page of its own, where the tap only scrolls to its section.
    public static func tileTarget(state: String, rows: [UInt64], connected: Bool) -> Page? {
        guard connected else { return nil }
        if rows.count == 1 { return .task(rows[0]) }
        return Page.sectionPage(state)
    }
}

public enum PhoneScreen: Equatable, Sendable {
    case sidebar
    case detail(Page)
}

/// The phone's navigation: a stack of pushed pages over the sidebar.
public struct PhoneNav: Equatable, Sendable {
    public private(set) var stack: [Page] = []
    public init() {}
    /// Starts with these pages pushed (a screenshot of a pushed page, a restored session).
    public init(stack: [Page]) { self.stack = stack.filter(PhoneLayout.hasPage) }

    public var screen: PhoneScreen {
        stack.last.map(PhoneScreen.detail) ?? .sidebar
    }

    /// Selecting a row pushes its detail. Home is a row like the rest: it pushes the home page.
    /// A page the phone has none of is not pushed.
    public mutating func select(_ page: Page) {
        guard PhoneLayout.hasPage(page) else { return }
        if stack.last != page { stack.append(page) }
    }

    /// The model went to `page` by itself (a link inside a page, a notification): show it. The
    /// model sits on Home while the sidebar shows, so that is not a push.
    public mutating func follow(_ page: Page) {
        if page == .home && stack.isEmpty { return }
        select(page)
    }

    /// The stack as the navigation view reports it after a swipe back or a pop.
    public mutating func replace(_ pages: [Page]) { stack = pages.filter(PhoneLayout.hasPage) }

    /// Back pops one; from the sidebar there is nothing to go back to.
    public mutating func back() { _ = stack.popLast() }
}
