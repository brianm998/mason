// SPDX-License-Identifier: GPL-3.0-or-later

import Combine
import Foundation

/// Words for a pairing that did not work. The daemon's own message is kept (it says what is
/// wrong); the status only picks the lead-in. Pure.
public enum PairingMessage {
    public static func text(status: Int, daemon: String) -> String {
        let said = daemon.isEmpty ? "" : " (\(daemon))"
        switch status {
        case 400: return "That code is wrong\(said). Check the 8 digits on the Mac and try again."
        case 410: return "That code has expired\(said). Make a new one with `mason pair` and try again."
        case 429: return "Too many wrong codes\(said). Pairing is locked for a while; make a new code later."
        case 401, 403: return "The daemon refused this device\(said). LAN access may be off."
        default: return "The daemon refused the pairing (\(status))\(said)."
        }
    }

    public static func text(_ error: Error) -> String {
        if let e = error as? PairingError {
            switch e {
            case .refused(let s, let m): return text(status: s, daemon: m)
            case .badAnswer: return "The daemon's answer could not be read."
            }
        }
        if let u = error as? URLError, u.code == .cancelled || u.code == .serverCertificateUntrusted {
            return "This is not the daemon the code or fingerprint named: its certificate did not match."
        }
        return "Could not reach the daemon: \(error.localizedDescription)"
    }
}

/// The pairing screen's state: which path, what was scanned or chosen, the typed code, the last
/// error. No I/O; `PairingModel` runs the exchange. Pure.
public struct PairingScreenState: Equatable, Sendable {
    public enum Path: Equatable, Sendable { case scan, nearby }

    public var path: Path = .scan
    /// A daemon picked from the Nearby list (its fingerprint came from unauthenticated mDNS).
    public var chosen: ResolvedService?
    /// A full pairing address: scanned, or opened from a `mason://pair` link.
    public var address: PairURL?
    public var code = ""
    public var error: String?
    public var busy = false

    public init() {}

    /// The caution under the Nearby list.
    public static let nearbyWarning =
        "These names come from the network and are not verified. Scanning the QR code is the stronger check. Compare the fingerprint start with `mason lan status` before you pair."

    /// The first 8 characters of a fingerprint, to compare with `mason lan status`.
    public static func shortFingerprint(_ fp: String) -> String { String(fp.prefix(8)) }

    /// A scan or link result. False (and an error) when the text is not a pairing address.
    @discardableResult
    public mutating func scanned(_ text: String) -> Bool {
        guard let u = PairURL.parse(text) else {
            error = "That is not a MASON pairing code."
            return false
        }
        address = u
        chosen = nil
        code = u.code
        error = nil
        path = .scan
        return true
    }

    public mutating func choose(_ s: ResolvedService) {
        chosen = s
        address = nil
        code = ""
        error = nil
        path = .nearby
    }

    /// Digits only, at most 8.
    public mutating func setCode(_ typed: String) {
        code = String(typed.filter { $0.isASCII && $0.isNumber }.prefix(8))
    }

    public var codeValid: Bool { Pin.isCode(code) }

    /// What to pair with, when there is a target and a valid code.
    public var request: PairURL? {
        guard codeValid else { return nil }
        if let a = address { return PairURL(host: a.host, port: a.port, fingerprint: a.fingerprint, code: code, alternates: a.alternates) }
        if let c = chosen { return PairURL(host: c.host, port: c.port, fingerprint: c.record.fingerprint, code: code) }
        return nil
    }

    public var canSubmit: Bool { request != nil && !busy }

    /// Start a pairing; nil (with an error) when it cannot start.
    public mutating func begin() -> PairURL? {
        guard !busy else { return nil }
        guard let r = request else {
            error = (address == nil && chosen == nil) ? "Choose a daemon or scan its QR code first." : "The code is 8 digits."
            return nil
        }
        busy = true
        error = nil
        return r
    }

    public mutating func failed(_ e: Error) { busy = false; error = PairingMessage.text(e) }
    public mutating func succeeded() { self = PairingScreenState() }
}

#if canImport(CryptoKit)
/// Runs the pairing exchange for the screen, saves the daemon, and tells the session.
@MainActor
public final class PairingModel: ObservableObject {
    @Published public var state = PairingScreenState()
    private let session: RemoteSession
    private let store: PairedStore
    private let tokens: TokenStore
    private let deviceName: String
    private let pair: (PairURL, String, TokenStore) async throws -> PairedDaemon

    public init(session: RemoteSession, store: PairedStore, tokens: TokenStore, deviceName: String,
                pair: ((PairURL, String, TokenStore) async throws -> PairedDaemon)? = nil) {
        self.session = session; self.store = store; self.tokens = tokens; self.deviceName = deviceName
        self.pair = pair ?? { try await Pairing.pair($0, deviceName: $1, store: $2) }
    }

    /// A `mason://pair` link opened the app.
    public func open(url: URL) { state.scanned(url.absoluteString) }

    public func submit() async {
        guard let req = state.begin() else { return }
        do {
            let d = try await pair(req, deviceName, tokens)
            store.save(PairedRecord(d))
            state.succeeded()
            session.pairingChanged()
        } catch {
            state.failed(error)
        }
    }

    /// Settings > Forget: delete the token and go back to pairing.
    public func forget() {
        session.unpair()
        state = PairingScreenState()
    }
}
#endif
