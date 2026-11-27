// SPDX-License-Identifier: GPL-3.0-or-later

import Foundation
import MasonProtocol
import Observation

/// The home page's disk section, as data: one line per volume of the daemon's `DiskHistory`
/// (free bytes over time, y from 0 to the largest total shown). The same rules are in the
/// Compose client's `DiskView.kt`.
public enum Disk {
    /// A volume's colour is one of the theme's ten task colours.
    public static let colours = Cpu.taskColours
    /// Home puts CPU and disk side by side from this width (points); narrower, they stack.
    public static let twoColumnMinWidth: Double = 900
    /// The CPU section's share of the row against the disk section's (about 3:2).
    public static let cpuWeight: Double = 3
    public static let diskWeight: Double = 2
    /// The preference holding the hidden volumes' ids, one per line.
    public static let hiddenKey = "disk.hidden"
}

extension Format {
    /// Bytes in decimal units like Finder: "412 GB", "1.8 TB". One decimal below 10 of a unit,
    /// none from 10 up; a value that rounds to 1000 moves up a unit.
    public static func bytes(_ n: UInt64) -> String {
        let units = ["B", "KB", "MB", "GB", "TB", "PB"]
        var v = Double(n)
        var u = 0
        while u < units.count - 1 {
            let shown = v < 10 ? (v * 10).rounded(.toNearestOrAwayFromZero) / 10 : v.rounded(.toNearestOrAwayFromZero)
            if v < 1000 && shown < 1000 { break }
            v /= 1000
            u += 1
        }
        if u == 0 { return "\(Int(v)) B" }
        return v < 10 ? String(format: "%.1f %@", v, units[u]) : "\(Int(v.rounded(.toNearestOrAwayFromZero))) \(units[u])"
    }

    /// "412 GB free of 1.8 TB".
    public static func freeOf(_ free: UInt64, _ total: UInt64) -> String { "\(bytes(free)) free of \(bytes(total))" }
}

/// The disk graph offers the same four spans as the CPU graph.
public typealias DiskSpan = CpuSpan

/// Keeps a volume's colour while it is listed, so hiding another volume or a reorder does not
/// recolour it. A new volume takes the lowest slot nobody holds; one that vanishes frees its slot.
public struct DiskColors: Sendable {
    private var held: [String: Int] = [:]
    public init() {}

    public mutating func assign(_ ids: [String]) -> [String: Int] {
        let present = Set(ids)
        held = held.filter { present.contains($0.key) }
        for id in ids where held[id] == nil {
            let used = Set(held.values)
            held[id] = (0..<Disk.colours).first { !used.contains($0) } ?? (held.count % Disk.colours)
        }
        return held
    }
}

/// One volume's line: free bytes at each time of `DiskChart.times`, nil where the volume was not read.
public struct DiskSeries: Equatable, Sendable {
    public let id: String
    public let name: String
    public let mount: String
    public let roles: [String]
    /// A theme colour key, "1"..."10" (what `Theme.cpu` takes).
    public let color: String
    public let free: [UInt64?]
    public let total: [UInt64?]

    /// "name: 412 GB free of 1.8 TB" at `index`; "name: not read" for a gap.
    public func tip(at index: Int) -> String {
        guard let f = free[index], let t = total[index] else { return "\(name): not read" }
        return "\(name): \(Format.freeOf(f, t))"
    }
}

/// The history ready to draw: `maxTotal` is the top of the y scale (never 0).
public struct DiskChart: Equatable, Sendable {
    public let spanMs: UInt64
    public let times: [UInt64]
    public let series: [DiskSeries]
    public let maxTotal: UInt64
}

/// The chart for `h` without the volumes in `hidden`: a series per shown volume, in the daemon's
/// order. `colors` (volume id to slot) is what the model keeps stable; without it the slots go by
/// the daemon's order of all volumes, hidden ones included, so hiding never recolours.
public func diskChart(_ h: Mason_V1_DiskHistory, hidden: Set<String>, colors: [String: Int]? = nil) -> DiskChart {
    var fallback = DiskColors()
    let slots = colors ?? fallback.assign(h.volumes.map(\.id))
    let series = h.volumes.filter { !hidden.contains($0.id) }.map { v -> DiskSeries in
        let readings = h.points.map { p in p.readings.first { $0.volume == v.id } }
        return DiskSeries(
            id: v.id, name: v.name, mount: v.mount, roles: v.roles, color: String((slots[v.id] ?? 0) + 1),
            free: readings.map { $0?.freeBytes }, total: readings.map { $0?.totalBytes })
    }
    let top = series.flatMap { $0.total.compactMap { $0 } }.max() ?? 1
    return DiskChart(spanMs: h.spanMs, times: h.points.map(\.atMs), series: series, maxTotal: max(1, top))
}

/// The index of the point nearest to `fraction` (0...1) of the chart's width; nil for an empty chart.
public func diskChartIndex(_ c: DiskChart, fraction: Double) -> Int? {
    guard let first = c.times.first, let last = c.times.last else { return nil }
    let at = Double(first) + min(max(fraction, 0), 1) * (Double(last) - Double(first))
    return c.times.indices.min { abs(Double(c.times[$0]) - at) < abs(Double(c.times[$1]) - at) }
}

/// The newest free and total bytes of volume `id` in `h`; nil if it was never read.
public func diskLatest(_ h: Mason_V1_DiskHistory, id: String) -> (free: UInt64, total: UInt64)? {
    for p in h.points.reversed() {
        if let r = p.readings.first(where: { $0.volume == id }) { return (r.freeBytes, r.totalBytes) }
    }
    return nil
}

/// The legend's tip for a volume: its mount and what lives on it, "/Volumes/sp: data, cache".
public func diskVolumeTip(mount: String, roles: [String]) -> String {
    let what = roles.map { $0 == "project" ? "projects" : $0 }.joined(separator: ", ")
    return what.isEmpty ? mount : "\(mount): \(what)"
}

/// The hidden volumes as the preference stores them, one id per line.
public func encodeHiddenVolumes(_ ids: Set<String>) -> String { ids.sorted().joined(separator: "\n") }

public func decodeHiddenVolumes(_ s: String?) -> Set<String> {
    Set((s ?? "").split(separator: "\n").map(String.init))
}

/// `hidden` without the ids `h` no longer lists.
public func pruneHiddenVolumes(_ hidden: Set<String>, _ h: Mason_V1_DiskHistory) -> Set<String> {
    hidden.intersection(h.volumes.map(\.id))
}

/// What the disk section observes, apart from `MonitorModel`'s whole-model publishing.
@MainActor
@Observable
public final class DiskModel {
    public private(set) var history: Mason_V1_DiskHistory?
    /// True once the daemon has sent any disk history; an older daemon never does, which hides the section.
    public private(set) var seen = false
    public private(set) var span: DiskSpan = .hour
    public private(set) var hidden: Set<String>
    public private(set) var colors: [String: Int] = [:]
    @ObservationIgnored private var palette = DiskColors()
    @ObservationIgnored private let prefs: Preferences

    public init(preferences: Preferences = MemoryPreferences()) {
        prefs = preferences
        hidden = decodeHiddenVolumes(preferences.string(Disk.hiddenKey))
    }

    public func setSpan(_ s: DiskSpan) {
        guard s != span else { return }
        span = s
        history = nil
    }

    /// Show or hide a volume's line; remembered.
    public func toggleVolume(_ id: String) {
        if hidden.contains(id) { hidden.remove(id) } else { hidden.insert(id) }
        prefs.setString(encodeHiddenVolumes(hidden), for: Disk.hiddenKey)
    }

    public func update(history h: Mason_V1_DiskHistory) {
        history = h
        seen = true
        colors = palette.assign(h.volumes.map(\.id))
        if !h.volumes.isEmpty {
            let kept = pruneHiddenVolumes(hidden, h)
            if kept != hidden {
                hidden = kept
                prefs.setString(encodeHiddenVolumes(kept), for: Disk.hiddenKey)
            }
        }
    }

    public var chart: DiskChart? { history.map { diskChart($0, hidden: hidden, colors: colors) } }
}

/// What the pipeline's worktrees take, the same totals as `mason disk`: a line for all of it,
/// then one per kind and task state. Empty until the daemon has measured them.
public func diskWorktreeLines(_ h: Mason_V1_DiskHistory) -> [String] {
    guard !h.worktrees.isEmpty else { return [] }
    let all = h.worktrees.reduce(UInt64(0)) { $0 + $1.bytes }
    let next = h.worktrees.reduce(UInt64(0)) { $0 + $1.freeNextBytes }
    return ["Worktrees \(Format.bytes(all)) · next sweep frees \(Format.bytes(next))"]
        + h.worktrees.map { "\($0.kind), \($0.state): \($0.count), \(Format.bytes($0.bytes))" }
}
