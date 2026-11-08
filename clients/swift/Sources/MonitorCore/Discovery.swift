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
        info(arguments: arguments, environment: environment)?.port
    }

    /// The same, with the pid and start time from `daemon.json`, read afresh each
    /// call: a monitor asks again after every failure and while connected.
    public static func info(arguments: [String], environment: [String: String] = ProcessInfo.processInfo.environment) -> DaemonInfo? {
        if let i = arguments.firstIndex(of: "--port"), i + 1 < arguments.count, let p = UInt16(arguments[i + 1]) {
            return DaemonInfo(port: p)
        }
        var dir = defaultDataDir
        if let d = environment["MASON_DATA_DIR"] { dir = URL(fileURLWithPath: d) }
        if let i = arguments.firstIndex(of: "--data-dir"), i + 1 < arguments.count {
            dir = URL(fileURLWithPath: arguments[i + 1])
        }
        return info(file: dir.appendingPathComponent("daemon.json"))
    }

    public static func info(file: URL) -> DaemonInfo? {
        guard let data = try? Data(contentsOf: file),
              let v = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let p = v["port"] as? Int, let port = UInt16(exactly: p) else { return nil }
        return DaemonInfo(port: port, pid: v["pid"] as? Int, startedMs: v["started_ms"] as? Int)
    }
}

/// What a `_mason._tcp` Bonjour record says (TXT: proto, fp, version; the instance is
/// "MASON on <host>").
public struct ServiceRecord: Equatable, Sendable {
    public static let type = "_mason._tcp"
    public var instance: String
    public var proto: Int
    public var fingerprint: String
    public var version: String

    /// nil when the record lacks a valid fingerprint or protocol: such a record is ignored.
    public static func parse(instance: String, txt: [String: String]) -> ServiceRecord? {
        guard let fp = txt["fp"]?.lowercased(), Pin.isFingerprint(fp),
              let proto = txt["proto"].flatMap({ Int($0) }) else { return nil }
        return ServiceRecord(instance: instance, proto: proto, fingerprint: fp, version: txt["version"] ?? "")
    }
}

#if canImport(Network)
import Network

/// Looks for daemons on the local network. The app's Info.plist must declare `_mason._tcp` in
/// NSBonjourServices and say why in NSLocalNetworkUsageDescription, or iOS shows nothing.
public final class BonjourBrowser: @unchecked Sendable {
    private let browser = NWBrowser(for: .bonjourWithTXTRecord(type: ServiceRecord.type, domain: nil), using: .tcp)
    public var onChange: (([(record: ServiceRecord, endpoint: NWEndpoint)]) -> Void)?
    public init() {}

    public func start() {
        browser.browseResultsChangedHandler = { [weak self] results, _ in
            let found: [(record: ServiceRecord, endpoint: NWEndpoint)] = results.compactMap { r in
                guard case let .service(name, _, _, _) = r.endpoint,
                      case let .bonjour(txt) = r.metadata else { return nil }
                var d: [String: String] = [:]
                for (k, v) in txt.dictionary { d[k] = v }
                return ServiceRecord.parse(instance: name, txt: d).map { ($0, r.endpoint) }
            }
            self?.onChange?(found)
        }
        browser.start(queue: .main)
    }

    public func stop() { browser.cancel() }
}
#endif
