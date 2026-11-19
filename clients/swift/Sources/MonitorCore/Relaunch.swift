// SPDX-License-Identifier: GPL-3.0-or-later

/// Starting a monitor binary again after the daemon rebuilt it in place.
public enum Relaunch {
    /// Set by `make run` for the binary it started. The rebuilt binary carries its own commit in
    /// `BuildStamp`, so a copy of this from the old process would make the new one report the old
    /// commit, show the update banner again and rebuild the same commit on every press.
    public static let versionKey = "MASON_MONITOR_VERSION"

    /// The environment for the new process: this one's, without the version override.
    public static func environment(from current: [String: String]) -> [String: String] {
        var env = current
        env.removeValue(forKey: versionKey)
        return env
    }
}
