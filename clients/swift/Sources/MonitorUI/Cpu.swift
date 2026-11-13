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
    @ObservedObject var cpu: CpuModel
    @State private var expanded = false
    @State private var tip: String?

    var body: some View {
        if let layout = cpuLayout(cpu.usage, nowMs: model.daemonNow(), colors: cpu.colors) {
            CpuSectionBody(model: model, layout: layout, expanded: $expanded, tip: $tip)
        }
    }
}

/// The section for one layout (so a test can draw it without a model's stream).
struct CpuSectionBody: View {
    @Environment(\.theme) var theme
    let model: MonitorModel
    let layout: CpuLayout
    @Binding var expanded: Bool
    @Binding var tip: String?

    var body: some View {
        section("CPU", theme: theme) {
            Text(layout.header).font(theme.body()).foregroundStyle(theme.muted)
            if let hidden = layout.hidden {
                Text(hidden).font(theme.body()).foregroundStyle(theme.muted)
                    .accessibilityIdentifier("cpu-hidden")
            } else {
                bar
                if let tip { Text(tip).font(theme.small()).foregroundStyle(theme.text) }
                ForEach(layout.rows, id: \.label) { legend($0) }
                if !layout.more.isEmpty {
                    Button((expanded ? "Hide: " : "") + layout.moreText) { expanded.toggle() }
                        .buttonStyle(.link).font(theme.body())
                        .accessibilityIdentifier("cpu-more")
                    if expanded { ForEach(layout.more, id: \.label) { legend($0) } }
                }
            }
        }
    }

    private var bar: some View {
        GeometryReader { g in
            HStack(spacing: 0) {
                ForEach(layout.segments, id: \.label) { s in
                    Rectangle().fill(theme.cpu(s.color))
                        .frame(width: g.size.width * CGFloat(s.permille) / 1000)
                        .help(s.tip)
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
                Button(r.name) { model.go(.task(r.task)) }.buttonStyle(.link).font(theme.body(.semibold))
            } else {
                Text(r.name).font(theme.body(.semibold))
            }
            Text(r.title).font(theme.body()).foregroundStyle(theme.muted).lineLimit(1).truncationMode(.tail)
            Spacer(minLength: theme.spacing)
            Text(Format.permille(r.permille)).font(theme.body()).monospacedDigit()
        }
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("cpu-legend:\(r.label)")
    }
}
