// SPDX-License-Identifier: GPL-3.0-or-later

import MonitorCore
import SwiftUI

/// The sidebar, on every page. Pinned above the list: a Home row and search. In the
/// scrolling part: one collapsible section per state (the daemon's order and sorting),
/// then the other pages. `width` is the drawn width, already fitted to the window.
struct SidebarPane: View {
    @Environment(\.theme) var theme
    @ObservedObject var model: MonitorModel
    let width: CGFloat

    var body: some View {
        VStack(spacing: 0) {
            VStack(alignment: .leading, spacing: 0) {
                homeRow
                SearchBox(model: model).padding(theme.spacing)
            }
            Divider()
            ScrollViewReader { proxy in
                ScrollView {
                    VStack(alignment: .leading, spacing: 0) {
                        ForEach(Array(model.sidebarEntries.enumerated()), id: \.offset) { _, e in
                            entry(e)
                        }
                    }
                }
                .onChange(of: model.scrollTarget) { _, target in
                    guard let t = target else { return }
                    model.scrollTarget = nil
                    // Next turn: the expanded section's rows are laid out by then. The pinned
                    // area is outside the ScrollView, so the top is below it.
                    DispatchQueue.main.async { withAnimation { proxy.scrollTo(t, anchor: .top) } }
                }
            }
        }
        .frame(width: width)
        .background(theme.surface.opacity(0.6))
    }

    @ViewBuilder func entry(_ e: SidebarEntry) -> some View {
        switch e {
        case .header(let sec): SectionHeader(model: model, section: sec).id(sec.state)
        case .row(let r): RowView(model: model, row: r)
        case .rule: Divider().padding(.vertical, theme.spacing)
        case .pageLink(let title, let page):
            pageLink(title, icon: Self.icon(page), page: page)
        }
    }

    static func icon(_ p: Page) -> String {
        switch p {
        case .landing: return "arrow.down.to.line"
        case .usage: return "gauge"
        case .pipeline: return "point.3.connected.trianglepath.dotted"
        default: return "heart.text.square"
        }
    }

    var homeRow: some View {
        Button { model.goHome() } label: {
            Label("Home", systemImage: "house")
                .font(theme.body(.semibold))
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(theme.spacing)
                .background(model.page == .home ? theme.accent.opacity(0.18) : Color.clear)
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier("sidebar:home")
    }

    func pageLink(_ title: String, icon: String, page: Page) -> some View {
        Button { model.go(page) } label: {
            Label(title, systemImage: icon)
                .font(theme.body())
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, theme.spacing).padding(.vertical, theme.spacingSmall + 2)
                .background(model.page == page ? theme.accent.opacity(0.18) : Color.clear)
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier("sidebar:\(title.lowercased().replacingOccurrences(of: " ", with: "-"))")
    }
}

struct SearchBox: View {
    @Environment(\.theme) var theme
    @ObservedObject var model: MonitorModel

    var body: some View {
        VStack(alignment: .leading, spacing: theme.spacingSmall) {
            HStack {
                Image(systemName: "magnifyingglass").foregroundStyle(theme.muted)
                TextField("Search: name, title, body", text: $model.search.text)
                    .textFieldStyle(.plain)
                    .font(theme.body())
                    .accessibilityIdentifier("search")
            }
            .padding(theme.spacingSmall + 2)
            .background(RoundedRectangle(cornerRadius: theme.radius).fill(theme.background))
            .overlay(RoundedRectangle(cornerRadius: theme.radius).stroke(theme.border))
            HStack(spacing: theme.spacing) {
                Toggle("name", isOn: $model.search.inName)
                Toggle("title", isOn: $model.search.inTitle)
                Toggle("body", isOn: $model.search.inBody)
            }
            .toggleStyle(.checkbox)
            .font(theme.small())
        }
    }
}

struct SectionHeader: View {
    @Environment(\.theme) var theme
    @ObservedObject var model: MonitorModel
    let section: SidebarSection

    var body: some View {
        let open = model.isExpanded(section)
        Button { model.toggle(section) } label: {
            HStack(spacing: theme.spacingSmall) {
                Image(systemName: open ? "chevron.down" : "chevron.right").frame(width: theme.cell)
                Text(section.title).font(theme.body(.semibold))
                Spacer()
                Text(String(model.visibleCount(section)))
                    .font(theme.body(.semibold))
                    .foregroundStyle(section.state == "needs_attention" && section.count > 0 ? theme.attention : theme.muted)
            }
            .padding(.horizontal, theme.spacing).padding(.vertical, theme.spacingSmall + 2)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(section.title), \(model.visibleCount(section)), \(open ? "expanded" : "collapsed")")
        .accessibilityIdentifier("section:\(section.state)")
    }
}

/// A sidebar row: key and title, project, activity, time in state, progress
/// cells, the model when one is running, and a link for each blocker.
struct RowView: View {
    @Environment(\.theme) var theme
    @ObservedObject var model: MonitorModel
    let row: TaskRow

    var body: some View {
        let selected = model.page == .task(row.id)
        VStack(alignment: .leading, spacing: 2) {
            Button { model.go(.task(row.id)) } label: {
                VStack(alignment: .leading, spacing: 2) {
                    HStack(alignment: .firstTextBaseline, spacing: theme.spacingSmall) {
                        Text(row.key).font(theme.body(.semibold)).foregroundStyle(theme.muted)
                        Text(row.title).font(theme.body()).foregroundStyle(theme.text).lineLimit(2)
                    }
                    Text("\(row.project), \(activity)")
                        .font(theme.small()).foregroundStyle(theme.muted).lineLimit(2)
                    if !row.proposal.isEmpty {
                        Text(row.proposal).font(theme.small()).foregroundStyle(theme.muted)
                            .accessibilityIdentifier("row:\(row.id):proposal")
                    }
                    HStack(spacing: theme.spacing) {
                        Text(Format.duration(ms: row.inStateMs))
                            .font(theme.small()).foregroundStyle(row.slow ? theme.slow : theme.muted)
                        if !row.typical.isEmpty {
                            Text(row.typical).font(theme.small()).foregroundStyle(theme.muted)
                        }
                        ProgressCells(model: model, cells: row.progress, slow: row.slow)
                        if !row.display.isEmpty || !row.model.isEmpty {
                            Text(row.display.isEmpty ? row.model : row.display)
                                .font(theme.small()).foregroundStyle(theme.muted)
                        }
                    }
                    if let c = row.progress.running { ProgressLine(model: model, cell: c) }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            if row.issue != 0 {
                HStack(spacing: theme.spacing) {
                    if let url = URL(string: row.issueURL), !row.issueURL.isEmpty {
                        Link("issue #\(row.issue)", destination: url).font(theme.small())
                            .accessibilityIdentifier("issue:\(row.id)")
                    } else {
                        Text("issue #\(row.issue)").font(theme.small()).foregroundStyle(theme.muted)
                    }
                    if row.state == "proposed" && model.canAct && row.allowedActions.contains("approve") {
                        Button { model.approve(row.id) } label: { Text("Approve").pill(prominent: true) }
                            .buttonStyle(.plain)
                            .accessibilityIdentifier("row:\(row.id):approve")
                    }
                }
            }
            if !row.blockers.isEmpty {
                HStack(spacing: theme.spacingSmall) {
                    Text("blocked by").font(theme.small()).foregroundStyle(theme.muted)
                    ForEach(row.blockers, id: \.self) { b in
                        Button("#\(b)") { model.go(.task(b)) }
                            .buttonStyle(.link).font(theme.small())
                    }
                }
            }
        }
        .padding(.horizontal, theme.spacing).padding(.vertical, theme.spacingSmall + 1)
        .background(selected ? theme.accent.opacity(0.18) : Color.clear)
        .accessibilityElement(children: .contain)
        .accessibilityLabel("\(row.key) \(row.title), \(row.project), \(row.state)")
        .accessibilityIdentifier("row:\(row.state):\(row.id)")
    }

    var activity: String { row.whyWaiting.isEmpty ? row.activity : row.whyWaiting }
}
