// SPDX-License-Identifier: GPL-3.0-or-later

import Foundation

/// How the decision card is driven from the keyboard, as plain logic so it is
/// tested without a window: the arrow keys move a selection over the options,
/// Return picks the selected one, and the "Other" box sends on Enter and takes a
/// new line on Shift-Enter. (The same rules as the Compose client.)
public struct OptionSelection: Equatable, Sendable {
    public let count: Int
    /// The highlighted option, if any. Nothing is highlighted until a key is pressed.
    public private(set) var index: Int?

    public init(count: Int, recommended: Int? = nil) {
        self.count = count
        self.index = recommended.flatMap { $0 >= 0 && $0 < count ? $0 : nil }
    }

    public mutating func down() {
        guard count > 0 else { return }
        index = index.map { min($0 + 1, count - 1) } ?? 0
    }

    public mutating func up() {
        guard count > 0 else { return }
        index = index.map { max($0 - 1, 0) } ?? count - 1
    }

    /// The answer Return sends: the highlighted option's number (1-based), as the command line takes it.
    public var answer: String? { index.map { String($0 + 1) } }
}

public enum OtherKey: Equatable, Sendable {
    case send, newline
}

/// What Return does in the "Other" box.
public func otherBoxKey(shift: Bool) -> OtherKey { shift ? .newline : .send }

/// The text the Other box sends: trimmed, and nothing when it is blank.
public func otherAnswer(_ text: String) -> String? {
    let t = text.trimmingCharacters(in: .whitespacesAndNewlines)
    return t.isEmpty ? nil : t
}
