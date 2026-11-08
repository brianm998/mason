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
}

public enum PhoneScreen: Equatable, Sendable {
    case sidebar
    case detail(Page)
}

/// The phone's navigation: a stack of pushed pages over the sidebar.
public struct PhoneNav: Equatable, Sendable {
    public private(set) var stack: [Page] = []
    public init() {}

    public var screen: PhoneScreen {
        stack.last.map(PhoneScreen.detail) ?? .sidebar
    }

    /// Selecting a row pushes its detail. Home is a row like the rest: it pushes the home page.
    public mutating func select(_ page: Page) {
        if stack.last != page { stack.append(page) }
    }

    /// Back pops one; from the sidebar there is nothing to go back to.
    public mutating func back() { _ = stack.popLast() }
}
