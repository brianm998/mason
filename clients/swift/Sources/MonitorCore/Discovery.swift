// SPDX-License-Identifier: GPL-3.0-or-later

import Foundation

/// Finding the daemon on this machine: loopback only, by the port it
/// announces in `daemon.json` in its data directory.
public enum Discovery {
    public static var defaultDataDir: URL {
        let home = FileManager.default.homeDirectoryForCurrentUser
        return home.appendingPathComponent("Library/Application Support/mason")
    }

    /// `--port N`, else `--data-dir D` (or `MASON_DATA_DIR`), else the default.
    public static func port(arguments: [String], environment: [String: String] = ProcessInfo.processInfo.environment) -> UInt16? {
        if let i = arguments.firstIndex(of: "--port"), i + 1 < arguments.count, let p = UInt16(arguments[i + 1]) {
            return p
        }
        var dir = defaultDataDir
        if let d = environment["MASON_DATA_DIR"] { dir = URL(fileURLWithPath: d) }
        if let i = arguments.firstIndex(of: "--data-dir"), i + 1 < arguments.count {
            dir = URL(fileURLWithPath: arguments[i + 1])
        }
        guard let data = try? Data(contentsOf: dir.appendingPathComponent("daemon.json")),
              let v = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let p = v["port"] as? Int else { return nil }
        return UInt16(exactly: p)
    }
}
