// SPDX-License-Identifier: GPL-3.0-or-later

import Foundation

public enum Format {
    /// Time in state: "42 s", "9 min", "3h10". Always shown, never hidden.
    public static func duration(ms: UInt64) -> String {
        let s = ms / 1000
        switch s {
        case 0..<60: return "\(s) s"
        case 60..<3600: return "\(s / 60) min"
        default:
            let m = (s % 3600) / 60
            return m == 0 ? "\(s / 3600)h" : "\(s / 3600)h\(String(format: "%02d", m))"
        }
    }
}
