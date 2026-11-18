// SPDX-License-Identifier: GPL-3.0-or-later

import MasonProtocol
import MonitorCore
import SwiftUI

/// Home's CPU section: one bar standing for the whole machine, a segment per share from the
/// left in the daemon's order, and a legend in the same order. It observes only `CpuModel`,
/// so a new reading redraws this section and nothing else. Hovering a segment (Mac) or
/// tapping it shows "#287 Scheduler: … 23.4%".
struct CpuSection: View {
    @Environment(\.theme) var theme
    let model: MonitorModel
    let cpu: CpuModel
    /// Set on home: the title and its '>' open the CPU page.
    var open: (() -> Void)? = nil
    @State private var expanded = false
    @State private var tip: String?

    var body: some View {
        if let layout = cpuLayout(cpu.usage, nowMs: model.daemonNow(), colors: cpu.colors) {
            CpuSectionBody(model: model, layout: layout, chart: cpu.chart, span: cpu.span, open: open, expanded: $expanded, tip: $tip)
        }
    }
}

/// The section for one layout (so a test can draw it without a model's stream).
struct CpuSectionBody: View {
    @Environment(\.theme) var theme
    let model: MonitorModel
    let layout: CpuLayout
    var chart: CpuChart?
    var span: CpuSpan = .hour
    var open: (() -> Void)? = nil
    @Binding var expanded: Bool
    @Binding var tip: String?

    var body: some View {
        section("CPU (all projects)", theme: theme, open: open) {
            Text(layout.header).font(theme.body()).foregroundStyle(theme.muted)
            if let hidden = layout.hidden {
                Text(hidden).font(theme.body()).foregroundStyle(theme.muted)
                    .accessibilityIdentifier("cpu-hidden")
            } else {
                bar
                if let tip { Text(tip).font(theme.small()).foregroundStyle(theme.text) }
                history
                ForEach(layout.rows, id: \.label) { legend($0) }
                if !layout.more.isEmpty {
                    Button((expanded ? "Hide: " : "") + layout.moreText) { expanded.toggle() }
                        .buttonStyle(.plain).foregroundStyle(theme.accent).font(theme.body())
                        .accessibilityIdentifier("cpu-more")
                    if expanded { ForEach(layout.more, id: \.label) { legend($0) } }
                }
            }
        }
    }

    private var history: some View {
        VStack(alignment: .leading, spacing: theme.spacing) {
            Picker("Span", selection: Binding(get: { span }, set: { model.setCpuSpan($0) })) {
                ForEach(CpuSpan.allCases, id: \.self) { Text($0.label).tag($0) }
            }
            .pickerStyle(.segmented).labelsHidden()
            .accessibilityIdentifier("cpu-span")
            if let chart, chart.times.count > 1 {
                CpuChartView(chart: chart)
            } else {
                Text("Collecting CPU history…").font(theme.small()).foregroundStyle(theme.muted)
            }
        }
    }

    private var bar: some View {
        GeometryReader { g in
            HStack(spacing: 0) {
                ForEach(layout.segments, id: \.label) { s in
                    Rectangle().fill(theme.cpu(s.color))
                        .frame(width: g.size.width * CGFloat(s.permille) / 1000)
                        .hint(s.tip)
                        .onTapGesture { tip = tip == s.tip ? nil : s.tip }
                        .accessibilityLabel(s.tip)
                        .accessibilityIdentifier("cpu-segment:\(s.label)")
                }
                Spacer(minLength: 0)
            }
        }
        .frame(height: theme.cell + theme.spacing)
        .background(theme.cpuIdle)
        .clipShape(RoundedRectangle(cornerRadius: theme.radius))
        .accessibilityIdentifier("cpu-bar")
    }

    @ViewBuilder private func legend(_ r: CpuRow) -> some View {
        HStack(spacing: theme.spacing) {
            RoundedRectangle(cornerRadius: theme.radius / 2).fill(theme.cpu(r.color))
                .frame(width: theme.cell, height: theme.cell)
            if r.task > 0 {
                TaskLink(title: r.heading, font: theme.body(.semibold)) { model.go(.task(r.task)) }
            } else {
                Text(r.heading).font(theme.body(.semibold))
            }
            Text(r.title).font(theme.body()).foregroundStyle(theme.muted).lineLimit(1).truncationMode(.tail)
            Spacer(minLength: theme.spacing)
            Text(Format.permille(r.permille)).font(theme.body()).monospacedDigit()
        }
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("cpu-legend:\(r.label)")
    }
}

/// A stacked area chart of the CPU history, one series per share in the bar's colours. The points
/// were prepared off the main thread; drawing only walks them. Hover (Mac) or tap shows the time
/// and the nearest series' value at that x.
struct CpuChartView: View {
    @Environment(\.theme) var theme
    let chart: CpuChart
    @State private var tip: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Canvas { ctx, size in
                let n = chart.times.count
                guard n > 1 else { return }
                let t0 = Double(chart.times[0]), dt = max(1, Double(chart.times[n - 1]) - t0)
                let top = CGFloat(max(1000, chart.maxStack))
                var base = [CGFloat](repeating: 0, count: n)
                for s in chart.series {
                    var path = Path()
                    for i in 0..<n {
                        let x = size.width * CGFloat((Double(chart.times[i]) - t0) / dt)
                        path.addLine(to: CGPoint(x: x, y: size.height * (1 - (base[i] + CGFloat(s.values[i])) / top)))
                        if i == 0 { path = Path(); path.move(to: CGPoint(x: x, y: size.height * (1 - (base[i] + CGFloat(s.values[i])) / top))) }
                    }
                    for i in stride(from: n - 1, through: 0, by: -1) {
                        let x = size.width * CGFloat((Double(chart.times[i]) - t0) / dt)
                        path.addLine(to: CGPoint(x: x, y: size.height * (1 - base[i] / top)))
                        base[i] += CGFloat(s.values[i])
                    }
                    path.closeSubpath()
                    ctx.fill(path, with: .color(theme.cpu(s.color)))
                }
            }
            .frame(height: theme.cell * 5)
            .background(theme.cpuIdle)
            .clipShape(RoundedRectangle(cornerRadius: theme.radius))
            .overlay(GeometryReader { g in
                Color.clear.contentShape(Rectangle())
                    .onContinuousHover { phase in
                        if case .active(let p) = phase { tip = describe(x: p.x, y: p.y, size: g.size) } else { tip = nil }
                    }
                    .gesture(DragGesture(minimumDistance: 0).onEnded { tip = describe(x: $0.location.x, y: $0.location.y, size: g.size) })
            })
            .accessibilityIdentifier("cpu-chart")
            if let tip { Text(tip).font(theme.small()).foregroundStyle(theme.text) }
        }
    }

    private func describe(x: CGFloat, y: CGFloat, size: CGSize) -> String? {
        guard let i = cpuChartIndex(chart, fraction: Double(x / max(1, size.width))) else { return nil }
        let level = (1 - Double(y / max(1, size.height))) * Double(max(1000, chart.maxStack))
        var acc = 0.0
        var hit = chart.series.last
        for s in chart.series {
            acc += Double(s.values[i])
            if level <= acc { hit = s; break }
        }
        let time = Date(timeIntervalSince1970: Double(chart.times[i]) / 1000).formatted(date: .omitted, time: .standard)
        return hit.map { "\(time) \($0.tip(at: i))" } ?? time
    }
}
