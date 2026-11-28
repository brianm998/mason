// SPDX-License-Identifier: GPL-3.0-or-later

import Foundation
import MasonProtocol

/// The GitHub runner limit control (#1261): the words and the bounds for a `gha` or `gha-macos`
/// pool. The Mac and the phone use the same rules.
public enum RunnerLimit {
    /// The pools the control offers, in order. `gha_ceiling`, `gha-debt` and `gha_full_share` stay
    /// read-only (CLI only).
    public static let pools = ["gha", "gha-macos"]

    public static func title(_ name: String) -> String {
        name == "gha-macos" ? "GitHub macOS runners" : "GitHub runners"
    }

    /// The limit after a step of `delta`, kept between 0 and the account ceiling; nil when that is
    /// the limit already (a button at its bound sends nothing).
    public static func stepped(_ p: Mason_V1_RunnerPool, by delta: Int) -> UInt32? {
        let next = max(0, min(Int(p.ceiling), Int(p.limit) + delta))
        return next == Int(p.limit) ? nil : UInt32(next)
    }

    public static func canDecrease(_ p: Mason_V1_RunnerPool) -> Bool { p.limit > 0 }
    public static func canIncrease(_ p: Mason_V1_RunnerPool) -> Bool { p.limit < p.ceiling }

    /// What the limit allows: the smaller of the limit and the ceiling.
    public static func effective(_ p: Mason_V1_RunnerPool) -> UInt32 { min(p.limit, p.ceiling) }

    /// "13 in use of at most" (the words before the minus and plus).
    public static func head(_ p: Mason_V1_RunnerPool) -> String {
        "\(title(p.name)): \(p.used) in use of at most"
    }

    /// "MASON 9, other workflows 4; account ceiling 20; 2 waiting", and what the limit really is
    /// when the ceiling caps it.
    public static func detail(_ p: Mason_V1_RunnerPool) -> String {
        var parts = ["MASON \(p.mason), other workflows \(p.other)", "account ceiling \(p.ceiling)"]
        if p.limit > p.ceiling { parts.append("the ceiling caps the limit to \(p.ceiling)") }
        parts.append(p.waiting == 0 ? "none waiting" : "\(p.waiting) waiting")
        return parts.joined(separator: "; ")
    }

    /// The standing notes under the control: at 0 nothing goes to GitHub; lowering stops new
    /// grants only.
    public static func notes(_ p: Mason_V1_RunnerPool) -> [String] {
        var out: [String] = []
        if effective(p) == 0 {
            out.append(p.name == "gha-macos" ? "At 0 no macOS job goes to GitHub." : "At 0 no check goes to GitHub.")
        }
        out.append("Lowering the limit stops new grants only; jobs already running finish.")
        return out
    }
}

extension MonitorModel {
    /// The runner pools the daemon sent (`gha`, `gha-macos`); empty from an older daemon.
    public var runnerPools: [Mason_V1_RunnerPool] {
        let sent = home?.runnerPools ?? []
        return RunnerLimit.pools.compactMap { n in sent.first { $0.name == n } }
    }

    /// Set a runner pool's limit (the daemon says if it refuses; the reason shows in `lastResult`).
    public func setRunnerLimit(_ name: String, _ units: UInt32) {
        var c = Mason_V1_Command()
        c.setLimit = .with { $0.resource = name; $0.units = units }
        send(c)
    }

    /// The minus (`by: -1`) and plus (`by: 1`) of a runner pool: the limit moves by `delta`, between
    /// 0 and the account ceiling; at a bound nothing is sent.
    public func stepRunnerLimit(_ name: String, by delta: Int) {
        guard let p = runnerPools.first(where: { $0.name == name }),
              let next = RunnerLimit.stepped(p, by: delta) else { return }
        setRunnerLimit(name, next)
    }
}
