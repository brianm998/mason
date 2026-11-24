// SPDX-License-Identifier: GPL-3.0-or-later

import MonitorCore
import SwiftUI

/// "Accept work from other Macs": the owner's switch on a Mac that serves as a node. It is the
/// working-hours control for a work machine, so it is one click and always says what the node is
/// doing. Shown only where `[node] serve = true`.
struct NodeAcceptToggle: View {
    @Environment(\.theme) var theme
    let model: MonitorModel

    var body: some View {
        if let n = model.nodeSelf, n.serving {
            VStack(alignment: .leading, spacing: theme.spacingSmall) {
                Toggle("Accept work from other Macs", isOn: Binding(
                    get: { n.switchOn },
                    set: { model.setNodeAccept($0) }
                ))
                .font(theme.body(.semibold))
                .accessibilityIdentifier("node:accept")
                Text(n.status).font(theme.small()).foregroundStyle(theme.muted)
                    .accessibilityIdentifier("node:accept-status")
            }
        }
    }
}

/// Home's Nodes section on a coordinator: the nodes it found and is paired with, whether each takes
/// work, and the switch that lets runs go to them at all. Hidden until there is a node or nodes
/// are turned on. Refreshed every few seconds while it is on screen, by whoever shows it.
struct NodesSection: View {
    @Environment(\.theme) var theme
    let model: MonitorModel

    var body: some View {
        if let s = model.nodes, s.worthShowing {
            section("Nodes", theme: theme) {
                Toggle("Send runs to nodes", isOn: Binding(
                    get: { s.enabled },
                    set: { model.setNodesUse($0) }
                ))
                .font(theme.body())
                .accessibilityIdentifier("nodes:use")
                Text(s.header).font(theme.body()).foregroundStyle(theme.muted)
                    .accessibilityIdentifier("nodes:header")
                ForEach(s.nodes) { n in
                    VStack(alignment: .leading, spacing: 2) {
                        HStack(spacing: theme.spacing) {
                            Text(n.name).font(theme.body(.semibold))
                            Text(n.state).font(theme.body())
                                .foregroundStyle(n.usable ? theme.text : theme.muted)
                        }
                        if !n.load.isEmpty {
                            Text(n.load).font(theme.small()).foregroundStyle(theme.muted)
                        }
                    }
                    .accessibilityElement(children: .contain)
                    .accessibilityIdentifier("nodes:\(n.id)")
                }
            }
        }
    }
}

/// Asks for the node views now and every ten seconds while the view it is attached to is on
/// screen. Home and Settings attach it; the daemon's routes are cheap.
struct NodesRefresh: ViewModifier {
    let model: MonitorModel

    func body(content: Content) -> some View {
        content.task {
            while !Task.isCancelled {
                model.loadNodes()
                try? await Task.sleep(nanoseconds: 10_000_000_000)
            }
        }
    }
}

/// Nothing to see: it keeps the node views fresh while the page that holds it is on screen.
struct NodesPoller: View {
    let model: MonitorModel
    var body: some View {
        Color.clear.frame(width: 0, height: 0).modifier(NodesRefresh(model: model))
    }
}
