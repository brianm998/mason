// SPDX-License-Identifier: GPL-3.0-or-later

import MonitorCore
import SwiftUI

/// One run on a host: the task as a link (a run no task owns shows its label), its pipeline state
/// and what runs, for how long, and the GitHub run's link.
struct HostRunRow: View {
    @Environment(\.theme) var theme
    let model: MonitorModel
    let run: HostRun
    var compact = false

    var body: some View {
        let line = compact
            ? AnyLayout(VStackLayout(alignment: .leading, spacing: 2))
            : AnyLayout(HStackLayout(spacing: theme.spacing))
        line {
            if Hosts.hasLink(run) {
                TaskLink(title: Hosts.label(run), font: theme.body()) { model.go(.task(run.task.id)) }
            } else {
                Text(Hosts.label(run)).font(theme.body(.semibold))
            }
            Text(Hosts.runLine(run, now: model.daemonNow())).font(theme.small()).foregroundStyle(theme.muted)
            if let u = URL(string: run.url), Linkify.isWebURL(run.url) {
                WebLink(title: "open on GitHub", url: u, font: theme.small())
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("hosts:run:\(run.host):\(run.kind):\(run.name)")
    }
}

/// Home's Hosts section (#1156): this Mac, each node, GitHub, and the tasks running on each now.
/// Hidden on a single-machine install with nothing running. On a phone each host is one line with
/// its run count that opens to the rows.
struct HostsSection: View {
    @Environment(\.theme) var theme
    let model: MonitorModel
    var compact = false
    @State private var expanded: Set<String> = []

    var body: some View {
        if let hosts = Hosts.visible(model.home, nodesWorthShowing: model.nodes?.worthShowing ?? false) {
            section("Hosts", theme: theme) {
                ForEach(hosts, id: \.name) { h in
                    if compact { phoneHost(h) } else { host(h) }
                }
            }
            .accessibilityIdentifier("hosts")
        }
    }

    @ViewBuilder private func host(_ h: HostView) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack(spacing: theme.spacing) {
                Text(Hosts.title(h)).font(theme.body(.semibold))
                Text(h.status).font(theme.small()).foregroundStyle(theme.muted)
            }
            if h.runs.isEmpty {
                Text("idle").font(theme.small()).foregroundStyle(theme.muted)
            }
            ForEach(Array(h.runs.enumerated()), id: \.offset) { _, r in
                HostRunRow(model: model, run: r).padding(.leading, theme.spacingLarge)
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("hosts:\(h.kind):\(h.name)")
    }

    @ViewBuilder private func phoneHost(_ h: HostView) -> some View {
        let open = expanded.contains(h.name)
        VStack(alignment: .leading, spacing: 2) {
            Button {
                if open { expanded.remove(h.name) } else { expanded.insert(h.name) }
            } label: {
                HStack(spacing: theme.spacingSmall) {
                    Image(systemName: open ? "chevron.down" : "chevron.right").foregroundStyle(theme.muted)
                    Text(Hosts.compactTitle(h)).font(theme.body(.semibold))
                }
            }
            .buttonStyle(.plain).touchTarget()
            if !h.status.isEmpty {
                Text(h.status).font(theme.small()).foregroundStyle(theme.muted)
            }
            if open {
                ForEach(Array(h.runs.enumerated()), id: \.offset) { _, r in
                    HostRunRow(model: model, run: r, compact: true).padding(.leading, theme.spacingLarge)
                }
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("hosts:\(h.kind):\(h.name)")
    }
}
