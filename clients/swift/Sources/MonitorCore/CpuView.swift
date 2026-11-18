// SPDX-License-Identifier: GPL-3.0-or-later

import Foundation
import MasonProtocol
import Observation

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
    /// The task's project; empty for a fixed bucket.
    public let project: String
    public let permille: UInt32
    public let color: String

    /// "#287 (star)" for a task, the fixed name otherwise: what the legend and tips lead with.
    public var heading: String { project.isEmpty ? name : "\(name) (\(project))" }

    /// "#287 (star) Scheduler: … 23.4%", for a hover or a tap.
    public var tip: String { (title.isEmpty ? heading : "\(heading) \(title)") + " " + Format.permille(permille) }
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
            title: isTask ? s.title : "", project: isTask ? s.project : "", permille: s.permille, color: isTask ? String(colour) : s.kind)
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
@Observable
public final class CpuModel {
    public private(set) var usage: Mason_V1_CpuUsage?
    public private(set) var colors: [UInt64: Int] = [:]
    /// The graph, prepared off the main thread; nil until the first history arrives.
    public private(set) var chart: CpuChart?
    /// The span the graph shows; a change resubscribes (see `Connection.sendSubscription`).
    public private(set) var span: CpuSpan = .hour
    @ObservationIgnored private var palette = CpuColors()
    @ObservationIgnored private var generation = 0

    public init() {}

    public func setSpan(_ s: CpuSpan) {
        guard s != span else { return }
        span = s
        chart = nil
    }

    /// Take a history view. The chart is built on a background task; a newer history
    /// arriving first makes this one's result be dropped, so updates coalesce.
    public func update(history h: Mason_V1_CpuHistory) {
        generation += 1
        let gen = generation
        let usage = usage, colors = colors, span = span
        Task.detached(priority: .utility) {
            let c = cpuChart(h, usage: usage, colors: colors)
            await MainActor.run { [weak self] in
                guard let self, self.generation == gen, self.span == span else { return }
                if self.chart != c { self.chart = c }
            }
        }
    }

    /// Take the CPU data from a home view (nil from a daemon that sends none).
    public func update(_ home: Mason_V1_HomeView?) {
        let u: Mason_V1_CpuUsage? = (home?.hasCpu ?? false) ? home?.cpu : nil
        guard u != usage else { return }
        usage = u
        colors = palette.assign((u?.shares ?? []).filter { $0.kind == "task" }.map(\.task))
    }
}

/// The spans the graph offers.
public enum CpuSpan: CaseIterable, Sendable {
    case fiveMinutes, hour, sixHours, day

    public var ms: UInt64 {
        switch self {
        case .fiveMinutes: return 300_000
        case .hour: return 3_600_000
        case .sixHours: return 21_600_000
        case .day: return 86_400_000
        }
    }
    public var label: String {
        switch self {
        case .fiveMinutes: return "5 min"
        case .hour: return "1 h"
        case .sixHours: return "6 h"
        case .day: return "24 h"
        }
    }
}

/// One series of the graph: a share's permille at each time, in the same colour as its bar segment.
public struct CpuSeries: Equatable, Sendable {
    public let key: String
    public let heading: String
    public let title: String
    public let color: String
    /// One value per entry of `CpuChart.times` (0 where the share was absent).
    public let values: [UInt32]

    /// "#287 (star) Scheduler: … 23.4%" at `index`.
    public func tip(at index: Int) -> String {
        (title.isEmpty ? heading : "\(heading) \(title)") + " " + Format.permille(values[index])
    }
}

/// The CPU history, ready to draw: points in time order and a series per share, biggest total first
/// (that one at the bottom of the stack).
public struct CpuChart: Equatable, Sendable {
    public let spanMs: UInt64
    public let times: [UInt64]
    public let series: [CpuSeries]
    /// The tallest stack, in permille (at least 1).
    public let maxStack: UInt32
}

/// The chart for `h`. Titles and projects come from `usage` (the history carries neither); a task
/// in `colors` keeps the colour its bar segment has, any other the id-based fallback the bar uses.
public func cpuChart(_ h: Mason_V1_CpuHistory, usage: Mason_V1_CpuUsage?, colors: [UInt64: Int]) -> CpuChart {
    var info: [String: Mason_V1_CpuShare] = [:]
    for s in usage?.shares ?? [] { info[s.kind == "task" ? "task:\(s.task)" : s.kind] = s }
    var order: [String] = []
    var seen: [String: Mason_V1_CpuShare] = [:]
    var totals: [String: UInt64] = [:]
    for p in h.points {
        for s in p.shares {
            let k = s.kind == "task" ? "task:\(s.task)" : s.kind
            if seen[k] == nil { order.append(k); seen[k] = s }
            totals[k, default: 0] += UInt64(s.permille)
        }
    }
    order.sort { (totals[$0]!, $1) > (totals[$1]!, $0) }
    var stack = [UInt32](repeating: 0, count: h.points.count)
    let series = order.map { k -> CpuSeries in
        let s = seen[k]!
        let isTask = s.kind == "task"
        let now = info[k]
        let colour = isTask ? String((colors[s.task] ?? Int(s.task % UInt64(Cpu.taskColours))) + 1) : s.kind
        let values = h.points.enumerated().map { (i, p) -> UInt32 in
            let v = p.shares.first { ($0.kind == "task" ? "task:\($0.task)" : $0.kind) == k }?.permille ?? 0
            stack[i] += v
            return v
        }
        let name = isTask ? s.label : fixedName(s.kind)
        let project = isTask ? (now?.project ?? "") : ""
        return CpuSeries(
            key: k, heading: project.isEmpty ? name : "\(name) (\(project))",
            title: isTask ? (now?.title ?? "") : "", color: colour, values: values)
    }
    return CpuChart(spanMs: h.spanMs, times: h.points.map(\.atMs), series: series, maxStack: max(1, stack.max() ?? 1))
}

/// The index of the point nearest to `fraction` (0...1) of the chart's width; nil for an empty chart.
public func cpuChartIndex(_ c: CpuChart, fraction: Double) -> Int? {
    guard let first = c.times.first, let last = c.times.last, c.times.count > 0 else { return nil }
    let at = Double(first) + min(max(fraction, 0), 1) * Double(last - first)
    return c.times.indices.min { abs(Double(c.times[$0]) - at) < abs(Double(c.times[$1]) - at) }
}
