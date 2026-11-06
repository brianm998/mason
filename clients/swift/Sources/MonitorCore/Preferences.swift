// SPDX-License-Identifier: GPL-3.0-or-later

import Foundation

/// Per-device memory: which sidebar sections the user collapsed, the text
/// scale, mute. (UserDefaults on a device; a dictionary in tests.)
public protocol Preferences: AnyObject {
    func bool(_ key: String) -> Bool?
    func setBool(_ value: Bool, for key: String)
    func double(_ key: String) -> Double?
    func setDouble(_ value: Double, for key: String)
}

public final class MemoryPreferences: Preferences {
    private var values: [String: Any] = [:]
    public init() {}
    public func bool(_ key: String) -> Bool? { values[key] as? Bool }
    public func setBool(_ value: Bool, for key: String) { values[key] = value }
    public func double(_ key: String) -> Double? { values[key] as? Double }
    public func setDouble(_ value: Double, for key: String) { values[key] = value }
}

public final class DefaultsPreferences: Preferences {
    private let defaults: UserDefaults
    public init(_ defaults: UserDefaults = .standard) { self.defaults = defaults }
    public func bool(_ key: String) -> Bool? { defaults.object(forKey: key) as? Bool }
    public func setBool(_ value: Bool, for key: String) { defaults.set(value, forKey: key) }
    public func double(_ key: String) -> Double? { defaults.object(forKey: key) as? Double }
    public func setDouble(_ value: Double, for key: String) { defaults.set(value, forKey: key) }
}
