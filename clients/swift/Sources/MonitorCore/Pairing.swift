// SPDX-License-Identifier: GPL-3.0-or-later

import Foundation
#if canImport(CryptoKit)
import CryptoKit
#endif
#if canImport(Security)
import Security
#endif

/// What `mason pair` shows as a QR code (or types as a short code):
/// `mason://pair?host=H&port=P&fp=SHA256HEX&code=12345678`.
public struct PairURL: Equatable, Sendable {
    public var host: String
    public var port: UInt16
    public var fingerprint: String
    public var code: String

    public init(host: String, port: UInt16, fingerprint: String, code: String) {
        self.host = host
        self.port = port
        self.fingerprint = fingerprint
        self.code = code
    }

    /// nil for anything that is not a well-formed pairing address.
    public static func parse(_ text: String) -> PairURL? {
        guard let c = URLComponents(string: text.trimmingCharacters(in: .whitespacesAndNewlines)),
              c.scheme == "mason", c.host == "pair" else { return nil }
        var q: [String: String] = [:]
        for i in c.queryItems ?? [] { if let v = i.value { q[i.name] = v } }
        guard let host = q["host"], !host.isEmpty,
              let p = q["port"].flatMap({ UInt16($0) }), p > 0,
              let fp = q["fp"]?.lowercased(), Pin.isFingerprint(fp),
              let code = q["code"], Pin.isCode(code) else { return nil }
        return PairURL(host: host, port: p, fingerprint: fp, code: code)
    }
}

public enum Pin {
    public static func isFingerprint(_ s: String) -> Bool {
        s.count == 64 && s.allSatisfy { $0.isHexDigit && ($0.isNumber || $0.isLowercase) }
    }
    public static func isCode(_ s: String) -> Bool {
        s.count == 8 && s.allSatisfy { $0.isASCII && $0.isNumber }
    }

    public static func hex(_ bytes: [UInt8]) -> String { bytes.map { String(format: "%02x", $0) }.joined() }

    #if canImport(CryptoKit)
    /// The SHA-256 of a DER certificate, as the daemon computes it.
    public static func fingerprint(der: Data) -> String { hex(Array(SHA256.hash(data: der))) }
    #endif

    /// Does the certificate the server showed match the pin? Compared without early exit.
    public static func matches(shown: String, pinned: String) -> Bool {
        let a = Array(shown.lowercased().utf8), b = Array(pinned.lowercased().utf8)
        guard a.count == b.count else { return false }
        return zip(a, b).reduce(UInt8(0)) { $0 | ($1.0 ^ $1.1) } == 0
    }
}

/// A paired daemon: where it is, the certificate to expect, and our token.
public struct PairedDaemon: Equatable, Sendable {
    public var host: String
    public var port: UInt16
    public var fingerprint: String
    public var token: String
    public var role: String
    public init(host: String, port: UInt16, fingerprint: String, token: String, role: String) {
        self.host = host; self.port = port; self.fingerprint = fingerprint; self.token = token; self.role = role
    }
}

/// Where tokens live. They are keyed by the certificate fingerprint, so a daemon that comes
/// back with another certificate finds no token and must be paired again.
public protocol TokenStore: AnyObject {
    func token(forFingerprint fp: String) -> String?
    func setToken(_ token: String?, forFingerprint fp: String)
}

public enum TokenKey {
    public static let service = "mason.pairing"
    public static func account(fingerprint: String) -> String { "token:" + fingerprint.lowercased() }
}

public final class MemoryTokenStore: TokenStore {
    private var items: [String: String] = [:]
    public init() {}
    public func token(forFingerprint fp: String) -> String? { items[TokenKey.account(fingerprint: fp)] }
    public func setToken(_ token: String?, forFingerprint fp: String) { items[TokenKey.account(fingerprint: fp)] = token }
}

#if canImport(Security)
/// The Keychain (this device only, after first unlock).
public final class KeychainTokenStore: TokenStore {
    public init() {}
    private func query(_ fp: String) -> [String: Any] {
        [kSecClass as String: kSecClassGenericPassword,
         kSecAttrService as String: TokenKey.service,
         kSecAttrAccount as String: TokenKey.account(fingerprint: fp)]
    }
    public func token(forFingerprint fp: String) -> String? {
        var q = query(fp)
        q[kSecReturnData as String] = true
        q[kSecMatchLimit as String] = kSecMatchLimitOne
        var out: CFTypeRef?
        guard SecItemCopyMatching(q as CFDictionary, &out) == errSecSuccess, let d = out as? Data else { return nil }
        return String(data: d, encoding: .utf8)
    }
    public func setToken(_ token: String?, forFingerprint fp: String) {
        SecItemDelete(query(fp) as CFDictionary)
        guard let t = token else { return }
        var q = query(fp)
        q[kSecValueData as String] = Data(t.utf8)
        q[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        SecItemAdd(q as CFDictionary, nil)
    }
}
#endif

#if canImport(CryptoKit)
/// Accepts the server's certificate only when its SHA-256 is the pinned one.
public final class PinningDelegate: NSObject, URLSessionDelegate, @unchecked Sendable {
    let pinned: String
    public private(set) var rejected = false
    public init(pinned: String) { self.pinned = pinned }

    public func urlSession(_ session: URLSession, didReceive challenge: URLAuthenticationChallenge,
                           completionHandler: @escaping (URLSession.AuthChallengeDisposition, URLCredential?) -> Void) {
        guard challenge.protectionSpace.authenticationMethod == NSURLAuthenticationMethodServerTrust,
              let trust = challenge.protectionSpace.serverTrust,
              let chain = SecTrustCopyCertificateChain(trust) as? [SecCertificate],
              let leaf = chain.first else {
            completionHandler(.cancelAuthenticationChallenge, nil)
            return
        }
        let shown = Pin.fingerprint(der: SecCertificateCopyData(leaf) as Data)
        if Pin.matches(shown: shown, pinned: pinned) {
            completionHandler(.useCredential, URLCredential(trust: trust))
        } else {
            rejected = true
            completionHandler(.cancelAuthenticationChallenge, nil)
        }
    }
}

public enum PairingError: Error, Equatable {
    case refused(Int, String)
    case badAnswer
}

public enum Pairing {
    /// Exchange the code for a token, over TLS pinned to the fingerprint in the URL, and keep it.
    public static func pair(_ url: PairURL, deviceName: String, store: TokenStore) async throws -> PairedDaemon {
        let delegate = PinningDelegate(pinned: url.fingerprint)
        let session = URLSession(configuration: .ephemeral, delegate: delegate, delegateQueue: nil)
        defer { session.finishTasksAndInvalidate() }
        var req = URLRequest(url: URL(string: "https://\(url.host):\(url.port)/pair")!)
        req.httpMethod = "POST"
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        req.httpBody = try JSONSerialization.data(withJSONObject: ["code": url.code, "name": deviceName])
        let (data, resp) = try await session.data(for: req)
        let status = (resp as? HTTPURLResponse)?.statusCode ?? 0
        let obj = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] ?? [:]
        guard status == 200 else { throw PairingError.refused(status, obj["error"] as? String ?? "refused") }
        guard let token = obj["token"] as? String, let role = obj["role"] as? String else { throw PairingError.badAnswer }
        store.setToken(token, forFingerprint: url.fingerprint)
        return PairedDaemon(host: url.host, port: url.port, fingerprint: url.fingerprint, token: token, role: role)
    }
}

/// The monitor's socket to a paired daemon: `wss`, pinned, with the token on the upgrade.
@MainActor
public final class PinnedSocket: MonitorSocket {
    private let daemon: PairedDaemon
    private let delegate: PinningDelegate
    private let session: URLSession
    private var task: URLSessionWebSocketTask?

    public init(daemon: PairedDaemon) {
        self.daemon = daemon
        delegate = PinningDelegate(pinned: daemon.fingerprint)
        session = URLSession(configuration: .ephemeral, delegate: delegate, delegateQueue: nil)
    }

    public func open(port: UInt16) {
        var req = URLRequest(url: URL(string: "wss://\(daemon.host):\(daemon.port)/ws")!)
        req.setValue("Bearer \(daemon.token)", forHTTPHeaderField: "Authorization")
        let t = session.webSocketTask(with: req)
        task = t
        t.resume()
    }

    public func send(_ text: String) async throws { try await task?.send(.string(text)) }

    public func receive() async throws -> String {
        guard let t = task else { throw URLError(.cancelled) }
        switch try await t.receive() {
        case .string(let s): return s
        case .data(let d): return String(decoding: d, as: UTF8.self)
        @unknown default: return ""
        }
    }

    public func close() {
        task?.cancel(with: .goingAway, reason: nil)
        session.invalidateAndCancel()
    }
}
#endif
