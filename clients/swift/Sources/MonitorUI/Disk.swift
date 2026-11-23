// SPDX-License-Identifier: GPL-3.0-or-later

import MasonProtocol
import MonitorCore
import SwiftUI

/// Home's disk section: the newest free space of each shown volume, a span picker, a line chart of
/// free space per volume (y from 0 to the largest total shown) and a legend with a checkbox per
/// volume when there is more than one. Hidden entirely on a daemon that sends no disk history.
struct DiskSection: View {
    let model: MonitorModel
    let disk: DiskModel
    /// Set on home: the title and its '>' open the Disk page.
    var open: (() -> Void)? = nil

    var body: some View {
        if disk.seen {
            DiskSectionBody(
                history: disk.history, chart: disk.chart, span: disk.span, hidden: disk.hidden, colors: disk.colors,
                open: open, setSpan: { model.setDiskSpan($0) }, toggle: { model.toggleVolume($0) })
        }
    }
}

/// The section for one history (so a test can draw it without a model's stream).
struct DiskSectionBody: View {
    @Environment(\.theme) var theme
    let history: Mason_V1_DiskHistory?
    let chart: DiskChart?
    var span: DiskSpan = .hour
    var hidden: Set<String> = []
    var colors: [String: Int] = [:]
    var open: (() -> Void)? = nil
    var setSpan: (DiskSpan) -> Void = { _ in }
    var toggle: (String) -> Void = { _ in }

    var body: some View {
        let volumes = history?.volumes ?? []
        let shown = volumes.filter { !hidden.contains($0.id) }
        section("Disk (all projects)", theme: theme, open: open) {
            let header = shown.compactMap { v in
                history.flatMap { diskLatest($0, id: v.id) }.map { "\(v.name) \(Format.freeOf($0.free, $0.total))" }
            }
            if !header.isEmpty {
                Text(header.joined(separator: " · ")).font(theme.body()).foregroundStyle(theme.muted)
                    .accessibilityIdentifier("disk-header")
            }
            HStack(spacing: theme.spacing) {
                ForEach(DiskSpan.allCases, id: \.self) { sp in
                    Button { setSpan(sp) } label: {
                        Text(sp.label).font(theme.body(sp == span ? .bold : .regular))
                            .foregroundStyle(sp == span ? theme.text : theme.accent)
                    }
                    .buttonStyle(.plain).touchTarget()
                    .accessibilityIdentifier("disk-span-\(sp.label)")
                }
            }
            if !volumes.isEmpty && shown.isEmpty {
                Text("No volume selected.").font(theme.small()).foregroundStyle(theme.muted)
                    .accessibilityIdentifier("disk-none")
            } else if let chart, chart.times.count > 1, chart.spanMs == span.ms {
                DiskChartView(chart: chart)
            } else {
                Text("Collecting disk history…").font(theme.small()).foregroundStyle(theme.muted)
                    .accessibilityIdentifier("disk-collecting")
            }
            ForEach(volumes, id: \.id) { v in legend(v, Self.hasCheckboxes(volumeCount: volumes.count)) }
        }
    }

    /// A volume's legend row has a show/hide checkbox only when there is more than one volume.
    static func hasCheckboxes(volumeCount: Int) -> Bool { volumeCount > 1 }

    @ViewBuilder private func legend(_ v: Mason_V1_DiskVolume, _ checkable: Bool) -> some View {
        let latest = history.flatMap { diskLatest($0, id: v.id) }
        HStack(spacing: theme.spacing) {
            if checkable {
                Toggle("", isOn: Binding(get: { !hidden.contains(v.id) }, set: { _ in toggle(v.id) }))
                    .labelsHidden()
                    #if os(macOS)
                    .toggleStyle(.checkbox)
                    #endif
                    .accessibilityLabel("Show \(v.name)")
                    .accessibilityIdentifier("disk-check-\(v.name)")
            }
            RoundedRectangle(cornerRadius: theme.radius / 2).fill(theme.cpu(String((colors[v.id] ?? 0) + 1)))
                .frame(width: theme.cell, height: theme.cell)
            Text(v.name).font(theme.body(.semibold)).lineLimit(1).truncationMode(.tail)
                .hint(diskVolumeTip(mount: v.mount, roles: v.roles))
            Spacer(minLength: theme.spacing)
            Text(latest.map { Format.freeOf($0.free, $0.total) } ?? "not read")
                .font(theme.body()).foregroundStyle(theme.muted).lineLimit(1).truncationMode(.tail)
        }
        .accessibilityElement(children: checkable ? .contain : .combine)
        .accessibilityIdentifier("disk-legend-\(v.name)")
    }
}

/// A line chart of free space per volume. A volume absent at a point leaves a gap. Hover (Mac) or
/// tap shows the time and the nearest volume's "name: 412 GB free of 1.8 TB" at that x.
struct DiskChartView: View {
    @Environment(\.theme) var theme
    let chart: DiskChart
    @State private var tip: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Canvas { ctx, size in
                let n = chart.times.count
                guard n > 1 else { return }
                let t0 = Double(chart.times[0]), dt = max(1, Double(chart.times[n - 1]) - t0)
                for s in chart.series {
                    var path = Path()
                    var pen = false
                    for i in 0..<n {
                        guard let f = s.free[i] else { pen = false; continue }
                        let p = CGPoint(
                            x: size.width * CGFloat((Double(chart.times[i]) - t0) / dt),
                            y: size.height * (1 - CGFloat(f) / CGFloat(chart.maxTotal)))
                        if pen { path.addLine(to: p) } else { path.move(to: p) }
                        pen = true
                    }
                    ctx.stroke(path, with: .color(theme.cpu(s.color)), lineWidth: 2 * theme.scale)
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
            .accessibilityIdentifier("disk-chart")
            if let tip { Text(tip).font(theme.small()).foregroundStyle(theme.text).accessibilityIdentifier("disk-tip") }
        }
    }

    private func describe(x: CGFloat, y: CGFloat, size: CGSize) -> String? {
        guard let i = diskChartIndex(chart, fraction: Double(x / max(1, size.width))) else { return nil }
        let level = (1 - Double(y / max(1, size.height))) * Double(chart.maxTotal)
        let hit = chart.series.filter { $0.free[i] != nil }.min { abs(Double($0.free[i]!) - level) < abs(Double($1.free[i]!) - level) }
        let time = Date(timeIntervalSince1970: Double(chart.times[i]) / 1000).formatted(date: .omitted, time: .standard)
        return hit.map { "\(time) \($0.tip(at: i))" } ?? time
    }
}

/// CPU and disk on one row: side by side at about 3:2 when the width allows (and `stacked` is
/// false), else stacked with CPU first. Decided from the width offered, so an iPad in a narrow
/// split and an iPhone both stack.
struct CpuDiskRow: Layout {
    var stacked = false
    var spacing: CGFloat = 16

    private func sideBySide(_ width: CGFloat?) -> Bool {
        !stacked && (width ?? 0) >= CGFloat(Disk.twoColumnMinWidth)
    }

    /// s1027: the layout arithmetic as a pure function, so tests assert exact frames.
    /// Side by side only when not stacked and `width >= Disk.twoColumnMinWidth`, CPU taking 3:2 of
    /// the width minus `spacing`; otherwise the disk sits below the CPU at full width.
    static func placement(width: CGFloat, heights: (CGFloat, CGFloat), stacked: Bool, spacing: CGFloat)
        -> (cpu: CGRect, disk: CGRect)
    {
        if !stacked && width >= CGFloat(Disk.twoColumnMinWidth) {
            let room = max(0, width - spacing)
            let a = room * CGFloat(Disk.cpuWeight / (Disk.cpuWeight + Disk.diskWeight))
            return (
                CGRect(x: 0, y: 0, width: a, height: heights.0),
                CGRect(x: a + spacing, y: 0, width: room - a, height: heights.1)
            )
        }
        return (
            CGRect(x: 0, y: 0, width: width, height: heights.0),
            CGRect(x: 0, y: heights.0 + spacing, width: width, height: heights.1)
        )
    }

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let w = proposal.width ?? 0
        guard subviews.count == 2 else { return subviews.first?.sizeThatFits(proposal) ?? .zero }
        let (hs, _) = heights(w, subviews)
        let f = Self.placement(width: w, heights: hs, stacked: stacked, spacing: spacing)
        return CGSize(width: w, height: max(f.cpu.maxY, f.disk.maxY))
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        guard subviews.count == 2 else { subviews.first?.place(at: bounds.origin, proposal: .init(bounds.size)); return }
        let (hs, _) = heights(bounds.width, subviews)
        let f = Self.placement(width: bounds.width, heights: hs, stacked: stacked, spacing: spacing)
        for (view, r) in zip(subviews, [f.cpu, f.disk]) {
            view.place(
                at: CGPoint(x: bounds.minX + r.minX, y: bounds.minY + r.minY), anchor: .topLeading,
                proposal: .init(width: r.width, height: nil))
        }
    }

    /// Each child's height at the width it will get; the width split depends only on `width`.
    private func heights(_ width: CGFloat, _ subviews: Subviews) -> ((CGFloat, CGFloat), Bool) {
        let probe = Self.placement(width: width, heights: (0, 0), stacked: stacked, spacing: spacing)
        let ha = subviews[0].sizeThatFits(.init(width: probe.cpu.width, height: nil)).height
        let hb = subviews[1].sizeThatFits(.init(width: probe.disk.width, height: nil)).height
        return ((ha, hb), sideBySide(width))
    }
}
