// SPDX-License-Identifier: GPL-3.0-or-later

import Foundation
import MasonProtocol

public typealias ProgressCell = Mason_V1_ProgressCell
public typealias ProgressSource = Mason_V1_ProgressSource

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

    /// 1240 as "1,240" (always a comma, whatever the locale: the same text in every client).
    public static func group(_ n: UInt64) -> String {
        let s = String(n)
        var out = ""
        for (i, c) in s.enumerated() {
            if i > 0 && (s.count - i) % 3 == 0 { out.append(",") }
            out.append(c)
        }
        return out
    }

    /// A token count in few characters: "988", "12.2k", "1.2M". Whole-number arithmetic, so it
    /// rounds exactly as the daemon's own reading (`mason_core::progress_text::compact`) does.
    public static func compactCount(_ n: UInt64) -> String {
        if n < 1000 { return String(n) }
        let k = (n + 50) / 100
        if k < 10_000 { return "\(k / 10).\(k % 10)k" }
        let m = (n + 50_000) / 100_000
        return "\(m / 10).\(m % 10)M"
    }

    /// "12.2k in, 988 out": "in" is input plus cache read plus cache write; a side nothing was
    /// reported for is left out. Empty when `tokens` is absent or reports nothing.
    public static func tokensCompact(_ t: Mason_V1_TokenLine?) -> String {
        guard let t else { return "" }
        var parts: [String] = []
        if t.hasInput_p || t.hasCacheRead_p || t.hasCacheWrite_p {
            parts.append("\(compactCount(t.input + t.cacheRead + t.cacheWrite)) in")
        }
        if t.hasOutput_p { parts.append("\(compactCount(t.output)) out") }
        return parts.joined(separator: ", ")
    }

    /// Each reported figure in full: "input 4", "output 988", "cache read 7,664", "cache write
    /// 4,503", and "cost $0.0123" when there is one.
    /// A CI workflow's jobs on one line: "macOS: 4 jobs · 1 queued · 1 running (build) · 2 passed".
    public static func jobsText(name: String, jobs j: Mason_V1_JobCounts) -> String {
        let total = j.queued + j.running + j.succeeded + j.failed
        var parts = ["\(total) \(total == 1 ? "job" : "jobs")"]
        if j.queued > 0 { parts.append("\(j.queued) queued") }
        if j.running > 0 {
            var r = "\(j.running) running"
            if !j.runningNames.isEmpty { r += " (\(j.runningNames.joined(separator: ", ")))" }
            parts.append(r)
        }
        if j.succeeded > 0 { parts.append("\(j.succeeded) passed") }
        if j.failed > 0 { parts.append("\(j.failed) failed") }
        return "\(name): " + parts.joined(separator: " · ")
    }

    public static func tokensDetail(_ t: Mason_V1_TokenLine?) -> [String] {
        guard let t else { return [] }
        var v: [String] = []
        if t.hasInput_p { v.append("input \(group(t.input))") }
        if t.hasOutput_p { v.append("output \(group(t.output))") }
        if t.hasCacheRead_p { v.append("cache read \(group(t.cacheRead))") }
        if t.hasCacheWrite_p { v.append("cache write \(group(t.cacheWrite))") }
        if t.hasCost_p { v.append("cost $" + String(format: "%.4f", t.costUsd)) }
        return v
    }

    /// The whole percent `done` is of a real `total`: rounded down, at most 100, none for 0.
    /// Only real totals are ever passed in.
    public static func percent(done: UInt64, total: UInt64) -> Int? {
        if total == 0 { return nil }
        return Int(min(done, total) * 100 / total)
    }

    /// What a count and its total say, by the rule for numbers: "212 of 340 tests, 62%" for a real
    /// total; "1,240 lines, usually 1,800 to 2,400" for a typical one (never a percentage);
    /// "37 tool calls" for none. Empty when there is nothing to say yet.
    public static func countText(
        unit: String, done: UInt64, doneSource: ProgressSource,
        total: UInt64, totalSource: ProgressSource, totalLow: UInt64
    ) -> String {
        let u = unit.isEmpty ? "" : " \(unit)"
        switch totalSource {
        case .sourceReported, .sourceCounted, .sourceExpected, .sourceMinimum:
            if total > 0 {
                return "\(group(done)) of \(group(total))\(u), \(percent(done: done, total: total) ?? 0)%"
            }
        case .sourceHistory:
            if doneSource != .sourceNone {
                let range = totalLow > 0 && totalLow < total ? "\(group(totalLow)) to \(group(total))" : group(total)
                return "\(group(done))\(u), usually \(range)"
            }
        default: break
        }
        return doneSource != .sourceNone ? "\(group(done))\(u)" : ""
    }

    /// A real total (reported, counted, the last green run's count, or a suite's minimum), not a typical one.
    public static func isReal(_ s: ProgressSource) -> Bool {
        switch s {
        case .sourceReported, .sourceCounted, .sourceExpected, .sourceMinimum: return true
        default: return false
        }
    }

    /// Where a figure came from, in words.
    public static func sourceWords(_ s: ProgressSource, samples: UInt32) -> String {
        switch s {
        case .sourceReported: return "from the agent"
        case .sourceCounted: return "counted from output"
        case .sourceHistory: return "usually, from \(samples) past runs"
        case .sourceExpected: return "the last green run's count"
        case .sourceMinimum: return "the suite's minimum"
        default: return ""
        }
    }

    /// Quiet past `afterMs`: nothing has moved for longer than the step's usual gap.
    public static func isStale(now: UInt64, moved: UInt64, afterMs: UInt64) -> Bool {
        moved > 0 && (now > moved ? now - moved : 0) > afterMs
    }

    /// "updated 4 s ago", or "no output for 6 min" when stale. Empty when nothing moved yet.
    public static func timeText(now: UInt64, moved: UInt64, afterMs: UInt64) -> String {
        if moved == 0 { return "" }
        let span = duration(ms: now > moved ? now - moved : 0)
        return isStale(now: now, moved: moved, afterMs: afterMs) ? "no output for \(span)" : "updated \(span) ago"
    }
}

/// How a running cell is filled.
public enum CellFill: Equatable, Sendable {
    /// A real total: filled in proportion (0...1).
    case proportional(Double)
    /// Only a typical total: filled the same way, lighter.
    case typical(Double)
    /// Nothing to measure against: the outline alone.
    case outline
}

extension Mason_V1_ProgressCell {
    public var hasRealTotal: Bool {
        Format.isReal(totalSource) && total > 0
    }

    /// The whole percent, only from a real total.
    public var percent: Int? {
        hasRealTotal ? Format.percent(done: UInt64(done), total: UInt64(total)) : nil
    }

    public var countText: String {
        Format.countText(
            unit: unit, done: UInt64(done), doneSource: doneSource,
            total: UInt64(total), totalSource: totalSource, totalLow: UInt64(totalLow)
        )
    }

    public var fill: CellFill {
        if hasRealTotal { return .proportional(Double(min(done, total)) / Double(total)) }
        if totalSource == .sourceHistory && total > 0 { return .typical(min(1, Double(done) / Double(total))) }
        return .outline
    }

    public func isStale(now: UInt64) -> Bool {
        Format.isStale(now: now, moved: updatedAtMs, afterMs: staleAfterMs)
    }

    public func timeText(now: UInt64) -> String {
        Format.timeText(now: now, moved: updatedAtMs, afterMs: staleAfterMs)
    }

    /// The tokens the step's LLM sessions have used so far, "12.2k in, 988 out"; empty when none
    /// were reported.
    public var tokensCompact: String { Format.tokensCompact(hasTokens ? tokens : nil) }

    /// The cell's accessibility label: "develop, running, 3 of 7 plan steps, 42%" spoken as
    /// "42 percent". The ticking time is deliberately not in it.
    public var spokenLabel: String {
        let state: String
        switch self.state {
        case .filled: state = "finished"
        case .part: state = "running"
        case .hatched: state = "waiting"
        default: state = "not started"
        }
        var s = "\(step), \(state)"
        if self.state == .part {
            let c = countText
            if !c.isEmpty { s += ", " + (percent.map { c.replacingOccurrences(of: ", \($0)%", with: ", \($0) percent") } ?? c) }
        }
        return s
    }

    /// Everything a running step's progress says, without any clock text: what the conformance
    /// run compares between the clients and the daemon's own reading.
    public func progressTexts(now: UInt64) -> [String] {
        var t: [String] = []
        let c = countText
        if !c.isEmpty { t.append(c) }
        if !message.isEmpty { t.append(message) }
        if doneSource != .sourceNone { t.append("count " + Format.sourceWords(doneSource, samples: historySamples)) }
        if totalSource != .sourceNone { t.append("total " + Format.sourceWords(totalSource, samples: historySamples)) }
        if lines > 0 || bytes > 0 { t.append("output \(Format.group(lines)) lines, \(Format.group(bytes)) bytes") }
        if hasTokens {
            let c = tokensCompact
            if !c.isEmpty { t.append("tokens " + c) }
            t.append(contentsOf: Format.tokensDetail(tokens))
        }
        for s in subs {
            let text = Format.countText(
                unit: s.unit, done: UInt64(s.done), doneSource: s.doneSource,
                total: UInt64(s.total), totalSource: s.totalSource, totalLow: 0
            )
            t.append("\(s.name): \(text)")
        }
        for s in subs where s.hasJobs { t.append(Format.jobsText(name: s.name, jobs: s.jobs)) }
        for i in todo { t.append("\(i.status): \(i.text)") }
        if isStale(now: now) { t.append("silent") }
        return t
    }
}

public extension Mason_V1_Compilation {
    /// The CPU line of the home page's Compilation section.
    var cpuLine: String {
        let gate = "a further compile starts below \(cpuMaxPct)%"
        return cpuReadingLine(busy: cpuBusyPct < 0 ? nil : Int(cpuBusyPct), source: cpuSource, gate: gate)
    }
}

/// The CPU line shared by the Health page and the home page. A fallback source is named,
/// so a held compile is not read as a broken gauge.
public func cpuReadingLine(busy: Int?, source: String, gate: String) -> String {
    guard let b = busy else { return "CPU not readable; \(gate)." }
    switch source {
    case "loadavg": return "CPU \(b)% busy (load average; the CPU reading is missing); \(gate)."
    case "assumed-busy": return "CPU assumed \(b)% busy (nothing is readable, so new compiles wait); \(gate)."
    default: return "CPU \(b)% busy; \(gate)."
    }
}
