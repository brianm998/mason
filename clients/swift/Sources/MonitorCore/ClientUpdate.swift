// SPDX-License-Identifier: GPL-3.0-or-later

import Foundation

/// How this monitor gets a newer version of itself (Part 15). On desktop the
/// button builds from a source checkout (development) and relaunches; where a
/// signed build is published it would fetch that instead (not built yet); and
/// otherwise there is a page to go to. This only decides; the app does it.
public enum UpdatePlan: Equatable {
    /// Build the monitor from this checkout, launch it, close this one.
    case buildFromSource(String)
    /// Open this page (the install path).
    case openPage(String)
    case unavailable(String)
}

public enum ClientUpdate {
    public static func plan(
        env: [String: String],
        exists: (String) -> Bool = { FileManager.default.fileExists(atPath: $0) }
    ) -> UpdatePlan {
        if let dir = env["MASON_SOURCE_DIR"], !dir.isEmpty {
            if exists(dir + "/scripts/bundle-monitor.sh") { return .buildFromSource(dir) }
            return .unavailable("MASON_SOURCE_DIR is \(dir), but there is no scripts/bundle-monitor.sh in it")
        }
        if let url = env["MASON_INSTALL_URL"], url.hasPrefix("https://") { return .openPage(url) }
        return .unavailable("This monitor does not know where to get a newer one. Set MASON_SOURCE_DIR to a checkout of the source, or MASON_INSTALL_URL to the install page.")
    }
}
