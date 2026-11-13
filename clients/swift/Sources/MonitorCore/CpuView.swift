// SPDX-License-Identifier: GPL-3.0-or-later

import Foundation
import MasonProtocol

/// The home page's CPU section, as data: the bar's segments, the legend rows and the
/// header, from the daemon's `CpuUsage` (shares in permille of the whole machine,
/// highest first). The same rules are in the Compose client's `CpuView.kt`.
public enum Cpu {
    public static let taskColours = 10
    /// A reading older than this (three sampling periods of 5 s) is stale.
    public static let staleMs: UInt64 = 15_000
    /// The legend shows this many rows, then folds the rest into one.
    public static let legendTop = 8
    /// A segment narrower than this share of the bar (1%, in permille) is not drawn.
    public static let minSegmentPermille: UInt32 = 10
}

extension Format {
    /// A share in permille as a percentage with one decimal: 234 is "23.4%", 5 is "0.5%".
    public static func permille(_ p: UInt32) -> String { "\(p / 10).\(p % 10)%" }

    /// The task page's line: "CPU: 23.4% of the machine", or "CPU: not running".
    public static func cpuTaskLine(permille p: Int32) -> String {
        p < 0 ? "CPU: not running" : "CPU: \(permille(UInt32(p))) of the machine"
    }
}

/// One share, ready to draw. `color` is a token name suffix: "1"..."10", "masond", "monitor", "sccache", "other".
public struct CpuRow: Equatable, Sendable {
    public let label: String
    public let name: String
    public let task: UInt64
    public let title: String
    public let permille: UInt32
    public let color: String

    /// "#287 Scheduler: … 23.4%", for a hover or a tap.
    public var tip: String { (title.isEmpty ? name : "\(name) \(title)") + " " + Format.permille(permille) }
}

public struct CpuLayout: Equatable, Sendable {
    /// "CPU 84% busy · 18 cores".
    public let header: String
    /// Why nothing is drawn (stale or missing reading); nil when it is.
    public let hidden: String?
    /// The bar's segments, left to right (each as wide as its permille); empty while hidden.
    public let segments: [CpuRow]
    /// The legend's first rows, in the same order.
    public let rows: [CpuRow]
    /// The rows past the top 8, shown when the folded row is expanded.
    public let more: [CpuRow]
    /// What is left of the bar: idle, in permille.
    public let idlePermille: UInt32

    public var morePermille: UInt32 { more.reduce(0) { $0 + $1.permille } }
    public var moreText: String { "\(more.count) more, \(Format.permille(morePermille)) together" }
}

/// Keeps a task's colour while it is among the shares, so it does not change when the order
/// does. A new task takes the lowest colour no other task holds (not a hash, which collides);
/// a task that leaves frees its colour. With more tasks than colours the extra ones share by id.
public struct CpuColors: Sendable {
    private var held: [UInt64: Int] = [:]
    public init() {}

    public mutating func assign(_ tasks: [UInt64]) -> [UInt64: Int] {
        let present = Set(tasks)
        held = held.filter { present.contains($0.key) }
        for t in present.sorted() where held[t] == nil {
            let used = Set(held.values)
            held[t] = (0..<Cpu.taskColours).first { !used.contains($0) } ?? Int(t % UInt64(Cpu.taskColours))
        }
        return held
    }
}

private func fixedName(_ kind: String) -> String {
    switch kind {
    case "masond": return "MASON daemon"
    case "monitor": return "Monitors"
    case "sccache": return "Shared compiles (sccache)"
    case "other": return "Everything else"
    default: return kind
    }
}

/// The layout for `u` at the daemon's time `nowMs`; nil when the daemon sends no CPU data
/// (an older daemon), which hides the section.
public func cpuLayout(_ u: Mason_V1_CpuUsage?, nowMs: UInt64, colors: [UInt64: Int]) -> CpuLayout? {
    guard let u else { return nil }
    let header = u.busyPermille < 0 ? "CPU" : "CPU \((u.busyPermille + 5) / 10)% busy · \(u.cores) cores"
    var hidden: String?
    if u.busyPermille < 0 || u.sampledMs == 0 {
        hidden = "CPU use is not being read."
    } else if nowMs > u.sampledMs, nowMs - u.sampledMs > Cpu.staleMs {
        hidden = "The CPU reading is \(Format.duration(ms: nowMs - u.sampledMs)) old."
    }
    if let hidden { return CpuLayout(header: header, hidden: hidden, segments: [], rows: [], more: [], idlePermille: 1000) }
    let all = u.shares.map { s -> CpuRow in
        let isTask = s.kind == "task"
        let colour = isTask ? (colors[s.task] ?? Int(s.task % UInt64(Cpu.taskColours))) + 1 : 0
        return CpuRow(
            label: s.label, name: isTask ? s.label : fixedName(s.kind), task: isTask ? s.task : 0,
            title: isTask ? s.title : "", permille: s.permille, color: isTask ? String(colour) : s.kind)
    }
    let total = all.reduce(0) { $0 + $1.permille }
    return CpuLayout(
        header: header, hidden: nil,
        segments: all.filter { $0.permille >= Cpu.minSegmentPermille },
        rows: Array(all.prefix(Cpu.legendTop)), more: Array(all.dropFirst(Cpu.legendTop)),
        idlePermille: total >= 1000 ? 0 : 1000 - total)
}

/// What the CPU section observes: only the CPU data, apart from `MonitorModel`'s whole-model
/// publishing, so a new reading redraws the bar and legend and nothing else.
@MainActor
public final class CpuModel: ObservableObject {
    @Published public private(set) var usage: Mason_V1_CpuUsage?
    @Published public private(set) var colors: [UInt64: Int] = [:]
    private var palette = CpuColors()

    public init() {}

    /// Take the CPU data from a home view (nil from a daemon that sends none).
    public func update(_ home: Mason_V1_HomeView?) {
        let u: Mason_V1_CpuUsage? = (home?.hasCpu ?? false) ? home?.cpu : nil
        guard u != usage else { return }
        usage = u
        colors = palette.assign((u?.shares ?? []).filter { $0.kind == "task" }.map(\.task))
    }
}
